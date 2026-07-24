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
use std::io::{BufRead, BufReader, Write};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use interprocess::local_socket::{prelude::*, GenericNamespaced, ListenerOptions, Stream};
use protocol::{pipe_name, Request, Response, Usage};
use tray_icon::{
    menu::{Menu, MenuEvent, MenuId, MenuItem, PredefinedMenuItem},
    TrayIcon, TrayIconBuilder,
};
use winit::event_loop::{ControlFlow, EventLoop};
use winrt_notification::{Duration as ToastDuration, Toast};

mod icon;
mod tray_promote;
use icon::render_icon;
use tray_promote::promote_tray_icon;

pub struct UsageSnapshot {
    pub five_hour_pct: Option<f64>,
    pub seven_day_pct: Option<f64>,
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

fn main() {
    eprintln!("[specd] starting, pid={}", std::process::id());
    let event_loop = EventLoop::new().expect("event loop");

    let quit_id = MenuId::new("quit");
    let initial_menu = Menu::new();
    initial_menu
        .append(&MenuItem::with_id(quit_id.clone(), "Sair", true, None))
        .expect("append quit item");

    let no_usage = UsageSnapshot {
        five_hour_pct: None,
        seven_day_pct: None,
    };
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
    spawn_pipe_listener(daemon_tx);

    let menu_channel = MenuEvent::receiver();
    let mut pending: HashMap<String, PendingRequest> = HashMap::new();
    let mut usage = UsageSnapshot {
        five_hour_pct: None,
        seven_day_pct: None,
    };
    let mut eyes_open = true;
    let mut last_blink = Instant::now();

    event_loop
        .run(move |_event, elwt| {
            elwt.set_control_flow(ControlFlow::WaitUntil(
                Instant::now() + Duration::from_millis(200),
            ));

            let mut menu_dirty = false;
            let mut icon_dirty = false;

            while let Ok(ev) = daemon_rx.try_recv() {
                match ev {
                    DaemonEvent::NewRequest(req) => {
                        notify_new_request(&req.tool_name, &req.tool_input_preview);
                        pending.insert(req.request_id.clone(), req);
                        menu_dirty = true;
                    }
                    DaemonEvent::Expired(id) => {
                        menu_dirty |= pending.remove(&id).is_some();
                    }
                    DaemonEvent::Usage(u) => {
                        usage = UsageSnapshot {
                            five_hour_pct: u.five_hour_pct,
                            seven_day_pct: u.seven_day_pct,
                        };
                        menu_dirty = true;
                        icon_dirty = true;
                        tray.set_tooltip(Some(tooltip_text(&usage))).ok();
                    }
                }
            }

            if let Ok(event) = menu_channel.try_recv() {
                let id = event.id.0.as_str();
                if event.id == quit_id {
                    elwt.exit();
                } else if let Some(request_id) = id.strip_prefix("allow:") {
                    if let Some(req) = pending.remove(request_id) {
                        let _ = req.reply.send(("allow".to_string(), None));
                        menu_dirty = true;
                    }
                } else if let Some(request_id) = id.strip_prefix("deny:") {
                    if let Some(req) = pending.remove(request_id) {
                        let _ = req.reply.send(("deny".to_string(), None));
                        menu_dirty = true;
                    }
                }
            }

            if menu_dirty {
                rebuild_menu(&tray, &quit_id, &pending, &usage);
            }

            if last_blink.elapsed() >= IDLE_BLINK_INTERVAL {
                eyes_open = !eyes_open;
                last_blink = Instant::now();
                icon_dirty = true;
            }

            if icon_dirty {
                tray.set_icon(Some(render_icon(eyes_open))).ok();
            }
        })
        .expect("event loop run");
}
