//! GUI-side connection to the termojinald daemon.
//!
//! Provides a synchronous `DaemonHandle` for the GUI thread to communicate
//! with the daemon, and a `daemon_reader_thread` function that connects to
//! the daemon via binary framing and streams PTY output.

use crate::protocol::{
    read_frame_sync, write_frame_sync, Frame, MSG_CONTROL, MSG_PTY_OUTPUT, MSG_SNAPSHOT,
};

use std::collections::HashMap;
use std::sync::Mutex;

/// Messages sent through the persistent binary connection.
pub enum WriteCommand {
    /// Raw key input data.
    KeyInput(Vec<u8>),
    /// Resize request (cols, rows).
    Resize(u16, u16),
}

/// Global registry of session write channels.
/// daemon_reader_thread registers its write_tx here; daemon_pty_write uses it.
static WRITE_CHANNELS: std::sync::LazyLock<Mutex<HashMap<String, std::sync::mpsc::Sender<WriteCommand>>>> =
    std::sync::LazyLock::new(|| Mutex::new(HashMap::new()));

/// Register a write channel for a session (called by daemon_reader_thread).
pub fn register_write_channel(session_id: &str, tx: std::sync::mpsc::Sender<WriteCommand>) {
    if let Ok(mut map) = WRITE_CHANNELS.lock() {
        map.insert(session_id.to_string(), tx);
    }
}

/// Unregister a write channel (called when session exits).
pub fn unregister_write_channel(session_id: &str) {
    if let Ok(mut map) = WRITE_CHANNELS.lock() {
        map.remove(session_id);
    }
}

/// Synchronous handle for communicating with the termojinald daemon.
///
/// The GUI thread uses this to send key input, resize, and other control
/// messages. PTY output is received by per-pane background reader threads
/// that connect to the daemon independently via `daemon_reader_thread`.
pub struct DaemonHandle {
    socket_path: String,
}

impl DaemonHandle {
    pub fn new() -> Self {
        Self {
            socket_path: termojinal_session::daemon::socket_path(),
        }
    }

    /// Send a JSON request to the daemon and return the response.
    pub fn send_request_json(&self, req: &serde_json::Value) -> Option<serde_json::Value> {
        use std::io::{BufRead, Read, Write};
        use std::os::unix::net::UnixStream;

        let mut stream = UnixStream::connect(&self.socket_path).ok()?;
        stream
            .set_read_timeout(Some(std::time::Duration::from_secs(5)))
            .ok();
        let msg = format!("{}\n", req);
        stream.write_all(msg.as_bytes()).ok()?;
        let mut line = String::new();
        std::io::BufReader::new((&stream).take(1_048_576))
            .read_line(&mut line)
            .ok()?;
        serde_json::from_str(line.trim()).ok()
    }

    /// Create a session on the daemon. Returns (session_id, name, pid).
    pub fn create_session(
        &self,
        shell: &str,
        cwd: &str,
        cols: u16,
        rows: u16,
    ) -> Option<(String, String, i32)> {
        let req = serde_json::json!({
            "type": "create_session",
            "shell": shell,
            "cwd": cwd,
            "cols": cols,
            "rows": rows,
        });
        let resp = self.send_request_json(&req)?;
        if resp.get("success")?.as_bool()? {
            let data = resp.get("data")?;
            let id = data.get("id")?.as_str()?.to_string();
            let name = data.get("name")?.as_str()?.to_string();
            let pid = data.get("pid")?.as_i64().map(|v| v as i32).unwrap_or(0);
            Some((id, name, pid))
        } else {
            None
        }
    }

    /// Resize a session via the daemon.
    pub fn resize_session(&self, session_id: &str, cols: u16, rows: u16) {
        let req = serde_json::json!({
            "type": "resize_session",
            "id": session_id,
            "cols": cols,
            "rows": rows,
        });
        self.send_request_json(&req);
    }

