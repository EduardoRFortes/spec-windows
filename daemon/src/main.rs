// specd: system tray daemon for spec-windows.
//
// First milestone only: prove a Rust tray icon actually shows up and
// behaves on Windows before wiring in the named-pipe protocol described in
// PROTOCOL.md. No pipe listener yet — that's the next step, mirroring
// spec-fedora's specd (registers pending requests, updates the tray,
// replies with ack/decision).

use std::thread;
use std::time::Duration;

use tray_icon::{
    menu::{Menu, MenuEvent, MenuItem},
    Icon, TrayIconBuilder,
};
use winit::event_loop::{ControlFlow, EventLoop};

mod tray_promote;
use tray_promote::promote_tray_icon;

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

fn main() {
    let event_loop = EventLoop::new().expect("event loop");

    let menu = Menu::new();
    let quit_item = MenuItem::new("Sair", true, None);
    menu.append(&quit_item).expect("append menu item");
    let quit_id = quit_item.id().clone();

    let _tray = TrayIconBuilder::new()
        .with_menu(Box::new(menu))
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

    let menu_channel = MenuEvent::receiver();

    event_loop
        .run(move |_event, elwt| {
            elwt.set_control_flow(ControlFlow::Wait);
            if let Ok(event) = menu_channel.try_recv() {
                if event.id == quit_id {
                    elwt.exit();
                }
            }
        })
        .expect("event loop run");
}
