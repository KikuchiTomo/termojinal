//! IPC message types for communication between `jt` CLI and `termojinald`.
//!
//! All messages are JSON-serialized and sent over a Unix domain socket.
//! The protocol is line-delimited: each message is a single JSON line
//! terminated by a newline character.

use serde::{Deserialize, Serialize};

/// A request from the client to the daemon.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum IpcRequest {
    /// Check that the daemon is alive.
    Ping,

    /// List all active sessions (IDs only).
    ListSessions,

    /// List all active sessions with full details.
    ListSessionDetails,

    /// Create a new session.
    CreateSession {
        shell: Option<String>,
        cwd: Option<String>,
    },

    /// Kill a session by ID.
    KillSession { id: String },

    /// Resize a session's PTY.
    ResizeSession { id: String, cols: u16, rows: u16 },

    /// Focus a specific pane.
    FocusPane { id: u64 },

    /// Split the current pane.
    SplitPane {
        /// "horizontal" or "vertical"
        direction: String,
    },

    /// Close the current pane.
    ClosePane,

    /// Register an externally-spawned session (e.g. UI-spawned PTY) so the
    /// daemon can track it for `tm list` without owning the PTY.
    RegisterSession {
        pane_id: u64,
        pid: i32,
        shell: String,
        cwd: String,
        cols: u16,
        rows: u16,
    },

    /// Unregister a previously registered external session.
    UnregisterSession { pane_id: u64 },

    /// Gracefully exit a session by ID. Sends SIGHUP to the shell process.
    /// If a foreground process is running, reports it so the client can confirm.
    ExitSession { id: String },

    /// Kill all sessions (daemon-owned and externally tracked).
    KillAll,

    /// Claude Code status update from hooks.
    ///
    /// Sent by `tm status` when a Claude Code hook fires (PreToolUse,
    /// PostToolUse, Stop). The daemon forwards this to the session monitor
    /// so the GUI can update agent state without polling.
    ClaudeStatusUpdate {
        /// Claude Code session ID (`$CLAUDE_SESSION_ID`).
        #[serde(default)]
        session_id: Option<String>,
        /// State string: "running", "done", "idle".
        state: String,
        /// Subagent ID (for subagent-start / subagent-done).
        #[serde(default)]
        agent_id: Option<String>,
        /// Subagent type (e.g. "task", "search").
        #[serde(default)]
        agent_type: Option<String>,
        /// Subagent description.
        #[serde(default)]
        description: Option<String>,
        /// PID of the notifying process (used to identify which PTY pane).
        #[serde(default)]
        pid: Option<i32>,
    },
}

/// A response from the daemon to the client.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct IpcResponse {
    /// Whether the request was processed successfully.
    pub success: bool,

    /// Optional JSON payload (varies by request type).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<serde_json::Value>,

    /// Error message if `success` is false.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

impl IpcResponse {
    /// Create a successful response with data.
    pub fn ok(data: serde_json::Value) -> Self {
        Self {
            success: true,
            data: Some(data),
            error: None,
        }
    }

    /// Create a successful response with no data.
    pub fn ok_empty() -> Self {
        Self {
            success: true,
            data: None,
            error: None,
        }
    }

