// specd: system tray daemon for spec-windows.
//
// Listens on the named pipe described in PROTOCOL.md, tracks pending
// requests, and rebuilds the tray menu with an Allow/Deny pair per pending
// request. The event loop polls on a short interval (instead of
// ControlFlow::Wait) because arriving pipe requests are delivered over an
// std::sync::mpsc channel from background threads, not as native window
// messages — Wait would only wake on real OS/menu events, and a tray-only
// app doesn't generate enough of those to notice new requests promptly.

// No console window ever, not even a flash at boot when Task Scheduler
// launches this at logon -- this is a tray app, not a CLI tool. Without
// this, Windows treats it as a console-subsystem binary and briefly shows
// a black window every time it starts. println!/eprintln! below still work
// fine with no console attached (Rust's stdio no-ops instead of erroring).
#![windows_subsystem = "windows"]

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use interprocess::local_socket::{prelude::*, GenericNamespaced, ListenerOptions, Stream};
use protocol::{pipe_name, Request, Response, Usage};
use tray_icon::{
    menu::{Menu, MenuEvent, MenuId, MenuItem, PredefinedMenuItem},
    TrayIcon, TrayIconBuilder,
};
use winit::application::ApplicationHandler;
use winit::event::WindowEvent;
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::window::WindowId;
use winrt_notification::{Duration as ToastDuration, Toast};

mod icon;
mod tray_promote;
use icon::render_icon;
use tray_promote::promote_tray_icon;

#[derive(Default)]
pub struct UsageSnapshot {
    pub five_hour_pct: Option<f64>,
    pub five_hour_resets_at: Option<i64>,
    pub seven_day_pct: Option<f64>,
    pub seven_day_resets_at: Option<i64>,
}

/// The 5h/7d quota is per-*account*, not per-session -- a local Windows
/// session and a remote VM session (see spawn_http_listener) both draw from
/// the same quota, but each only reports the number it personally last saw
/// in an API response. Without this, whichever session happens to talk to
/// specd last wins, even if its cached number is older/lower than what's
/// already showing -- e.g. the VM reports 51%, then ten minutes later a
/// local prompt reports the 49% it had cached from before the VM's last
/// update, and the tray would wrongly drop back to 49%.
///
/// Usage only goes up within a window (it can't decrease until the window
/// rolls over), so: accept a lower number only when `resets_at` actually
/// changed (a genuinely new window); otherwise keep the higher of the two.
fn merge_usage_field(
    current_pct: &mut Option<f64>,
    current_resets_at: &mut Option<i64>,
    new_pct: Option<f64>,
    new_resets_at: Option<i64>,
) {
    let Some(new_pct) = new_pct else { return };
    let new_window = *current_resets_at != new_resets_at;
    let is_higher = current_pct.is_none_or(|p| new_pct >= p);
    if new_window || is_higher {
        *current_pct = Some(new_pct);
        *current_resets_at = new_resets_at;
    }
}

/// Block-character progress bar (e.g. "██████░░░░") -- the tray icon itself
/// is rendered at ~16px by Windows, too small to show a readable bar (see
/// icon.rs), so usage lives here instead: the right-click menu header and
/// the hover tooltip, both plain text but with actual room for a bar +
/// percentage.
fn bar(pct: f64, width: usize) -> String {
    let filled = ((pct.clamp(0.0, 100.0) / 100.0) * width as f64).round() as usize;
    let filled = filled.min(width);
    format!("{}{}", "\u{2588}".repeat(filled), "\u{2591}".repeat(width - filled))
}

/// A tab, not a space, between label and bar: "Sessão" renders narrower
/// than "Semana" in Segoe UI even though both are six characters (the two
/// esses are narrow, "Semana"'s m/n are wide), so padding with more spaces
/// is just guessing at a pixel offset. A tab jumps to the same fixed
/// tab-stop column on every line regardless of label width, which is what
/// actually lines the two bars up.
fn usage_line(label: &str, pct: Option<f64>) -> String {
    match pct {
        Some(p) => format!("{label}\t{} {p:.0}%", bar(p, 10)),
        None => format!("{label}\t?"),
    }
}

