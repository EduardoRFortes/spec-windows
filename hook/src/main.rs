// spec-hook: PreToolUse hook for Claude Code (Windows port of spec-fedora).
//
// Same contract as the Fedora version — see spec-fedora/hook/src/main.rs
// for the full rationale. Only the transport changes: named pipe (via the
// `interprocess` crate) instead of a Unix domain socket. `interprocess`
// local sockets don't expose read/write timeouts directly on Windows named
// pipes, so the ack/decision reads run on a background thread and the
// deadline is enforced with `mpsc::Receiver::recv_timeout` instead.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::io::{self, BufRead, BufReader, Read, Write};
use std::process::exit;
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

use interprocess::local_socket::{prelude::*, GenericNamespaced, Stream};

const CONNECT_ACK_TIMEOUT: Duration = Duration::from_millis(300);
const DECISION_TIMEOUT: Duration = Duration::from_secs(55);

#[derive(Deserialize)]
struct HookInput {
    session_id: Option<String>,
    prompt_id: Option<String>,
    cwd: Option<String>,
    tool_name: Option<String>,
    permission_mode: Option<String>,
    #[serde(default)]
    tool_input: Value,
}

#[derive(Serialize)]
struct SpecRequest<'a> {
    #[serde(rename = "type")]
    msg_type: &'static str,
    request_id: &'a str,
    session_id: &'a str,
    cwd: &'a str,
    tool_name: &'a str,
    tool_input: &'a Value,
}

#[derive(Deserialize)]
struct SpecResponse {
    #[serde(rename = "type")]
    msg_type: String,
    decision: Option<String>,
    reason: Option<String>,
}

fn pipe_name() -> String {
    std::env::var("SPEC_PIPE").unwrap_or_else(|_| "spec.sock".to_string())
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
    let tool_name = input.tool_name.as_deref().unwrap_or("");
    let session_id = input.session_id.as_deref().unwrap_or("");
    let cwd = input.cwd.as_deref().unwrap_or("");
    let owned_request_id;
    let request_id = match &input.prompt_id {
        Some(id) => id.as_str(),
        None => {
            owned_request_id = format!("{}-{}", session_id, tool_name);
            owned_request_id.as_str()
        }
    };

    let name = pipe_name().to_ns_name::<GenericNamespaced>().ok()?;
    let mut conn = Stream::connect(name).ok()?;

    let req = SpecRequest {
        msg_type: "request",
        request_id,
        session_id,
        cwd,
        tool_name,
        tool_input: &input.tool_input,
    };
    let mut line = serde_json::to_string(&req).ok()?;
    line.push('\n');
    conn.write_all(line.as_bytes()).ok()?;

    let reader = BufReader::new(conn);

    // 1) Fast handshake: proves the daemon is alive and has the request.
    let (ack_line, reader) = read_line_with_timeout(reader, CONNECT_ACK_TIMEOUT)?;
    let ack: SpecResponse = serde_json::from_str(ack_line.trim()).ok()?;
    if ack.msg_type != "ack" {
        return None;
    }

    // 2) Now wait (much longer, but still self-bounded) for the human.
    let (decision_line, _reader) = read_line_with_timeout(reader, DECISION_TIMEOUT)?;
    let resp: SpecResponse = serde_json::from_str(decision_line.trim()).ok()?;
    if resp.msg_type != "decision" {
        return None;
    }

    resp.decision.map(|d| (d, resp.reason))
}

fn fail_open() -> ! {
    // Exit 0 with empty stdout = "no decision" for Claude Code, which falls
    // back to the normal interactive permission prompt.
    exit(0);
}

fn main() {
    let mut buf = String::new();
    if io::stdin().read_to_string(&mut buf).is_err() {
        fail_open();
    }
    let input: HookInput = match serde_json::from_str(&buf) {
        Ok(v) => v,
        Err(_) => fail_open(),
    };

    // Only intervene in the plain interactive mode — see spec-fedora's
    // main.rs for why the other modes take precedence over Spec.
    if input.permission_mode.as_deref().is_some_and(|m| m != "default") {
        fail_open();
    }

    match talk_to_daemon(&input) {
        Some((decision, reason)) if decision == "allow" || decision == "deny" => {
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
        _ => fail_open(),
    }
}
