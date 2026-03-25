//! IPC message types for communication between `jt` CLI and `termojinald`.
//!
//! ## Protocol overview
//!
//! The daemon uses a length-prefixed binary framing protocol for efficient
//! transport of both control messages and raw PTY data:
//!
//! ```text
//! Frame: [4-byte length (big-endian u32)][1-byte type][payload]
//!
//! Type 0x01: Control message (JSON payload) -- bidirectional
//! Type 0x02: PTY output data (raw bytes)   -- Daemon -> GUI
//! Type 0x03: Key input data (raw bytes)    -- GUI -> Daemon
//! Type 0x04: Snapshot data (serialized)    -- Daemon -> GUI (re-attach)
//! ```
//!
//! For PTY data frames (0x02, 0x03, 0x04), the payload is:
//! ```text
//! [session_id_len: u8][session_id: bytes][data: bytes]
//! ```
//!
//! The legacy line-delimited JSON protocol is still used by the `tm` CLI
//! for simple request/response interactions.

use serde::{Deserialize, Serialize};
use std::io;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

// ---------------------------------------------------------------------------
// Frame type constants
// ---------------------------------------------------------------------------

/// Control message frame (JSON payload).
pub const MSG_CONTROL: u8 = 0x01;
/// PTY output data frame (Daemon -> GUI).
pub const MSG_PTY_OUTPUT: u8 = 0x02;
/// Key input data frame (GUI -> Daemon).
pub const MSG_KEY_INPUT: u8 = 0x03;
/// Terminal snapshot frame (Daemon -> GUI, on re-attach).
pub const MSG_SNAPSHOT: u8 = 0x04;

/// Maximum frame payload size: 16 MiB.
/// This limit prevents a malformed length header from causing unbounded
/// memory allocation.
pub const MAX_FRAME_SIZE: u32 = 16 * 1024 * 1024;

// ---------------------------------------------------------------------------
// Frame
// ---------------------------------------------------------------------------

/// A single binary frame on the wire.
#[derive(Debug, Clone)]
pub struct Frame {
    pub msg_type: u8,
    pub payload: Vec<u8>,
}

impl Frame {
    /// Create a control frame containing a JSON-serialized IPC request.
    pub fn control_request(request: &IpcRequest) -> Result<Self, serde_json::Error> {
        let payload = serde_json::to_vec(request)?;
        Ok(Frame {
            msg_type: MSG_CONTROL,
            payload,
        })
    }

    /// Create a control frame containing a JSON-serialized IPC response.
    pub fn control_response(response: &IpcResponse) -> Result<Self, serde_json::Error> {
        let payload = serde_json::to_vec(response)?;
        Ok(Frame {
            msg_type: MSG_CONTROL,
            payload,
        })
    }

    /// Create a PTY output data frame.
    pub fn pty_output(session_id: &str, data: &[u8]) -> Self {
        let sid = session_id.as_bytes();
        let mut payload = Vec::with_capacity(1 + sid.len() + data.len());
        payload.push(sid.len() as u8);
        payload.extend_from_slice(sid);
        payload.extend_from_slice(data);
        Frame {
            msg_type: MSG_PTY_OUTPUT,
            payload,
        }
    }

    /// Create a key input data frame.
    pub fn key_input(session_id: &str, data: &[u8]) -> Self {
        let sid = session_id.as_bytes();
        let mut payload = Vec::with_capacity(1 + sid.len() + data.len());
        payload.push(sid.len() as u8);
        payload.extend_from_slice(sid);
        payload.extend_from_slice(data);
        Frame {
            msg_type: MSG_KEY_INPUT,
            payload,
        }
    }

    /// Create a snapshot data frame.
    pub fn snapshot(session_id: &str, data: &[u8]) -> Self {
        let sid = session_id.as_bytes();
        let mut payload = Vec::with_capacity(1 + sid.len() + data.len());
        payload.push(sid.len() as u8);
        payload.extend_from_slice(sid);
        payload.extend_from_slice(data);
        Frame {
            msg_type: MSG_SNAPSHOT,
            payload,
        }
    }

    /// Parse a session-prefixed payload (types 0x02, 0x03, 0x04).
    /// Returns `(session_id, data)`.
    pub fn parse_session_payload(&self) -> Option<(&str, &[u8])> {
        if self.payload.is_empty() {
            return None;
        }
        let sid_len = self.payload[0] as usize;
        if self.payload.len() < 1 + sid_len {
            return None;
        }
        let sid = std::str::from_utf8(&self.payload[1..1 + sid_len]).ok()?;
        let data = &self.payload[1 + sid_len..];
        Some((sid, data))
    }

