//! Wire format shared by `hook` and `daemon` — see ../PROTOCOL.md. Kept in
//! one crate so the two binaries can't drift on the message shape.

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Named pipe name, overridable with `SPEC_PIPE` for manual testing.
pub fn pipe_name() -> String {
    std::env::var("SPEC_PIPE").unwrap_or_else(|_| "spec.sock".to_string())
}

/// hook -> daemon.
#[derive(Serialize, Deserialize, Clone)]
pub struct Request {
    #[serde(rename = "type")]
    pub msg_type: String,
    pub request_id: String,
    pub session_id: String,
    pub cwd: String,
    pub tool_name: String,
    #[serde(default)]
    pub tool_input: Value,
}

impl Request {
    pub fn new(
        request_id: impl Into<String>,
        session_id: impl Into<String>,
        cwd: impl Into<String>,
        tool_name: impl Into<String>,
        tool_input: Value,
    ) -> Self {
        Self {
            msg_type: "request".to_string(),
            request_id: request_id.into(),
            session_id: session_id.into(),
            cwd: cwd.into(),
            tool_name: tool_name.into(),
            tool_input,
        }
    }
}

/// hook (spec-statusline) -> daemon, fire-and-forget: no ack/decision, the
/// daemon just updates its cached snapshot for the tray icon/tooltip.
#[derive(Serialize, Deserialize, Default)]
pub struct Usage {
    #[serde(rename = "type")]
    pub msg_type: String,
    pub five_hour_pct: Option<f64>,
    pub five_hour_resets_at: Option<i64>,
    pub seven_day_pct: Option<f64>,
    pub seven_day_resets_at: Option<i64>,
}

impl Usage {
    pub fn new(
        five_hour_pct: Option<f64>,
        five_hour_resets_at: Option<i64>,
        seven_day_pct: Option<f64>,
        seven_day_resets_at: Option<i64>,
    ) -> Self {
        Self {
            msg_type: "usage".to_string(),
            five_hour_pct,
            five_hour_resets_at,
            seven_day_pct,
            seven_day_resets_at,
        }
    }
}

/// daemon -> hook. One enum for both messages of that direction since the
/// hook always knows which one it's expecting at each point in the
/// handshake (ack first, decision second).
#[derive(Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum Response {
    Ack {
        request_id: String,
    },
    Decision {
        request_id: String,
        decision: String,
        #[serde(default)]
        reason: Option<String>,
    },
}
