//! termojinald daemon -- manages sessions and listens for connections.
//!
//! Supports two connection protocols:
//! - **Legacy JSON**: Line-delimited JSON (first byte is `{`). Used by `tm` CLI.
//! - **Binary frame**: 4-byte length prefix (first byte is NOT `{`). Used by GUI.

use crate::{ClientMessage, ClientSender, SessionError, SessionManager};
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::UnixListener;
use tokio::sync::Mutex;

// ---------------------------------------------------------------------------
// Inline binary frame protocol (avoids circular dependency with termojinal-ipc)
// ---------------------------------------------------------------------------

/// Frame type constants.
const MSG_CONTROL: u8 = 0x01;
const MSG_PTY_OUTPUT: u8 = 0x02;
const MSG_KEY_INPUT: u8 = 0x03;
const MSG_SNAPSHOT: u8 = 0x04;
const MAX_FRAME_SIZE: u32 = 16 * 1024 * 1024;

/// A single binary frame on the wire.
struct Frame {
    msg_type: u8,
    payload: Vec<u8>,
}

impl Frame {
    fn pty_output(session_id: &str, data: &[u8]) -> Self {
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

    fn snapshot(session_id: &str, data: &[u8]) -> Self {
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

    fn control_response(response: &serde_json::Value) -> Self {
        let payload = serde_json::to_vec(response).unwrap_or_default();
        Frame {
            msg_type: MSG_CONTROL,
            payload,
        }
    }

    fn parse_session_payload(&self) -> Option<(&str, &[u8])> {
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
}

async fn write_frame<W: AsyncWriteExt + Unpin>(
    writer: &mut W,
    frame: &Frame,
) -> std::io::Result<()> {
    let length = 1u32 + frame.payload.len() as u32;
    writer.write_all(&length.to_be_bytes()).await?;
    writer.write_u8(frame.msg_type).await?;
    writer.write_all(&frame.payload).await?;
    writer.flush().await?;
    Ok(())
}

async fn read_frame<R: AsyncReadExt + Unpin>(reader: &mut R) -> std::io::Result<Frame> {
    let mut len_buf = [0u8; 4];
    reader.read_exact(&mut len_buf).await?;
    let length = u32::from_be_bytes(len_buf);
    if length == 0 || length > MAX_FRAME_SIZE {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "invalid frame",
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
// Daemon
// ---------------------------------------------------------------------------

/// The daemon state.
pub struct Daemon {
    manager: Arc<Mutex<SessionManager>>,
    socket_path: String,
}

impl Daemon {
    pub fn new() -> Result<Self, SessionError> {
        let manager = SessionManager::new()?;
        let socket_path = socket_path();

        Ok(Self {
            manager: Arc::new(Mutex::new(manager)),
            socket_path,
        })
    }

    /// Run the daemon event loop.
    pub async fn run(&self) -> Result<(), SessionError> {
        // Clean up any stale socket file.
        if std::path::Path::new(&self.socket_path).exists() {
            std::fs::remove_file(&self.socket_path).ok();
        }

        if let Some(parent) = std::path::Path::new(&self.socket_path).parent() {
            std::fs::create_dir_all(parent)?;
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(parent, std::fs::Permissions::from_mode(0o700))?;
        }

        let listener = UnixListener::bind(&self.socket_path).map_err(SessionError::Io)?;

        // Restrict socket file permissions to owner only.
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(
                &self.socket_path,
                std::fs::Permissions::from_mode(0o600),
            )?;
        }

        log::info!("termojinald listening on {}", self.socket_path);

        // --- Clean up stale session files from previous daemon runs ---
        // True session restoration (reattaching to an existing PTY) is not
        // possible after a daemon restart because the kernel closes the master
        // fd.  Instead we just remove stale persistence files so they don't
        // accumulate.
        {
            let manager = self.manager.lock().await;

            match manager.load_saved_states() {
                Ok(states) => {
                    for s in &states {
                        log::info!(
                            "cleaning up stale session file: {} (pid={:?})",
                            s.name,
                            s.pid
                        );
                        manager.remove_saved(&s.id).ok();
                    }
                    if !states.is_empty() {
                        log::info!("removed {} stale session files", states.len());
                    }
                }
                Err(e) => log::warn!("failed to load saved sessions: {e}"),
            }
        }

        // --- Periodically reap dead sessions ---
        let manager = self.manager.clone();
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(std::time::Duration::from_secs(5));
            loop {
                interval.tick().await;
                let mut mgr = manager.lock().await;
                let dead = mgr.reap_dead();
                for id in &dead {
                    log::info!("reaped dead session: {id}");
                }
            }
        });

        // Accept connections.
        loop {
            match listener.accept().await {
                Ok((stream, _addr)) => {
                    let manager = self.manager.clone();
                    tokio::spawn(async move {
                        if let Err(e) = handle_connection(stream, manager).await {
                            log::error!("connection error: {e}");
                        }
                    });
                }
                Err(e) => log::error!("accept error: {e}"),
            }
        }
    }

    pub fn manager(&self) -> &Arc<Mutex<SessionManager>> {
        &self.manager
    }
}

impl Drop for Daemon {
    fn drop(&mut self) {
        std::fs::remove_file(&self.socket_path).ok();
    }
}

/// Handle a single IPC connection.
///
/// Peeks the first byte to determine the protocol:
/// - `{` (0x7B): Legacy JSON line protocol
/// - Anything else: Binary frame protocol
async fn handle_connection(
    stream: tokio::net::UnixStream,
    manager: Arc<Mutex<SessionManager>>,
) -> Result<(), SessionError> {
    // Read the first byte.
    stream.readable().await.map_err(SessionError::Io)?;

    let mut peek = [0u8; 1];
    let n = match stream.try_read(&mut peek) {
        Ok(n) => n,
        Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
            stream.readable().await.map_err(SessionError::Io)?;
            stream.try_read(&mut peek).map_err(SessionError::Io)?
        }
        Err(e) => return Err(SessionError::Io(e)),
    };

    if n == 0 {
        return Ok(());
    }

    if peek[0] == b'{' {
        handle_json_connection(stream, manager, peek[0]).await
    } else {
        handle_binary_connection(stream, manager, peek[0]).await
    }
}

/// Handle a legacy JSON line protocol connection.
async fn handle_json_connection(
    stream: tokio::net::UnixStream,
    manager: Arc<Mutex<SessionManager>>,
    first_byte: u8,
) -> Result<(), SessionError> {
    use serde_json::json;
    use tokio::io::AsyncBufReadExt;
    use tokio::io::BufReader;

    let (reader, mut writer) = stream.into_split();
    let mut buf_reader = BufReader::new(reader);
    let mut line = String::new();
    line.push(first_byte as char);

    let n = buf_reader
        .read_line(&mut line)
        .await
        .map_err(SessionError::Io)?;
    if n == 0 && line.len() <= 1 {
        return Ok(());
    }

    let trimmed = line.trim();
    log::debug!("IPC request (JSON): {trimmed}");

    let response = match serde_json::from_str::<serde_json::Value>(trimmed) {
        Ok(req) => {
            let req_type = req.get("type").and_then(|v| v.as_str()).unwrap_or("");
            match req_type {
                "ping" => json!({"success": true, "data": {"status": "pong"}}),
                "list_sessions" => {
                    let mgr = manager.lock().await;
                    let ids: Vec<String> = mgr.list().into_iter().map(|s| s.to_string()).collect();
                    json!({"success": true, "data": {"sessions": ids}})
                }
                "list_session_details" => {
                    let mut mgr = manager.lock().await;
                    let dead = mgr.reap_dead();
                    if !dead.is_empty() {
                        log::info!("reaped {} dead session(s) during list", dead.len());
                    }
                    let details: Vec<serde_json::Value> = mgr
                        .list_details_extended()
                        .iter()
                        .map(|(s, attached, title, pwd)| {
                            json!({
                                "id": s.id, "name": s.name, "shell": s.shell,
                                "cwd": s.cwd, "pid": s.pid, "cols": s.cols,
                                "rows": s.rows, "created_at": s.created_at.to_rfc3339(),
                                "attached": attached,
                                "workspace_name": s.workspace_name,
                                "title": title,
                                "pwd": pwd,
                            })
                        })
                        .collect();
                    json!({"success": true, "data": {"sessions": details}})
                }
                "create_session" => {
                    let shell = req
                        .get("shell")
                        .and_then(|v| v.as_str())
                        .unwrap_or(
                            &std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".to_string()),
                        )
                        .to_string();
                    let cwd = req
                        .get("cwd")
                        .and_then(|v| v.as_str())
                        .unwrap_or(".")
                        .to_string();
                    let cols = req.get("cols").and_then(|v| v.as_u64()).unwrap_or(80) as u16;
                    let rows = req.get("rows").and_then(|v| v.as_u64()).unwrap_or(24) as u16;
                    let mut mgr = manager.lock().await;
                    match mgr.create_session_with_manager(&shell, &cwd, cols, rows, Some(manager.clone())) {
                        Ok(session) => json!({
                            "success": true,
                            "data": {"id": session.state.id, "name": session.state.name, "pid": session.state.pid}
                        }),
                        Err(e) => json!({"success": false, "error": format!("{e}")}),
                    }
                }
                "kill_session" => {
                    let id = req.get("id").and_then(|v| v.as_str()).unwrap_or("");
                    let mut mgr = manager.lock().await;
                    match mgr.remove(id) {
                        Ok(()) => json!({"success": true}),
                        Err(e) => json!({"success": false, "error": format!("{e}")}),
                    }
                }
                "resize_session" => {
                    let id = req
                        .get("id")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string();
                    let cols = req.get("cols").and_then(|v| v.as_u64()).unwrap_or(80) as u16;
                    let rows = req.get("rows").and_then(|v| v.as_u64()).unwrap_or(24) as u16;
                    let mut mgr = manager.lock().await;
                    match mgr.resize_session(&id, cols, rows).await {
                        Ok(()) => json!({"success": true}),
                        Err(e) => json!({"success": false, "error": format!("{e}")}),
                    }
                }
                "register_session" => {
                    let pane_id = req.get("pane_id").and_then(|v| v.as_u64()).unwrap_or(0);
                    let pid = req.get("pid").and_then(|v| v.as_i64()).unwrap_or(0) as i32;
                    if pid <= 0 {
                        json!({"success": false, "error": "invalid pid"})
                    } else {
                        let shell = req
                            .get("shell")
                            .and_then(|v| v.as_str())
                            .unwrap_or("/bin/sh")
                            .to_string();
                        let cwd = req
                            .get("cwd")
                            .and_then(|v| v.as_str())
                            .unwrap_or(".")
                            .to_string();
                        let cols = req.get("cols").and_then(|v| v.as_u64()).unwrap_or(80) as u16;
                        let rows = req.get("rows").and_then(|v| v.as_u64()).unwrap_or(24) as u16;
                        let mut mgr = manager.lock().await;
                        let id =
                            mgr.register_external_session(pane_id, pid, &shell, &cwd, cols, rows);
                        json!({"success": true, "data": {"id": id, "pane_id": pane_id}})
                    }
                }
                "unregister_session" => {
                    let pane_id = req.get("pane_id").and_then(|v| v.as_u64()).unwrap_or(0);
                    let mut mgr = manager.lock().await;
                    let removed = mgr.unregister_external_session(pane_id);
                    json!({"success": true, "data": {"removed": removed}})
                }
                "exit_session" => {
                    let id = req.get("id").and_then(|v| v.as_str()).unwrap_or("");
                    let force = req.get("force").and_then(|v| v.as_bool()).unwrap_or(false);
                    let mut mgr = manager.lock().await;
                    if force {
                        match mgr.force_exit_session(id) {
                            Ok(()) => {
                                log::info!("force-exited session: {id}");
                                json!({"success": true})
                            }
                            Err(e) => json!({"success": false, "error": format!("{e}")}),
                        }
                    } else {
                        match mgr.exit_session(id) {
                            Ok(None) => {
                                log::info!("exited session: {id}");
                                json!({"success": true})
                            }
                            Ok(Some(proc_name)) => {
                                log::info!("session {id} has running process: {proc_name}");
                                json!({"success": true, "data": {"running_process": proc_name}})
                            }
                            Err(e) => json!({"success": false, "error": format!("{e}")}),
                        }
                    }
                }
                "kill_all" => {
                    let mut mgr = manager.lock().await;
                    let count = mgr.kill_all();
                    log::info!("killed all {count} sessions");
                    json!({"success": true, "data": {"killed": count}})
                }
                "update_session_workspace" => {
                    let id = req.get("id").and_then(|v| v.as_str()).unwrap_or("");
                    let workspace_name = req
                        .get("workspace_name")
                        .and_then(|v| v.as_str())
                        .unwrap_or("");
                    let mut mgr = manager.lock().await;
                    match mgr.update_session_workspace(id, workspace_name) {
                        Ok(()) => json!({"success": true}),
                        Err(e) => json!({"success": false, "error": format!("{e}")}),
                    }
                }
                "claude_status_update" => {
                    // The daemon's own listener acknowledges the request.
                    // In the full architecture the GUI app handles these via
                    // the IpcServer callback; here we just log and ACK.
                    let state = req
                        .get("state")
                        .and_then(|v| v.as_str())
                        .unwrap_or("unknown");
                    let pid = req.get("pid").and_then(|v| v.as_i64());
                    log::info!("claude status update (daemon): state={state}, pid={pid:?}");
                    json!({"success": true})
                }
                _ => {
                    json!({"success": false, "error": format!("unknown request type: {req_type}")})
                }
            }
        }
        Err(e) => json!({"success": false, "error": format!("invalid JSON: {e}")}),
    };

    let mut response_json = serde_json::to_string(&response)
        .map_err(|e| SessionError::Io(std::io::Error::new(std::io::ErrorKind::Other, e)))?;
    response_json.push('\n');
    writer
        .write_all(response_json.as_bytes())
        .await
        .map_err(SessionError::Io)?;

    Ok(())
}

/// Handle a binary frame protocol connection (GUI streaming).
async fn handle_binary_connection(
    stream: tokio::net::UnixStream,
    manager: Arc<Mutex<SessionManager>>,
    first_byte: u8,
) -> Result<(), SessionError> {
    use serde_json::json;

    let (mut reader, mut writer) = stream.into_split();

    // Read the remaining 3 bytes of the first frame's length header.
    let mut rest = [0u8; 3];
    reader
        .read_exact(&mut rest)
        .await
        .map_err(SessionError::Io)?;
    let length = u32::from_be_bytes([first_byte, rest[0], rest[1], rest[2]]);

    if length == 0 || length > MAX_FRAME_SIZE {
        return Err(SessionError::Io(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("invalid frame length: {length}"),
        )));
    }