    /// Deserialize the payload as a control request (type 0x01).
    pub fn as_control_request(&self) -> Result<IpcRequest, serde_json::Error> {
        serde_json::from_slice(&self.payload)
    }

    /// Deserialize the payload as a control response (type 0x01).
    pub fn as_control_response(&self) -> Result<IpcResponse, serde_json::Error> {
        serde_json::from_slice(&self.payload)
    }
}

// ---------------------------------------------------------------------------
// Async frame I/O
// ---------------------------------------------------------------------------

/// Write a frame to an async writer.
///
/// Wire format: `[4-byte big-endian length][1-byte type][payload]`
/// where `length = 1 + payload.len()` (covers the type byte + payload).
pub async fn write_frame<W: AsyncWrite + Unpin>(writer: &mut W, frame: &Frame) -> io::Result<()> {
    let length = 1u32 + frame.payload.len() as u32;
    writer.write_all(&length.to_be_bytes()).await?;
    writer.write_u8(frame.msg_type).await?;
    writer.write_all(&frame.payload).await?;
    writer.flush().await?;
    Ok(())
}

/// Read a frame from an async reader.
///
/// Returns `Err` with `UnexpectedEof` if the connection is closed cleanly.
pub async fn read_frame<R: AsyncRead + Unpin>(reader: &mut R) -> io::Result<Frame> {
    let mut len_buf = [0u8; 4];
    reader.read_exact(&mut len_buf).await?;
    let length = u32::from_be_bytes(len_buf);

    if length == 0 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "frame length is zero",
        ));
    }

    if length > MAX_FRAME_SIZE {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("frame too large: {length} bytes (max {MAX_FRAME_SIZE})"),
        ));
    }

    let msg_type = reader.read_u8().await?;
    let payload_len = (length - 1) as usize;
    let mut payload = vec![0u8; payload_len];
    if payload_len > 0 {
        reader.read_exact(&mut payload).await?;
    }

    Ok(Frame { msg_type, payload })
}

// ---------------------------------------------------------------------------
// Synchronous frame I/O (for CLI / blocking contexts)
// ---------------------------------------------------------------------------

/// Write a frame to a synchronous writer.
pub fn write_frame_sync<W: std::io::Write>(writer: &mut W, frame: &Frame) -> io::Result<()> {
    let length = 1u32 + frame.payload.len() as u32;
    writer.write_all(&length.to_be_bytes())?;
    writer.write_all(&[frame.msg_type])?;
    writer.write_all(&frame.payload)?;
    writer.flush()?;
    Ok(())
}

/// Read a frame from a synchronous reader.
pub fn read_frame_sync<R: std::io::Read>(reader: &mut R) -> io::Result<Frame> {
    let mut len_buf = [0u8; 4];
    reader.read_exact(&mut len_buf)?;
    let length = u32::from_be_bytes(len_buf);

    if length == 0 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "frame length is zero",
        ));
    }

    if length > MAX_FRAME_SIZE {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("frame too large: {length} bytes (max {MAX_FRAME_SIZE})"),
        ));
    }

    let mut type_buf = [0u8; 1];
    reader.read_exact(&mut type_buf)?;
    let msg_type = type_buf[0];

    let payload_len = (length - 1) as usize;
    let mut payload = vec![0u8; payload_len];
    if payload_len > 0 {
        reader.read_exact(&mut payload)?;
    }

    Ok(Frame { msg_type, payload })
}

// ---------------------------------------------------------------------------
// IPC Request / Response (JSON control messages)
// ---------------------------------------------------------------------------

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
        #[serde(default)]
        cols: Option<u16>,
        #[serde(default)]
        rows: Option<u16>,
    },

    /// Attach to an existing session (receive PTY output, send input).
    AttachSession { id: String },

    /// Detach from a session (stop receiving PTY output; shell survives).
    DetachSession { id: String },

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