const SESSION_LABEL: &str = "Sessão";
const WEEK_LABEL: &str = "Semana";

/// How often the daemon safety-net gives up on an unanswered request if
/// nobody clicks Allow/Deny — matches the 600s ceiling from PROTOCOL.md.
/// In practice the hook itself gives up after ~55s and closes the
/// connection, so this mostly just cleans up the stale menu entry.
const PENDING_SAFETY_TIMEOUT: Duration = Duration::from_secs(600);

/// Idle animation only — a slow blink, not tied to pending requests (the
/// toast notification is what's supposed to get your attention for those).
const IDLE_BLINK_INTERVAL: Duration = Duration::from_secs(5);

struct PendingRequest {
    request_id: String,
    tool_name: String,
    tool_input_preview: String,
    reply: mpsc::Sender<(String, Option<String>)>,
}

enum DaemonEvent {
    NewRequest(PendingRequest),
    Expired(String),
    Usage(Usage),
}

/// One-shot toast for a newly arrived request. Uses the PowerShell AUMID
/// since this isn't installed/packaged with its own — Windows will show it
/// as coming from "Windows PowerShell" until that changes. Best-effort:
/// notification failures shouldn't affect the actual allow/deny flow, so
/// errors are only logged.
fn notify_new_request(tool_name: &str, preview: &str) {
    let result = Toast::new(Toast::POWERSHELL_APP_ID)
        .title("Spec — pedido de permissão")
        .text1(tool_name)
        .text2(preview)
        .duration(ToastDuration::Short)
        .show();
    if let Err(e) = result {
        eprintln!("[specd] toast notification failed: {e:?}");
    }
}

/// The `request` path: ack, register as pending, block this connection's
/// thread on the reply channel until a menu click resolves it (or the
/// PENDING_SAFETY_TIMEOUT net catches an unanswered one).
fn handle_request(line: &str, mut reader: BufReader<Stream>, tx: mpsc::Sender<DaemonEvent>) {
    let req: Request = match serde_json::from_str(line) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("[specd] request parse failed: {e}");
            return;
        }
    };

    let ack = Response::Ack {
        request_id: req.request_id.clone(),
    };
    let Ok(ack_line) = serde_json::to_string(&ack) else {
        return;
    };
    if let Err(e) = writeln!(reader.get_mut(), "{ack_line}") {
        eprintln!("[specd] writing ack failed: {e}");
        return;
    }
    eprintln!("[specd] ack sent for {}", req.request_id);

    let preview: String = serde_json::to_string(&req.tool_input)
        .unwrap_or_default()
        .chars()
        .take(60)
        .collect();

    let (reply_tx, reply_rx) = mpsc::channel::<(String, Option<String>)>();
    let pending = PendingRequest {
        request_id: req.request_id.clone(),
        tool_name: req.tool_name.clone(),
        tool_input_preview: preview,
        reply: reply_tx,
    };
    if tx.send(DaemonEvent::NewRequest(pending)).is_err() {
        return;
    }

    match reply_rx.recv_timeout(PENDING_SAFETY_TIMEOUT) {
        Ok((decision, reason)) => {
            eprintln!("[specd] decision for {}: {decision}", req.request_id);
            let msg = Response::Decision {
                request_id: req.request_id.clone(),
                decision,
                reason,
            };
            if let Ok(line) = serde_json::to_string(&msg) {
                if let Err(e) = writeln!(reader.get_mut(), "{line}") {
                    eprintln!("[specd] writing decision failed (client likely gone): {e}");
                }
            }
        }
        Err(_) => {
            eprintln!("[specd] {} expired unanswered", req.request_id);
            let _ = tx.send(DaemonEvent::Expired(req.request_id));
        }
    }
}