    let msg_type = reader.read_u8().await.map_err(SessionError::Io)?;
    let payload_len = (length - 1) as usize;
    let mut payload = vec![0u8; payload_len];
    if payload_len > 0 {
        reader
            .read_exact(&mut payload)
            .await
            .map_err(SessionError::Io)?;
    }

    let first_frame = Frame { msg_type, payload };

    // The first frame must be a control message with attach_session.
    if first_frame.msg_type != MSG_CONTROL {
        log::warn!(
            "binary connection: first frame is not MSG_CONTROL (got type={})",
            first_frame.msg_type
        );
        return Ok(());
    }

    let request: serde_json::Value =
        serde_json::from_slice(&first_frame.payload).map_err(SessionError::Serialize)?;

    let req_type = request.get("type").and_then(|v| v.as_str()).unwrap_or("");
    let session_id = if req_type == "attach_session" {
        request
            .get("id")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string()
    } else {
        // One-shot control request over binary protocol.
        let resp =
            json!({"success": false, "error": "expected attach_session as first binary frame"});
        let resp_frame = Frame::control_response(&resp);
        let _ = write_frame(&mut writer, &resp_frame).await;
        return Ok(());
    };

    if session_id.is_empty() {
        let resp = json!({"success": false, "error": "missing session id"});
        let resp_frame = Frame::control_response(&resp);
        let _ = write_frame(&mut writer, &resp_frame).await;
        return Ok(());
    }