    /// Create an error response.
    pub fn err(message: impl Into<String>) -> Self {
        Self {
            success: false,
            data: None,
            error: Some(message.into()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_serialize_ping() {
        let req = IpcRequest::Ping;
        let json = serde_json::to_string(&req).unwrap();
        assert_eq!(json, r#"{"type":"ping"}"#);
    }

    #[test]
    fn test_deserialize_ping() {
        let req: IpcRequest = serde_json::from_str(r#"{"type":"ping"}"#).unwrap();
        assert_eq!(req, IpcRequest::Ping);
    }

    #[test]
    fn test_serialize_list_sessions() {
        let req = IpcRequest::ListSessions;
        let json = serde_json::to_string(&req).unwrap();
        assert_eq!(json, r#"{"type":"list_sessions"}"#);
    }

    #[test]
    fn test_serialize_list_session_details() {
        let req = IpcRequest::ListSessionDetails;
        let json = serde_json::to_string(&req).unwrap();
        assert_eq!(json, r#"{"type":"list_session_details"}"#);
    }

    #[test]
    fn test_serialize_create_session_full() {
        let req = IpcRequest::CreateSession {
            shell: Some("/bin/zsh".to_string()),
            cwd: Some("/home/user".to_string()),
        };
        let json = serde_json::to_string(&req).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed["type"], "create_session");
        assert_eq!(parsed["shell"], "/bin/zsh");
        assert_eq!(parsed["cwd"], "/home/user");
    }

    #[test]
    fn test_serialize_create_session_defaults() {
        let req = IpcRequest::CreateSession {
            shell: None,
            cwd: None,
        };
        let json = serde_json::to_string(&req).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed["type"], "create_session");
        assert_eq!(parsed["shell"], serde_json::Value::Null);
    }

    #[test]
    fn test_serialize_kill_session() {
        let req = IpcRequest::KillSession {
            id: "abc-123".to_string(),
        };
        let json = serde_json::to_string(&req).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed["type"], "kill_session");
        assert_eq!(parsed["id"], "abc-123");
    }

    #[test]
    fn test_serialize_resize_session() {
        let req = IpcRequest::ResizeSession {
            id: "abc-123".to_string(),
            cols: 120,
            rows: 40,
        };
        let json = serde_json::to_string(&req).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed["type"], "resize_session");
        assert_eq!(parsed["cols"], 120);
        assert_eq!(parsed["rows"], 40);
    }

    #[test]
    fn test_serialize_focus_pane() {
        let req = IpcRequest::FocusPane { id: 42 };
        let json = serde_json::to_string(&req).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed["type"], "focus_pane");
        assert_eq!(parsed["id"], 42);
    }

    #[test]
    fn test_serialize_split_pane() {
        let req = IpcRequest::SplitPane {
            direction: "horizontal".to_string(),
        };
        let json = serde_json::to_string(&req).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed["type"], "split_pane");
        assert_eq!(parsed["direction"], "horizontal");
    }

    #[test]
    fn test_serialize_close_pane() {
        let req = IpcRequest::ClosePane;
        let json = serde_json::to_string(&req).unwrap();
        assert_eq!(json, r#"{"type":"close_pane"}"#);
    }

    #[test]
    fn test_serialize_claude_status_update() {
        let req = IpcRequest::ClaudeStatusUpdate {
            session_id: Some("sess-abc".to_string()),
            state: "running".to_string(),
            agent_id: None,
            agent_type: None,
            description: None,
            pid: Some(12345),
        };
        let json = serde_json::to_string(&req).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed["type"], "claude_status_update");
        assert_eq!(parsed["session_id"], "sess-abc");
        assert_eq!(parsed["state"], "running");
        assert_eq!(parsed["pid"], 12345);
    }

    #[test]
    fn test_deserialize_claude_status_update_minimal() {
        let req: IpcRequest = serde_json::from_str(
            r#"{"type":"claude_status_update","state":"done"}"#,
        )
        .unwrap();
        assert_eq!(
            req,
            IpcRequest::ClaudeStatusUpdate {
                session_id: None,
                state: "done".to_string(),
                agent_id: None,
                agent_type: None,
                description: None,
                pid: None,
            }
        );
    }

    #[test]
    fn test_serialize_claude_status_update_subagent() {
        let req = IpcRequest::ClaudeStatusUpdate {
            session_id: Some("sess-1".to_string()),
            state: "running".to_string(),
            agent_id: Some("agent-42".to_string()),
            agent_type: Some("task".to_string()),
            description: Some("fixing bug".to_string()),
            pid: Some(9999),
        };
        let json = serde_json::to_string(&req).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed["type"], "claude_status_update");
        assert_eq!(parsed["agent_id"], "agent-42");
        assert_eq!(parsed["agent_type"], "task");
        assert_eq!(parsed["description"], "fixing bug");
    }

    #[test]
    fn test_response_ok() {
        let resp = IpcResponse::ok(json!({"sessions": ["a", "b"]}));
        assert!(resp.success);
        assert!(resp.data.is_some());
        assert!(resp.error.is_none());

        let json = serde_json::to_string(&resp).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed["success"], true);
        assert!(parsed.get("error").is_none()); // skip_serializing_if
    }

    #[test]
    fn test_response_ok_empty() {
        let resp = IpcResponse::ok_empty();
        assert!(resp.success);
        assert!(resp.data.is_none());
        assert!(resp.error.is_none());
    }

    #[test]
    fn test_response_err() {
        let resp = IpcResponse::err("session not found");
        assert!(!resp.success);
        assert!(resp.data.is_none());
        assert_eq!(resp.error.as_deref(), Some("session not found"));
    }

    #[test]
    fn test_roundtrip_request() {
        let requests = vec![
            IpcRequest::Ping,
            IpcRequest::ListSessions,
            IpcRequest::ListSessionDetails,
            IpcRequest::CreateSession {
                shell: Some("/bin/bash".to_string()),
                cwd: None,
            },
            IpcRequest::KillSession {
                id: "test-id".to_string(),
            },
            IpcRequest::ResizeSession {
                id: "test-id".to_string(),
                cols: 80,
                rows: 24,
            },
            IpcRequest::FocusPane { id: 1 },
            IpcRequest::SplitPane {
                direction: "vertical".to_string(),
            },
            IpcRequest::ClosePane,
            IpcRequest::RegisterSession {
                pane_id: 1,
                pid: 1234,
                shell: "/bin/zsh".to_string(),
                cwd: "/tmp".to_string(),
                cols: 80,
                rows: 24,
            },
            IpcRequest::UnregisterSession { pane_id: 1 },
            IpcRequest::ExitSession {
                id: "test-id".to_string(),
            },
            IpcRequest::KillAll,
            IpcRequest::ClaudeStatusUpdate {
                session_id: Some("sess-1".to_string()),
                state: "running".to_string(),
                agent_id: None,
                agent_type: None,
                description: None,
                pid: Some(42),
            },
            IpcRequest::ClaudeStatusUpdate {
                session_id: None,
                state: "done".to_string(),
                agent_id: Some("a-1".to_string()),
                agent_type: Some("task".to_string()),
                description: Some("desc".to_string()),
                pid: None,
            },
        ];

        for req in requests {
            let json = serde_json::to_string(&req).unwrap();
            let deserialized: IpcRequest = serde_json::from_str(&json).unwrap();
            assert_eq!(req, deserialized);
        }
    }

    #[test]
    fn test_roundtrip_response() {
        let responses = vec![
            IpcResponse::ok(json!({"key": "value"})),
            IpcResponse::ok_empty(),
            IpcResponse::err("something went wrong"),
        ];

        for resp in responses {
            let json = serde_json::to_string(&resp).unwrap();
            let deserialized: IpcResponse = serde_json::from_str(&json).unwrap();
            assert_eq!(resp, deserialized);
        }
    }
}
