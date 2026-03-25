//! Daemon IPC client.

use crate::{Pane, UserEvent};
use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex};
use serde_json::json;
use termojinal_layout::PaneId;
use termojinal_vt::Terminal;
use winit::event_loop::EventLoopProxy;

pub(crate) struct DaemonHandle {
    socket_path: String,
}

impl DaemonHandle {
    pub(crate) fn new() -> Self {
        Self {
            socket_path: daemon_socket_path(),
        }
    }

    /// Send a JSON request to the daemon and return the response.
    pub(crate) fn send_request_json(&self, req: &serde_json::Value) -> Option<serde_json::Value> {
        use std::io::{BufRead, Write};
        use std::os::unix::net::UnixStream;

        let mut stream = UnixStream::connect(&self.socket_path).ok()?;
        stream
            .set_read_timeout(Some(std::time::Duration::from_secs(5)))
            .ok();
        let msg = format!("{}\n", req);
        stream.write_all(msg.as_bytes()).ok()?;
        let mut line = String::new();
        std::io::BufReader::new(&stream).read_line(&mut line).ok()?;
        serde_json::from_str(line.trim()).ok()
    }

    /// Create a session on the daemon. Returns (session_id, name, pid).
    pub(crate) fn create_session(
        &self,
        shell: &str,
        cwd: &str,
        cols: u16,
        rows: u16,
    ) -> Option<(String, String, i32)> {
        let req = json!({
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

    /// Resize a session.
    #[allow(dead_code)]
    pub(crate) fn resize_session(&self, session_id: &str, cols: u16, rows: u16) {
        let req = json!({
            "type": "resize_session",
            "id": session_id,
            "cols": cols,
            "rows": rows,
        });
        self.send_request_json(&req);
    }

    /// Kill a session.
    #[allow(dead_code)]
    pub(crate) fn kill_session(&self, session_id: &str) {
        let req = json!({
            "type": "kill_session",
            "id": session_id,
        });
        self.send_request_json(&req);
    }
}






// ---------------------------------------------------------------------------
// User events from PTY reader threads
// ---------------------------------------------------------------------------


pub(crate) fn spawn_pane(
    id: PaneId,
    cols: u16,
    rows: u16,
    proxy: &EventLoopProxy<UserEvent>,
    buffers: &Arc<Mutex<HashMap<PaneId, VecDeque<Vec<u8>>>>>,
    cwd: Option<String>,
    time_travel_config: Option<&crate::config::TimeTravelConfig>,
    cjk_width: bool,
) -> Result<Pane, termojinal_pty::PtyError> {
    let daemon = DaemonHandle::new();
    let shell = termojinal_pty::detect_shell();
    let cwd_str = cwd.as_deref().unwrap_or(".");

    // Create session on the daemon.
    let (session_id, _name, shell_pid) = daemon
        .create_session(&shell, cwd_str, cols, rows)
        .ok_or_else(|| {
            termojinal_pty::PtyError::Open(
                "failed to create session on daemon (is termojinald running?)".to_string(),
            )
        })?;

    log::info!(
        "pane {id}: daemon session={}, shell={}, pid={}",
        session_id,
        shell,
        shell_pid
    );

    let mut terminal = Terminal::new(cols as usize, rows as usize);
    terminal.set_cjk_width(cjk_width);
    if let Some(tt) = time_travel_config {
        terminal.set_command_history_enabled(tt.command_history);
        terminal.set_max_command_history(tt.max_command_history);
    }
    let vt_parser = vte::Parser::new();

    // Insert buffer for this pane.
    buffers.lock().unwrap().insert(id, VecDeque::new());

    // Create write channel for sending key input to the daemon reader thread.
    let (write_tx, write_rx) = std::sync::mpsc::channel::<Vec<u8>>();

    // Spawn daemon reader thread that connects to the daemon via binary
    // framing, attaches to the session, and reads PTY output.
    let proxy_clone = proxy.clone();
    let buffers_clone = buffers.clone();
    let sid = session_id.clone();
    let sock_path = daemon_socket_path();
    std::thread::Builder::new()
        .name(format!("daemon-reader-{id}"))
        .spawn(move || {
            use termojinal_ipc::daemon_connection::{daemon_reader_thread, DaemonReaderEvent};
            daemon_reader_thread(
                id,
                &sid,
                &sock_path,
                move |event| {
                    match event {
                        DaemonReaderEvent::Output(data) => {
                            if let Ok(mut lock) = buffers_clone.lock() {
                                if let Some(q) = lock.get_mut(&id) {
                                    q.push_back(data);
                                }
                            }
                            let _ = proxy_clone.send_event(UserEvent::PtyOutput(id));
                        }
                        DaemonReaderEvent::Snapshot(_data) => {
                            // TODO: restore terminal from snapshot on re-attach
                        }
                        DaemonReaderEvent::Exited => {
                            let _ = proxy_clone.send_event(UserEvent::PtyExited(id));
                        }
                    }
                },
                write_rx,
            );
        })
        .expect("failed to spawn daemon-reader thread");

    Ok(Pane {
        id,
        terminal,
        vt_parser,
        session_id,
        shell,
        shell_pid,
        write_tx,
        selection: None,
        preedit: None,
    })
}

/// Write data to a pane's PTY via the daemon (fire-and-forget, synchronous).
pub(crate) fn daemon_pty_write(session_id: &str, data: &[u8]) {
    termojinal_ipc::daemon_connection::daemon_pty_write(session_id, data);
}

/// Resize a pane's PTY via the daemon (fire-and-forget, synchronous).
pub(crate) fn daemon_pty_resize(session_id: &str, cols: u16, rows: u16) {
    termojinal_ipc::daemon_connection::daemon_pty_resize(session_id, cols, rows);
}

/// Get the Unix socket path for the termojinald daemon.
/// Mirrors `termojinal_session::daemon::socket_path()`.
pub(crate) fn daemon_socket_path() -> String {
    let runtime_dir = dirs::runtime_dir()
        .or_else(|| dirs::data_local_dir())
        .unwrap_or_else(|| std::path::PathBuf::from("/tmp"));
    runtime_dir
        .join("termojinal")
        .join("termojinald.sock")
        .to_string_lossy()
        .to_string()
}