    /// List all sessions with details from the daemon.
    /// Returns a list of (session_id, name, shell, cwd, pid, cols, rows, attached, workspace_name).
    #[allow(clippy::type_complexity)]
    pub fn list_session_details(
        &self,
    ) -> Vec<(String, String, String, String, i32, u16, u16, bool, Option<String>)> {
        let req = serde_json::json!({"type": "list_session_details"});
        let resp = match self.send_request_json(&req) {
            Some(r) => r,
            None => return Vec::new(),
        };
        let sessions = match resp
            .get("data")
            .and_then(|d| d.get("sessions"))
            .and_then(|s| s.as_array())
        {
            Some(s) => s,
            None => return Vec::new(),
        };
        sessions
            .iter()
            .filter_map(|s| {
                let id = s.get("id")?.as_str()?.to_string();
                let name = s
                    .get("name")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                let shell = s
                    .get("shell")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                let cwd = s
                    .get("cwd")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                let pid = s.get("pid").and_then(|v| v.as_i64()).unwrap_or(0) as i32;
                let cols = s.get("cols").and_then(|v| v.as_u64()).unwrap_or(80) as u16;
                let rows = s.get("rows").and_then(|v| v.as_u64()).unwrap_or(24) as u16;
                let attached = s.get("attached").and_then(|v| v.as_bool()).unwrap_or(false);
                let workspace_name = s
                    .get("workspace_name")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string());
                Some((id, name, shell, cwd, pid, cols, rows, attached, workspace_name))
            })
            .collect()
    }

    /// Update a session's workspace name on the daemon.
    pub fn update_session_workspace(&self, session_id: &str, workspace_name: &str) {
        let req = serde_json::json!({
            "type": "update_session_workspace",
            "id": session_id,
            "workspace_name": workspace_name,
        });
        self.send_request_json(&req);
    }

    /// Kill a session via the daemon.
    pub fn kill_session(&self, session_id: &str) {
        let req = serde_json::json!({
            "type": "kill_session",
            "id": session_id,
        });
        self.send_request_json(&req);
    }
}

/// Write data to a pane's PTY via the daemon.
/// Uses the persistent connection's write channel registered by daemon_reader_thread.
pub fn daemon_pty_write(session_id: &str, data: &[u8]) {
    if let Ok(map) = WRITE_CHANNELS.lock() {
        if let Some(tx) = map.get(session_id) {
            let _ = tx.send(WriteCommand::KeyInput(data.to_vec()));
        }
    }
}

/// Resize a pane's PTY via the daemon persistent binary connection.
pub fn daemon_pty_resize(session_id: &str, cols: u16, rows: u16) {
    if let Ok(map) = WRITE_CHANNELS.lock() {
        if let Some(tx) = map.get(session_id) {
            let _ = tx.send(WriteCommand::Resize(cols, rows));
        }
    }
}