/// Session information returned by `ListSessions` / `ListSessionDetails`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionInfo {
    pub id: String,
    pub name: String,
    pub shell: String,
    pub cwd: String,
    pub pid: Option<i32>,
    pub cols: u16,
    pub rows: u16,
    pub created_at: String,
    pub attached: bool,
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

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
            cols: Some(120),
            rows: Some(40),
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
            cols: None,
            rows: None,
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
        let req: IpcRequest =
            serde_json::from_str(r#"{"type":"claude_status_update","state":"done"}"#).unwrap();
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
                cols: None,
                rows: None,
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
            IpcRequest::AttachSession {
                id: "test-id".to_string(),
            },
            IpcRequest::DetachSession {
                id: "test-id".to_string(),
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

    // --- Frame tests ---

    #[test]
    fn test_frame_control_roundtrip() {
        let req = IpcRequest::Ping;
        let frame = Frame::control_request(&req).unwrap();
        assert_eq!(frame.msg_type, MSG_CONTROL);
        let decoded: IpcRequest = frame.as_control_request().unwrap();
        assert_eq!(decoded, req);
    }

    #[test]
    fn test_frame_pty_output() {
        let data = b"hello world";
        let frame = Frame::pty_output("sess-123", data);
        assert_eq!(frame.msg_type, MSG_PTY_OUTPUT);
        let (sid, payload) = frame.parse_session_payload().unwrap();
        assert_eq!(sid, "sess-123");
        assert_eq!(payload, data);
    }

    #[test]
    fn test_frame_key_input() {
        let data = b"\x1b[A"; // Up arrow
        let frame = Frame::key_input("sess-456", data);
        assert_eq!(frame.msg_type, MSG_KEY_INPUT);
        let (sid, payload) = frame.parse_session_payload().unwrap();
        assert_eq!(sid, "sess-456");
        assert_eq!(payload, data);
    }

    #[test]
    fn test_frame_snapshot() {
        let data = b"{\"grid_cells\":[]}";
        let frame = Frame::snapshot("sess-789", data);
        assert_eq!(frame.msg_type, MSG_SNAPSHOT);
        let (sid, payload) = frame.parse_session_payload().unwrap();
        assert_eq!(sid, "sess-789");
        assert_eq!(payload, data);
    }

    #[test]
    fn test_frame_empty_session_payload() {
        let frame = Frame {
            msg_type: MSG_PTY_OUTPUT,
            payload: vec![],
        };
        assert!(frame.parse_session_payload().is_none());
    }

    #[tokio::test]
    async fn test_frame_async_roundtrip() {
        let frame = Frame::pty_output("test-session", b"hello");
        let mut buf: Vec<u8> = Vec::new();
        write_frame(&mut buf, &frame).await.unwrap();

        let mut cursor = io::Cursor::new(buf);
        let decoded = read_frame(&mut cursor).await.unwrap();
        assert_eq!(decoded.msg_type, MSG_PTY_OUTPUT);
        let (sid, data) = decoded.parse_session_payload().unwrap();
        assert_eq!(sid, "test-session");
        assert_eq!(data, b"hello");
    }

    #[test]
    fn test_frame_sync_roundtrip() {
        let frame = Frame::key_input("sync-session", b"typed");
        let mut buf: Vec<u8> = Vec::new();
        write_frame_sync(&mut buf, &frame).unwrap();

        let mut cursor = io::Cursor::new(buf);
        let decoded = read_frame_sync(&mut cursor).unwrap();
        assert_eq!(decoded.msg_type, MSG_KEY_INPUT);
        let (sid, data) = decoded.parse_session_payload().unwrap();
        assert_eq!(sid, "sync-session");
        assert_eq!(data, b"typed");
    }

    #[tokio::test]
    async fn test_frame_max_size_rejection() {
        let bad_length: u32 = MAX_FRAME_SIZE + 1;
        let mut buf = Vec::new();
        buf.extend_from_slice(&bad_length.to_be_bytes());
        buf.push(MSG_CONTROL);
        buf.extend_from_slice(&vec![0u8; 16]);

        let mut cursor = io::Cursor::new(buf);
        let result = read_frame(&mut cursor).await;
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidData);
    }

    #[tokio::test]
    async fn test_multiple_frames_on_stream() {
        let frames = vec![
            Frame::control_request(&IpcRequest::Ping).unwrap(),
            Frame::pty_output("s1", b"data1"),
            Frame::key_input("s2", b"input2"),
        ];

        let mut buf: Vec<u8> = Vec::new();
        for f in &frames {
            write_frame(&mut buf, f).await.unwrap();
        }

        let mut cursor = io::Cursor::new(buf);
        for expected in &frames {
            let decoded = read_frame(&mut cursor).await.unwrap();
            assert_eq!(decoded.msg_type, expected.msg_type);
            assert_eq!(decoded.payload, expected.payload);
        }
    }
}