    log::info!("binary attach: session_id={session_id}");

    // Create a client sender for this connection.
    static CLIENT_ID_COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
    let client_id = CLIENT_ID_COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);

    let (client_tx, mut client_rx) = tokio::sync::mpsc::channel::<ClientMessage>(1024);
    let client = ClientSender::new(client_id, client_tx);

    // Attach first, then send snapshot. This avoids a race where PTY output
    // arrives between snapshot and attach, causing the client to miss data.
    {
        let mgr = manager.lock().await;
        if let Err(e) = mgr.attach_session(&session_id, client) {
            let resp = json!({"success": false, "error": format!("attach failed: {e}")});
            let resp_frame = Frame::control_response(&resp);
            let _ = write_frame(&mut writer, &resp_frame).await;
            return Ok(());
        }
        // Send snapshot after attach so the client sees a consistent view.
        // Any PTY output that arrives between attach and snapshot delivery
        // will be queued in the client's channel and delivered after the snapshot.
        if let Some(snapshot) = mgr.get_snapshot(&session_id) {
            let snapshot_bytes = serde_json::to_vec(&snapshot).unwrap_or_default();
            let snap_frame = Frame::snapshot(&session_id, &snapshot_bytes);
            if write_frame(&mut writer, &snap_frame).await.is_err() {
                let _ = mgr.detach_session(&session_id, client_id);
                return Ok(());
            }
        }
    }

    // Send OK response.
    {
        let resp = json!({"success": true, "data": {"attached": true, "session_id": &session_id}});
        let resp_frame = Frame::control_response(&resp);
        if write_frame(&mut writer, &resp_frame).await.is_err() {
            let mgr = manager.lock().await;
            let _ = mgr.detach_session(&session_id, client_id);
            return Ok(());
        }
    }

    // Streaming loop.
    loop {
        tokio::select! {
            msg = client_rx.recv() => {
                match msg {
                    Some(ClientMessage::PtyOutput { session_id: sid, data }) => {
                        let frame = Frame::pty_output(&sid, &data);
                        if write_frame(&mut writer, &frame).await.is_err() {
                            break;
                        }
                    }
                    Some(ClientMessage::SessionExited { session_id: sid, exit_code }) => {
                        let resp = json!({
                            "success": true,
                            "data": {"event": "session_exited", "session_id": sid, "exit_code": exit_code}
                        });
                        let frame = Frame::control_response(&resp);
                        let _ = write_frame(&mut writer, &frame).await;
                        break;
                    }
                    None => break,
                }
            }

            frame_result = read_frame(&mut reader) => {
                match frame_result {
                    Ok(frame) => {
                        match frame.msg_type {
                            MSG_KEY_INPUT => {
                                if let Some((_sid, data)) = frame.parse_session_payload() {
                                    let mgr = manager.lock().await;
                                    let _ = mgr.send_input(&session_id, data.to_vec()).await;
                                }
                            }
                            MSG_CONTROL => {
                                if let Ok(ctrl_req) = serde_json::from_slice::<serde_json::Value>(&frame.payload) {
                                    let ctrl_type = ctrl_req.get("type").and_then(|v| v.as_str()).unwrap_or("");
                                    match ctrl_type {
                                        "detach_session" => break,
                                        "resize_session" => {
                                            let id = ctrl_req.get("id").and_then(|v| v.as_str()).unwrap_or("").to_string();
                                            let cols = ctrl_req.get("cols").and_then(|v| v.as_u64()).unwrap_or(80) as u16;
                                            let rows = ctrl_req.get("rows").and_then(|v| v.as_u64()).unwrap_or(24) as u16;
                                            let mut mgr = manager.lock().await;
                                            let _ = mgr.resize_session(&id, cols, rows).await;
                                        }
                                        _ => {}
                                    }
                                }
                            }
                            _ => {}
                        }
                    }
                    Err(_) => break,
                }
            }
        }
    }

    log::info!("binary detach: session_id={session_id}, client_id={client_id}");
    let mgr = manager.lock().await;
    let _ = mgr.detach_session(&session_id, client_id);

    Ok(())
}

/// Get the Unix socket path for termojinald.
pub fn socket_path() -> String {
    let runtime_dir = dirs::runtime_dir()
        .or_else(|| dirs::data_local_dir())
        .unwrap_or_else(|| std::path::PathBuf::from("/tmp"));
    runtime_dir
        .join("termojinal")
        .join("termojinald.sock")
        .to_string_lossy()
        .to_string()
}

/// Get the Unix socket path for the termojinal app's IPC listener.
pub fn app_socket_path() -> String {
    let data_dir = dirs::data_local_dir().unwrap_or_else(|| std::path::PathBuf::from("/tmp"));
    data_dir
        .join("termojinal")
        .join("termojinal-app.sock")
        .to_string_lossy()
        .to_string()
}
