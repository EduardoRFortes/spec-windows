// spec-statusline: statusLine hook for Claude Code (Windows port of
// spec-fedora's hook/src/bin/spec-statusline.rs — see that file for the
// full rationale).
//
// Claude Code renders whatever this prints as the terminal status line, and
// invokes it on an event-driven, debounced (~300ms) basis whenever session
// data changes. It receives a JSON payload with a "rate_limits" object
// (Claude.ai Pro/Max subscribers only, absent otherwise) containing the
// official 5-hour and 7-day usage percentages straight from Anthropic's
// API — not something scraped from `/usage` output.
//
// This binary does two things with that data:
//   1. Prints a compact status line so the terminal keeps showing it.
//   2. Forwards it to specd (fire-and-forget) so the tray can show it too.
//
// Unlike spec-hook, nothing here can block Claude Code's UI on a decision,
// so there's no ack/decision handshake: connect, write one line, done. Any
// failure (daemon down, timeout) just means the tray doesn't get this
// update — it must never affect what gets printed to stdout.

use serde::Deserialize;
use std::io::{self, Read, Write};

use interprocess::local_socket::{prelude::*, GenericNamespaced, Stream};
use protocol::{pipe_name, Usage};

#[derive(Deserialize, Clone)]
struct RateLimitWindow {
    used_percentage: Option<f64>,
    resets_at: Option<i64>,
}

#[derive(Deserialize)]
struct RateLimits {
    five_hour: Option<RateLimitWindow>,
    seven_day: Option<RateLimitWindow>,
}

#[derive(Deserialize)]
struct StatusLineInput {
    #[serde(default)]
    rate_limits: Option<RateLimits>,
}

/// Best-effort forward to specd. Never lets a slow/dead daemon delay the
/// status line: connect, write, move on. interprocess local sockets don't
/// expose a Windows-native write timeout, so a hung daemon would block this
/// indefinitely in theory — acceptable here since specd normally drains
/// connections instantly and this only runs on a debounced UI callback, not
/// a user-facing decision path like spec-hook's.
fn forward_to_daemon(five_hour: &Option<RateLimitWindow>, seven_day: &Option<RateLimitWindow>) {
    let msg = Usage::new(
        five_hour.as_ref().and_then(|w| w.used_percentage),
        five_hour.as_ref().and_then(|w| w.resets_at),
        seven_day.as_ref().and_then(|w| w.used_percentage),
        seven_day.as_ref().and_then(|w| w.resets_at),
    );
    let Ok(line) = serde_json::to_string(&msg) else {
        return;
    };
    let Ok(name) = pipe_name().to_ns_name::<GenericNamespaced>() else {
        return;
    };
    let Ok(mut conn) = Stream::connect(name) else {
        return;
    };
    let _ = conn.write_all(format!("{line}\n").as_bytes());
}

fn render_status_line(
    five_hour: &Option<RateLimitWindow>,
    seven_day: &Option<RateLimitWindow>,
) -> Option<String> {
    let fh = five_hour.as_ref().and_then(|w| w.used_percentage)?;
    let wk = seven_day.as_ref().and_then(|w| w.used_percentage)?;
    Some(format!("\u{1FAA8} sessão {fh:.0}% · semana {wk:.0}%"))
}

fn main() {
    let mut buf = String::new();
    if io::stdin().read_to_string(&mut buf).is_err() {
        return;
    }
    let buf = buf.strip_prefix('\u{feff}').unwrap_or(&buf);
    let Ok(input) = serde_json::from_str::<StatusLineInput>(buf) else {
        return;
    };

    let five_hour = input.rate_limits.as_ref().and_then(|r| r.five_hour.clone());
    let seven_day = input.rate_limits.as_ref().and_then(|r| r.seven_day.clone());

    // rate_limits is absent for non-Pro/Max accounts and before the first
    // API response of the session — print nothing rather than a blank or
    // misleading line.
    if let Some(line) = render_status_line(&five_hour, &seven_day) {
        println!("{line}");
    }

    forward_to_daemon(&five_hour, &seven_day);
}