/// The `usage` path: fire-and-forget, no ack, no reply — just relay the
/// snapshot to the event loop and let the connection close.
fn handle_usage(line: &str, tx: mpsc::Sender<DaemonEvent>) {
    let usage: Usage = match serde_json::from_str(line) {
        Ok(u) => u,
        Err(e) => {
            eprintln!("[specd] usage parse failed: {e}");
            return;
        }
    };
    eprintln!(
        "[specd] usage: 5h={:?}% week={:?}%",
        usage.five_hour_pct, usage.seven_day_pct
    );
    let _ = tx.send(DaemonEvent::Usage(usage));
}

fn handle_connection(conn: Stream, tx: mpsc::Sender<DaemonEvent>) {
    eprintln!("[specd] connection accepted");
    let mut reader = BufReader::new(conn);
    let mut line = String::new();
    if let Err(e) = reader.read_line(&mut line) {
        eprintln!("[specd] read_line failed: {e}");
        return;
    }
    let line = line.trim();
    if line.is_empty() {
        eprintln!("[specd] empty line, dropping connection");
        return;
    }
    eprintln!("[specd] received line: {line:?}");

    let Ok(peek) = serde_json::from_str::<serde_json::Value>(line) else {
        eprintln!("[specd] line is not valid JSON, dropping");
        return;
    };
    match peek.get("type").and_then(|v| v.as_str()) {
        Some("request") => handle_request(line, reader, tx),
        Some("usage") => handle_usage(line, tx),
        other => eprintln!("[specd] unknown msg_type {other:?}, dropping"),
    }
}

fn spawn_pipe_listener(tx: mpsc::Sender<DaemonEvent>) {
    thread::spawn(move || {
        let Ok(name) = pipe_name().to_ns_name::<GenericNamespaced>() else {
            eprintln!("[specd] pipe_name().to_ns_name() failed");
            return;
        };
        let listener = match ListenerOptions::new().name(name).create_sync() {
            Ok(l) => l,
            Err(e) => {
                eprintln!("[specd] FATAL: failed to bind pipe: {e}");
                return;
            }
        };
        eprintln!("[specd] listening on pipe {:?}", pipe_name());
        for conn in listener.incoming() {
            let conn = match conn {
                Ok(c) => c,
                Err(e) => {
                    eprintln!("[specd] accept error: {e}");
                    continue;
                }
            };
            let tx = tx.clone();
            thread::spawn(move || handle_connection(conn, tx));
        }
        eprintln!("[specd] listener loop exited (should never happen)");
    });
}

/// Default `SPEC_HTTP_PORT` — only reachable from outside this machine via
/// an explicit SSH `RemoteForward`, never exposed directly (bound to
/// loopback below), so there's no real collision/security surface to pick
/// around.
const DEFAULT_HTTP_PORT: u16 = 27182;

/// Reads exactly `Content-Length` bytes of body after the headers. Not a
/// real HTTP parser — this only ever needs to serve one thing (`POST
/// /usage`, see PROTOCOL.md), so anything past "find Content-Length, read
/// that many bytes" is unneeded surface.
fn read_http_body(stream: &TcpStream) -> Option<String> {
    let mut reader = BufReader::new(stream);
    let mut content_length: Option<usize> = None;
    loop {
        let mut header_line = String::new();
        if reader.read_line(&mut header_line).ok()? == 0 {
            return None;
        }
        let header_line = header_line.trim_end();
        if header_line.is_empty() {
            break;
        }
        if let Some(value) = header_line
            .to_ascii_lowercase()
            .strip_prefix("content-length:")
        {
            content_length = value.trim().parse().ok();
        }
    }
    let mut body = vec![0u8; content_length?];
    reader.read_exact(&mut body).ok()?;
    String::from_utf8(body).ok()
}

fn handle_http_connection(mut stream: TcpStream, tx: mpsc::Sender<DaemonEvent>) {
    let body = {
        let Some(b) = read_http_body(&stream) else {
            eprintln!("[specd] http: bad request, dropping");
            return;
        };
        b
    };
    handle_usage(&body, tx);
    let _ =
        stream.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
}