/// Background thread function that connects to the daemon via binary framing,
/// sends `AttachSession`, and reads PTY output frames.
///
/// Output data is pushed into the shared `buffers` map and the GUI is notified
/// via the `proxy`. When the session exits, `PtyExited` is sent.
///
/// The function also provides a `write_tx` channel for sending key input to
/// the daemon through the same persistent connection.
pub fn daemon_reader_thread(
    pane_id: u64,
    session_id: &str,
    socket_path: &str,
    proxy: impl Fn(DaemonReaderEvent) + Send + 'static,
    write_rx: std::sync::mpsc::Receiver<WriteCommand>,
) {
    use std::os::unix::net::UnixStream;

    // Connect to daemon.
    let mut stream = match UnixStream::connect(socket_path) {
        Ok(s) => s,
        Err(e) => {
            log::error!("pane {pane_id}: failed to connect to daemon: {e}");
            proxy(DaemonReaderEvent::Exited);
            return;
        }
    };

    // Send AttachSession control frame (binary protocol).
    let attach_req = serde_json::json!({"type": "attach_session", "id": session_id});
    let attach_payload = serde_json::to_vec(&attach_req).unwrap_or_default();
    let attach_frame = Frame {
        msg_type: MSG_CONTROL,
        payload: attach_payload,
    };
    if write_frame_sync(&mut stream, &attach_frame).is_err() {
        log::error!("pane {pane_id}: failed to send attach frame");
        proxy(DaemonReaderEvent::Exited);
        return;
    }

    // Read the attach response and verify success.
    // Use a timeout for the initial handshake to avoid hanging forever.
    stream
        .set_read_timeout(Some(std::time::Duration::from_secs(5)))
        .ok();
    let mut attach_ok = false;
    // Read frames until we get the control response (skip any snapshot frames).
    loop {
        match read_frame_sync(&mut stream) {
            Ok(frame) if frame.msg_type == MSG_SNAPSHOT => {
                // Snapshot arrives before the success response; forward it and continue.
                if let Some((_sid, data)) = frame.parse_session_payload() {
                    proxy(DaemonReaderEvent::Snapshot(data.to_vec()));
                }
            }
            Ok(frame) if frame.msg_type == MSG_CONTROL => {
                if let Ok(resp) = serde_json::from_slice::<serde_json::Value>(&frame.payload) {
                    if resp.get("success").and_then(|v| v.as_bool()) == Some(true) {
                        attach_ok = true;
                    } else {
                        let err = resp
                            .get("error")
                            .and_then(|v| v.as_str())
                            .unwrap_or("unknown error");
                        log::error!("pane {pane_id}: attach failed: {err}");
                    }
                }
                break;
            }
            Ok(_) => {
                // Unexpected frame type during handshake; ignore and continue.
            }
            Err(e) => {
                log::error!("pane {pane_id}: failed to read attach response: {e}");
                break;
            }
        }
    }
    if !attach_ok {
        proxy(DaemonReaderEvent::Exited);
        return;
    }

    // Clone the stream for the writer thread.
    let write_stream = match stream.try_clone() {
        Ok(s) => s,
        Err(e) => {
            log::error!("pane {pane_id}: failed to clone stream: {e}");
            proxy(DaemonReaderEvent::Exited);
            return;
        }
    };

    // Writer thread uses a cloned stream, so the reader can block indefinitely.
    stream.set_read_timeout(None).ok();

    let write_stream = std::sync::Arc::new(std::sync::Mutex::new(write_stream));

    // Spawn a writer thread that forwards key input from the GUI to the daemon.
    let sid = session_id.to_string();
    let ws = write_stream.clone();
    let write_stream_for_shutdown = write_stream;
    std::thread::Builder::new()
        .name(format!("daemon-writer-{pane_id}"))
        .spawn(move || {
            while let Ok(cmd) = write_rx.recv() {
                let frame = match cmd {
                    WriteCommand::KeyInput(data) => Frame::key_input(&sid, &data),
                    WriteCommand::Resize(cols, rows) => {
                        let req = serde_json::json!({
                            "type": "resize_session",
                            "id": &sid,
                            "cols": cols,
                            "rows": rows,
                        });
                        Frame {
                            msg_type: MSG_CONTROL,
                            payload: serde_json::to_vec(&req).unwrap_or_default(),
                        }
                    }
                };
                if let Ok(mut w) = ws.lock() {
                    if write_frame_sync(&mut *w, &frame).is_err() {
                        break;
                    }
                }
            }
        })
        .ok();

    // Read loop: read frames from the daemon.
    loop {
        match read_frame_sync(&mut stream) {
            Ok(frame) => {
                match frame.msg_type {
                    MSG_PTY_OUTPUT => {
                        if let Some((_sid, data)) = frame.parse_session_payload() {
                            proxy(DaemonReaderEvent::Output(data.to_vec()));
                        }
                    }
                    MSG_SNAPSHOT => {
                        if let Some((_sid, data)) = frame.parse_session_payload() {
                            proxy(DaemonReaderEvent::Snapshot(data.to_vec()));
                        }
                    }
                    MSG_CONTROL => {
                        // Check for session_exited event.
                        if let Ok(resp) =
                            serde_json::from_slice::<serde_json::Value>(&frame.payload)
                        {
                            if let Some(data) = resp.get("data") {
                                if data.get("event").and_then(|v| v.as_str())
                                    == Some("session_exited")
                                {
                                    // Shut down write stream to terminate writer thread.
                                    if let Ok(w) = write_stream_for_shutdown.lock() {
                                        w.shutdown(std::net::Shutdown::Both).ok();
                                    }
                                    proxy(DaemonReaderEvent::Exited);
                                    return;
                                }
                            }
                        }
                    }
                    _ => {}
                }
            }
            Err(ref e) if e.kind() == std::io::ErrorKind::UnexpectedEof => {
                // Connection closed.
                break;
            }
            Err(e) => {
                log::error!("pane {pane_id}: daemon read error: {e}");
                break;
            }
        }
    }

    // Shut down write stream to terminate writer thread.
    if let Ok(w) = write_stream_for_shutdown.lock() {
        w.shutdown(std::net::Shutdown::Both).ok();
    }
    proxy(DaemonReaderEvent::Exited);
}

/// Events produced by the daemon reader thread.
pub enum DaemonReaderEvent {
    /// Raw PTY output data.
    Output(Vec<u8>),
    /// Terminal snapshot data (JSON-serialized TerminalSnapshot).
    Snapshot(Vec<u8>),
    /// Session has exited or connection was lost.
    Exited,
}
