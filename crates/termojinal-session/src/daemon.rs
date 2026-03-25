//! termojinald daemon — manages sessions and listens for connections.

use crate::{SessionError, SessionManager};
use std::sync::Arc;
use tokio::net::UnixListener;
use tokio::sync::Mutex;

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

        // Ensure parent directory exists.
        if let Some(parent) = std::path::Path::new(&self.socket_path).parent() {
            std::fs::create_dir_all(parent)?;
        }

        let listener =
            UnixListener::bind(&self.socket_path).map_err(|e| SessionError::Io(e))?;

        log::info!("termojinald listening on {}", self.socket_path);

        // --- Clean up stale session files, then restore saved sessions ---
        {
            let mut manager = self.manager.lock().await;

            // Load saved session states.
            match manager.load_saved_states() {
                Ok(states) => {
                    // Remove stale session files whose PIDs are no longer alive.
                    let (live, stale): (Vec<_>, Vec<_>) =
                        states.into_iter().partition(|s| is_pid_alive(s.pid));

                    for s in &stale {
                        log::info!(
                            "removing stale session file: {} (pid={:?})",
                            s.name,
                            s.pid
                        );
                        manager.remove_saved(&s.id).ok();
                    }

                    // Restore live sessions by respawning shells in their original dirs.
                    log::info!("restoring {} saved sessions", live.len());
                    for saved in &live {
                        match manager.create_session(
                            &saved.shell,
                            &saved.cwd,
                            saved.cols,
                            saved.rows,
                        ) {
                            Ok(_) => log::info!(
                                "restored session: {} (cwd={})",
                                saved.name,
                                saved.cwd
                            ),
                            Err(e) => {
                                log::warn!("failed to restore session {}: {e}", saved.name)
                            }
                        }
                    }
                }
                Err(e) => {
                    log::warn!("failed to load saved sessions: {e}");
                }
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

        // Accept connections (Phase 1: basic loop, full IPC in Phase 2).
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
                Err(e) => {
                    log::error!("accept error: {e}");
                }
            }
        }
    }

    pub fn manager(&self) -> &Arc<Mutex<SessionManager>> {
        &self.manager
    }
}

impl Drop for Daemon {
    fn drop(&mut self) {
        // Clean up socket file.
        std::fs::remove_file(&self.socket_path).ok();
    }
}

/// Check if a process with the given PID is still alive.
/// If `pid` is `None`, the session is considered stale.
fn is_pid_alive(pid: Option<i32>) -> bool {
    let Some(pid) = pid else {
        return false;
    };
    // Use kill(pid, 0) to check — returns Ok if the process exists.
    use nix::sys::signal;
    use nix::unistd::Pid;
    signal::kill(Pid::from_raw(pid), None).is_ok()
}



/// Handle a single IPC connection (JSON protocol).
async fn handle_connection(
    stream: tokio::net::UnixStream,
    manager: Arc<Mutex<SessionManager>>,
) -> Result<(), SessionError> {
    use serde_json::json;
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

    let (reader, mut writer) = stream.into_split();
    let mut buf_reader = BufReader::new(reader);
    let mut line = String::new();

    let n = buf_reader
        .read_line(&mut line)
        .await
        .map_err(SessionError::Io)?;
    if n == 0 {
        return Ok(());
    }

    let trimmed = line.trim();
    log::debug!("IPC request: {trimmed}");

    // Parse JSON and dispatch by "type" field.
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
                    let mgr = manager.lock().await;
                    let details: Vec<serde_json::Value> = mgr.list_details().iter().map(|s| {
                        json!({
                            "id": s.id,
                            "name": s.name,
                            "shell": s.shell,
                            "cwd": s.cwd,
                            "pid": s.pid,
                            "cols": s.cols,
                            "rows": s.rows,
                            "created_at": s.created_at.to_rfc3339(),
                        })
                    }).collect();
                    json!({"success": true, "data": {"sessions": details}})
                }
                "create_session" => {
                    let shell = req.get("shell").and_then(|v| v.as_str())
                        .unwrap_or(&std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".to_string()))
                        .to_string();
                    let cwd = req.get("cwd").and_then(|v| v.as_str()).unwrap_or(".").to_string();
                    let mut mgr = manager.lock().await;
                    match mgr.create_session(&shell, &cwd, 80, 24) {
                        Ok(session) => json!({
                            "success": true,
                            "data": {"id": session.state.id, "name": session.state.name}
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
                    let id = req.get("id").and_then(|v| v.as_str()).unwrap_or("");
                    let cols = req.get("cols").and_then(|v| v.as_u64()).unwrap_or(80) as u16;
                    let rows = req.get("rows").and_then(|v| v.as_u64()).unwrap_or(24) as u16;
                    let mut mgr = manager.lock().await;
                    if let Some(session) = mgr.get_mut(id) {
                        match session.resize(cols, rows) {
                            Ok(()) => json!({"success": true}),
                            Err(e) => json!({"success": false, "error": format!("{e}")}),
                        }
                    } else {
                        json!({"success": false, "error": "session not found"})
                    }
                }
                "register_session" => {
                    let pane_id = req.get("pane_id").and_then(|v| v.as_u64()).unwrap_or(0);
                    let pid = req.get("pid").and_then(|v| v.as_i64()).unwrap_or(0) as i32;
                    if pid <= 0 {
                        json!({"success": false, "error": "invalid pid"})
                    } else {
                        let shell = req.get("shell").and_then(|v| v.as_str()).unwrap_or("/bin/sh").to_string();
                        let cwd = req.get("cwd").and_then(|v| v.as_str()).unwrap_or(".").to_string();
                        let cols = req.get("cols").and_then(|v| v.as_u64()).unwrap_or(80) as u16;
                        let rows = req.get("rows").and_then(|v| v.as_u64()).unwrap_or(24) as u16;
                        let mut mgr = manager.lock().await;
                        let id = mgr.register_external_session(pane_id, pid, &shell, &cwd, cols, rows);
                        log::info!("registered external session: pane_id={pane_id}, pid={pid}, id={id}");
                        json!({"success": true, "data": {"id": id, "pane_id": pane_id}})
                    }
                }
                "unregister_session" => {
                    let pane_id = req.get("pane_id").and_then(|v| v.as_u64()).unwrap_or(0);
                    let mut mgr = manager.lock().await;
                    let removed = mgr.unregister_external_session(pane_id);
                    log::info!("unregistered external session: pane_id={pane_id}, removed={removed}");
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
                "claude_status_update" => {
                    // The daemon's own listener acknowledges the request.
                    // In the full architecture the GUI app handles these via
                    // the IpcServer callback; here we just log and ACK.
                    let state = req.get("state").and_then(|v| v.as_str()).unwrap_or("unknown");
                    let pid = req.get("pid").and_then(|v| v.as_i64());
                    log::info!("claude status update (daemon): state={state}, pid={pid:?}");
                    json!({"success": true})
                }
                _ => json!({"success": false, "error": format!("unknown request type: {req_type}")}),
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
///
/// This is the socket the app binds to so the daemon can send commands
/// like `toggle_quick_terminal` directly to the running GUI process.
pub fn app_socket_path() -> String {
    let data_dir = dirs::data_local_dir()
        .unwrap_or_else(|| std::path::PathBuf::from("/tmp"));
    data_dir
        .join("termojinal")
        .join("termojinal-app.sock")
        .to_string_lossy()
        .to_string()
}