/// TCP counterpart to the named pipe, for Claude Code sessions that never
/// touch this Windows machine directly -- a VM/container reached over SSH
/// (including VS Code Remote - SSH) runs `spec-statusline-remote.py`
/// instead of `spec-statusline.exe`, and POSTs usage JSON here through an
/// SSH `RemoteForward` tunnel (`RemoteForward 27182 localhost:27182`) set up
/// on the Windows side. Bound to loopback only: the tunnel is what makes
/// this reachable from the VM at all, not an open port. Same fail-open rule
/// as the pipe path — if the tunnel isn't up or this isn't running, the
/// remote hook's POST just fails silently and Claude Code is unaffected.
fn spawn_http_listener(tx: mpsc::Sender<DaemonEvent>) {
    thread::spawn(move || {
        let port: u16 = std::env::var("SPEC_HTTP_PORT")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(DEFAULT_HTTP_PORT);
        let listener = match TcpListener::bind(("127.0.0.1", port)) {
            Ok(l) => l,
            Err(e) => {
                eprintln!("[specd] failed to bind http 127.0.0.1:{port}: {e}");
                return;
            }
        };
        eprintln!("[specd] listening on http 127.0.0.1:{port}");
        for conn in listener.incoming() {
            let conn = match conn {
                Ok(c) => c,
                Err(e) => {
                    eprintln!("[specd] http accept error: {e}");
                    continue;
                }
            };
            let tx = tx.clone();
            thread::spawn(move || handle_http_connection(conn, tx));
        }
    });
}

fn rebuild_menu(
    tray: &TrayIcon,
    quit_id: &MenuId,
    pending: &HashMap<String, PendingRequest>,
    usage: &UsageSnapshot,
) {
    let menu = Menu::new();

    if usage.five_hour_pct.is_some() || usage.seven_day_pct.is_some() {
        let _ = menu.append(&MenuItem::new(
            usage_line(SESSION_LABEL, usage.five_hour_pct),
            false,
            None,
        ));
        let _ = menu.append(&MenuItem::new(
            usage_line(WEEK_LABEL, usage.seven_day_pct),
            false,
            None,
        ));
        let _ = menu.append(&PredefinedMenuItem::separator());
    }

    for req in pending.values() {
        let header = MenuItem::new(
            format!("{} — {}", req.tool_name, req.tool_input_preview),
            false,
            None,
        );
        let allow = MenuItem::with_id(
            MenuId::new(format!("allow:{}", req.request_id)),
            "  \u{2713} Permitir",
            true,
            None,
        );
        let deny = MenuItem::with_id(
            MenuId::new(format!("deny:{}", req.request_id)),
            "  \u{2717} Negar",
            true,
            None,
        );
        let _ = menu.append(&header);
        let _ = menu.append(&allow);
        let _ = menu.append(&deny);
    }
    if !pending.is_empty() {
        let _ = menu.append(&PredefinedMenuItem::separator());
    }
    let quit_item = MenuItem::with_id(quit_id.clone(), "Sair", true, None);
    let _ = menu.append(&quit_item);
    tray.set_menu(Some(Box::new(menu)));
}

fn tooltip_text(usage: &UsageSnapshot) -> String {
    if usage.five_hour_pct.is_none() && usage.seven_day_pct.is_none() {
        return "spec-windows".to_string();
    }
    format!(
        "{}\n{}",
        usage_line(SESSION_LABEL, usage.five_hour_pct),
        usage_line(WEEK_LABEL, usage.seven_day_pct)
    )
}

/// Everything the tick loop touches. winit 0.30 deprecated the old
/// closure-based `EventLoop::run` in favor of this handler trait run via
/// `run_app` -- we don't actually care about individual winit events (no
/// window of our own; the tray icon and its menu live outside winit's
/// window system), we just need a steady wakeup to drain `daemon_rx` and
/// the menu click channel, which `about_to_wait` gives us.
struct App {
    tray: TrayIcon,
    quit_id: MenuId,
    pending: HashMap<String, PendingRequest>,
    usage: UsageSnapshot,
    eyes_open: bool,
    last_blink: Instant,
    daemon_rx: mpsc::Receiver<DaemonEvent>,
    menu_channel: &'static tray_icon::menu::MenuEventReceiver,
}

