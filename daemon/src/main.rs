// specd: system tray daemon for spec-windows.
//
// Listens on the named pipe described in PROTOCOL.md, tracks pending
// requests, and rebuilds the tray menu with an Allow/Deny pair per pending
// request. The event loop polls on a short interval (instead of
// ControlFlow::Wait) because arriving pipe requests are delivered over an
// std::sync::mpsc channel from background threads, not as native window
// messages — Wait would only wake on real OS/menu events, and a tray-only
// app doesn't generate enough of those to notice new requests promptly.

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use interprocess::local_socket::{prelude::*, GenericNamespaced, ListenerOptions, Stream};
use protocol::{pipe_name, Request, Response};
use tray_icon::{
    menu::{Menu, MenuEvent, MenuId, MenuItem, PredefinedMenuItem},
    Icon, TrayIcon, TrayIconBuilder,
};
use winit::event_loop::{ControlFlow, EventLoop};

mod tray_promote;
use tray_promote::promote_tray_icon;

/// How often the daemon safety-net gives up on an unanswered request if
/// nobody clicks Allow/Deny — matches the 600s ceiling from PROTOCOL.md.
/// In practice the hook itself gives up after ~55s and closes the
/// connection, so this mostly just cleans up the stale menu entry.
const PENDING_SAFETY_TIMEOUT: Duration = Duration::from_secs(600);

struct PendingRequest {
    request_id: String,
    tool_name: String,
    tool_input_preview: String,
    reply: mpsc::Sender<(String, Option<String>)>,
}

enum DaemonEvent {
    NewRequest(PendingRequest),
    Expired(String),
}

/// Solid-color 16x16 placeholder until there's real tray art. Swapped out
/// once the request/no-request icon states are designed.
fn placeholder_icon() -> Icon {
    let size = 16u32;
    let mut rgba = Vec::with_capacity((size * size * 4) as usize);
    for _ in 0..(size * size) {
        rgba.extend_from_slice(&[0x2b, 0x8a, 0x3e, 0xff]);
    }
    Icon::from_rgba(rgba, size, size).expect("valid icon buffer")
}

fn handle_connection(conn: Stream, tx: mpsc::Sender<DaemonEvent>) {
    let mut reader = BufReader::new(conn);
    let mut line = String::new();
    if reader.read_line(&mut line).is_err() || line.trim().is_empty() {
        return;
    }
    let req: Request = match serde_json::from_str(line.trim()) {
        Ok(r) => r,
        Err(_) => return,
    };
    if req.msg_type != "request" {
        // Not implemented yet on this port: `usage`, fire-and-forget from
        // spec-statusline. Nothing to ack, just drop the connection.
        return;
    }

    let ack = Response::Ack {
        request_id: req.request_id.clone(),
    };
    let Ok(ack_line) = serde_json::to_string(&ack) else {
        return;
    };
    if writeln!(reader.get_mut(), "{ack_line}").is_err() {
        return;
    }

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
            let msg = Response::Decision {
                request_id: req.request_id.clone(),
                decision,
                reason,
            };
            if let Ok(line) = serde_json::to_string(&msg) {
                let _ = writeln!(reader.get_mut(), "{line}");
            }
        }
        Err(_) => {
            let _ = tx.send(DaemonEvent::Expired(req.request_id));
        }
    }
}

fn spawn_pipe_listener(tx: mpsc::Sender<DaemonEvent>) {
    thread::spawn(move || {
        let Ok(name) = pipe_name().to_ns_name::<GenericNamespaced>() else {
            return;
        };
        let Ok(listener) = ListenerOptions::new().name(name).create_sync() else {
            return;
        };
        for conn in listener.incoming() {
            let Ok(conn) = conn else { continue };
            let tx = tx.clone();
            thread::spawn(move || handle_connection(conn, tx));
        }
    });
}

fn rebuild_menu(tray: &TrayIcon, quit_id: &MenuId, pending: &HashMap<String, PendingRequest>) {
    let menu = Menu::new();
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

fn main() {
    let event_loop = EventLoop::new().expect("event loop");

    let quit_id = MenuId::new("quit");
    let initial_menu = Menu::new();
    initial_menu
        .append(&MenuItem::with_id(quit_id.clone(), "Sair", true, None))
        .expect("append quit item");

    let tray = TrayIconBuilder::new()
        .with_menu(Box::new(initial_menu))
        .with_tooltip("spec-windows (protótipo)")
        .with_icon(placeholder_icon())
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

    event_loop
        .run(move |_event, elwt| {
            elwt.set_control_flow(ControlFlow::WaitUntil(
                Instant::now() + Duration::from_millis(200),
            ));

            let mut dirty = false;

            while let Ok(ev) = daemon_rx.try_recv() {
                match ev {
                    DaemonEvent::NewRequest(req) => {
                        pending.insert(req.request_id.clone(), req);
                        dirty = true;
                    }
                    DaemonEvent::Expired(id) => {
                        dirty |= pending.remove(&id).is_some();
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
                        dirty = true;
                    }
                } else if let Some(request_id) = id.strip_prefix("deny:") {
                    if let Some(req) = pending.remove(request_id) {
                        let _ = req.reply.send(("deny".to_string(), None));
                        dirty = true;
                    }
                }
            }

            if dirty {
                rebuild_menu(&tray, &quit_id, &pending);
            }
        })
        .expect("event loop run");
}
