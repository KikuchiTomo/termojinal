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



/// Handle a single IPC connection.
async fn handle_connection(
    mut stream: tokio::net::UnixStream,
    manager: Arc<Mutex<SessionManager>>,
) -> Result<(), SessionError> {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    let mut buf = vec![0u8; 4096];
    let n = stream
        .read(&mut buf)
        .await
        .map_err(SessionError::Io)?;

    if n == 0 {
        return Ok(());
    }

    let request = String::from_utf8_lossy(&buf[..n]);
    log::debug!("IPC request: {request}");

    // Phase 1: simple text protocol.
    let response = match request.trim() {
        "list" => {
            let mgr = manager.lock().await;
            let ids = mgr.list();
            if ids.is_empty() {
                "no sessions\n".to_string()
            } else {
                ids.join("\n") + "\n"
            }
        }
        "ping" => "pong\n".to_string(),
        "show_palette" => {
            log::info!("received show_palette command");
            "ok\n".to_string()
        }
        "show_allow_flow" => {
            log::info!("received show_allow_flow command");
            "ok\n".to_string()
        }
        "toggle_quick_terminal" => {
            log::info!("received toggle_quick_terminal command");
            "ok\n".to_string()
        }
        _ => format!("unknown command: {}\n", request.trim()),
    };

    stream
        .write_all(response.as_bytes())
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