impl ApplicationHandler for App {
    fn resumed(&mut self, _event_loop: &ActiveEventLoop) {}

    fn window_event(&mut self, _event_loop: &ActiveEventLoop, _id: WindowId, _event: WindowEvent) {}

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        event_loop.set_control_flow(ControlFlow::WaitUntil(
            Instant::now() + Duration::from_millis(200),
        ));

        let mut menu_dirty = false;
        let mut icon_dirty = false;

        while let Ok(ev) = self.daemon_rx.try_recv() {
            match ev {
                DaemonEvent::NewRequest(req) => {
                    notify_new_request(&req.tool_name, &req.tool_input_preview);
                    self.pending.insert(req.request_id.clone(), req);
                    menu_dirty = true;
                }
                DaemonEvent::Expired(id) => {
                    menu_dirty |= self.pending.remove(&id).is_some();
                }
                DaemonEvent::Usage(u) => {
                    merge_usage_field(
                        &mut self.usage.five_hour_pct,
                        &mut self.usage.five_hour_resets_at,
                        u.five_hour_pct,
                        u.five_hour_resets_at,
                    );
                    merge_usage_field(
                        &mut self.usage.seven_day_pct,
                        &mut self.usage.seven_day_resets_at,
                        u.seven_day_pct,
                        u.seven_day_resets_at,
                    );
                    menu_dirty = true;
                    icon_dirty = true;
                    self.tray.set_tooltip(Some(tooltip_text(&self.usage))).ok();
                }
            }
        }

        if let Ok(event) = self.menu_channel.try_recv() {
            let id = event.id.0.as_str();
            if event.id == self.quit_id {
                event_loop.exit();
            } else if let Some(request_id) = id.strip_prefix("allow:") {
                if let Some(req) = self.pending.remove(request_id) {
                    let _ = req.reply.send(("allow".to_string(), None));
                    menu_dirty = true;
                }
            } else if let Some(request_id) = id.strip_prefix("deny:") {
                if let Some(req) = self.pending.remove(request_id) {
                    let _ = req.reply.send(("deny".to_string(), None));
                    menu_dirty = true;
                }
            }
        }

        if menu_dirty {
            rebuild_menu(&self.tray, &self.quit_id, &self.pending, &self.usage);
        }

        if self.last_blink.elapsed() >= IDLE_BLINK_INTERVAL {
            self.eyes_open = !self.eyes_open;
            self.last_blink = Instant::now();
            icon_dirty = true;
        }

        if icon_dirty {
            self.tray.set_icon(Some(render_icon(self.eyes_open))).ok();
        }
    }
}

fn main() {
    eprintln!("[specd] starting, pid={}", std::process::id());
    let event_loop = EventLoop::new().expect("event loop");

    let quit_id = MenuId::new("quit");
    let initial_menu = Menu::new();
    initial_menu
        .append(&MenuItem::with_id(quit_id.clone(), "Sair", true, None))
        .expect("append quit item");

    let no_usage = UsageSnapshot::default();
    let tray = TrayIconBuilder::new()
        .with_menu(Box::new(initial_menu))
        .with_tooltip(tooltip_text(&no_usage))
        .with_icon(render_icon(true))
        .build()
        .expect("tray icon");

    // Explorer only creates our NotifyIconSettings entry after the first
    // Shell_NotifyIcon call above, and that can lag a little — retry for a
    // few seconds instead of trying once.
    thread::spawn(|| {
        for _ in 0..10 {
            if promote_tray_icon() {
                break;
            }
            thread::sleep(Duration::from_millis(300));
        }
    });

    let (daemon_tx, daemon_rx) = mpsc::channel::<DaemonEvent>();
    spawn_pipe_listener(daemon_tx.clone());
    spawn_http_listener(daemon_tx);

    let mut app = App {
        tray,
        quit_id,
        pending: HashMap::new(),
        usage: UsageSnapshot::default(),
        eyes_open: true,
        last_blink: Instant::now(),
        daemon_rx,
        menu_channel: MenuEvent::receiver(),
    };

    event_loop.run_app(&mut app).expect("event loop run");
}
