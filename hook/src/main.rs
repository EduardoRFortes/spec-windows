// spec-hook: PreToolUse hook for Claude Code (Windows port of spec-fedora).
//
// Same contract as the Fedora version — see spec-fedora/hook/src/main.rs
// for the full rationale. Only the transport changes: named pipe (via the
// `interprocess` crate) instead of a Unix domain socket. `interprocess`
// local sockets don't expose read/write timeouts directly on Windows named
// pipes, so the ack/decision reads run on a background thread and the
// deadline is enforced with `mpsc::Receiver::recv_timeout` instead.

use serde::Deserialize;
use serde_json::Value;
use std::io::{self, BufRead, BufReader, Read, Write};
use std::process::exit;
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

use interprocess::local_socket::{prelude::*, GenericNamespaced, Stream};
use protocol::{pipe_name, Request, Response};

const CONNECT_ACK_TIMEOUT: Duration = Duration::from_millis(300);
const DECISION_TIMEOUT: Duration = Duration::from_secs(55);

// TEMPORARY: diagnosing why real Claude Code invocations aren't reaching
// specd even though manual test invocations do. stderr isn't visible when
// Claude Code is the one launching this process, so this appends to a file
// instead. Remove once the cause is found.
fn debug_log(msg: &str) {
    use std::io::Write as _;
    let path = std::env::temp_dir().join("spec-hook-debug.log");
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
    {
        let _ = writeln!(f, "[{:?}] {msg}", std::time::SystemTime::now());
    }
}

#[derive(Deserialize)]
struct HookInput {
    session_id: Option<String>,
    prompt_id: Option<String>,
    // Unique per tool call. prompt_id is shared by every tool call within
    // the same turn (a turn can invoke several tools), so it collides as a
    // pending-request key when more than one is in flight — tool_use_id is
    // the one that's actually unique per PreToolUse invocation.
    tool_use_id: Option<String>,
    cwd: Option<String>,
    tool_name: Option<String>,
    permission_mode: Option<String>,
    #[serde(default)]
    tool_input: Value,
}

/// Reads one newline-delimited JSON line off a background thread and
/// enforces `timeout` from the caller's side, since interprocess local
/// sockets don't support native read timeouts on Windows named pipes.
fn read_line_with_timeout<R: Read + Send + 'static>(
    mut reader: BufReader<R>,
    timeout: Duration,
) -> Option<(String, BufReader<R>)> {
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        let mut line = String::new();
        let res = reader.read_line(&mut line);
        let _ = tx.send(res.map(|_| (line, reader)));
    });
    rx.recv_timeout(timeout).ok()?.ok()
}

/// Talks to the daemon end to end. Any failure (no pipe, bad handshake,
/// timeout, malformed response) surfaces as None, which the caller turns
/// into fail-open.
fn talk_to_daemon(input: &HookInput) -> Option<(String, Option<String>)> {
    let tool_name = input.tool_name.as_deref().unwrap_or("").to_string();
    let session_id = input.session_id.as_deref().unwrap_or("").to_string();
    let cwd = input.cwd.as_deref().unwrap_or("").to_string();
    let request_id = input
        .tool_use_id
        .clone()
        .or_else(|| input.prompt_id.clone())
        .unwrap_or_else(|| format!("{session_id}-{tool_name}"));

    let name = match pipe_name().to_ns_name::<GenericNamespaced>() {
        Ok(n) => n,
        Err(e) => {
            debug_log(&format!("to_ns_name failed: {e}"));
            return None;
        }
    };
    let mut conn = match Stream::connect(name) {
        Ok(c) => c,
        Err(e) => {
            debug_log(&format!("connect failed: {e}"));
            return None;
        }
    };
    debug_log("connected to daemon");

    let req = Request::new(
        request_id,
        session_id,
        cwd,
        tool_name,
        input.tool_input.clone(),
    );
    let mut line = serde_json::to_string(&req).ok()?;
    line.push('\n');
    conn.write_all(line.as_bytes()).ok()?;

    let reader = BufReader::new(conn);

    // 1) Fast handshake: proves the daemon is alive and has the request.
    let (ack_line, reader) = read_line_with_timeout(reader, CONNECT_ACK_TIMEOUT)?;
    match serde_json::from_str(ack_line.trim()).ok()? {
        Response::Ack { .. } => {}
        Response::Decision { .. } => return None,
    }

    // 2) Now wait (much longer, but still self-bounded) for the human.
    let (decision_line, _reader) = read_line_with_timeout(reader, DECISION_TIMEOUT)?;
    match serde_json::from_str(decision_line.trim()).ok()? {
        Response::Decision {
            decision, reason, ..
        } => Some((decision, reason)),
        Response::Ack { .. } => None,
    }
}

fn fail_open() -> ! {
    // Exit 0 with empty stdout = "no decision" for Claude Code, which falls
    // back to the normal interactive permission prompt.
    exit(0);
}

fn main() {
    debug_log("=== spec-hook invoked ===");
    let mut buf = String::new();
    if io::stdin().read_to_string(&mut buf).is_err() {
        debug_log("stdin read failed");
        fail_open();
    }
    debug_log(&format!("stdin ({} bytes): {buf:?}", buf.len()));
    // A leading UTF-8 BOM shows up when this is fed via some Windows
    // pipelines (e.g. PowerShell piping a string to a native process's
    // stdin); serde_json treats it as invalid input otherwise.
    let buf = buf.strip_prefix('\u{feff}').unwrap_or(&buf);
    let input: HookInput = match serde_json::from_str(buf) {
        Ok(v) => v,
        Err(e) => {
            debug_log(&format!("json parse failed: {e}"));
            fail_open();
        }
    };

    // Only intervene in the plain interactive mode — see spec-fedora's
    // main.rs for why the other modes take precedence over Spec.
    if input.permission_mode.as_deref().is_some_and(|m| m != "default") {
        debug_log(&format!(
            "permission_mode not default: {:?}",
            input.permission_mode
        ));
        fail_open();
    }

    match talk_to_daemon(&input) {
        Some((decision, reason)) if decision == "allow" || decision == "deny" => {
            debug_log(&format!("decision received: {decision}"));
            let reason = reason.unwrap_or_else(|| {
                if decision == "deny" {
                    "Negado pela bandeja do Spec".to_string()
                } else {
                    "Aprovado pela bandeja do Spec".to_string()
                }
            });
            let out = serde_json::json!({
                "hookSpecificOutput": {
                    "hookEventName": "PreToolUse",
                    "permissionDecision": decision,
                    "permissionDecisionReason": reason,
                }
            });
            // Errors writing stdout are unrecoverable here either way.
            let _ = writeln!(io::stdout(), "{}", out);
            exit(0);
        }
        other => {
            debug_log(&format!("talk_to_daemon returned {other:?}, failing open"));
            fail_open();
        }
    }
}
