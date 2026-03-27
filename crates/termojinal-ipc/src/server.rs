//! Unix domain socket IPC server.
//!
//! The server listens on the termojinald socket and dispatches incoming
//! [`IpcRequest`] messages to the [`SessionManager`], returning
//! [`IpcResponse`] results.

use crate::protocol::{IpcRequest, IpcResponse};
use std::sync::Arc;
use termojinal_session::{SessionError, SessionManager};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::Mutex;

/// Errors that can occur in the IPC server.
#[derive(Debug, thiserror::Error)]
pub enum ServerError {
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    #[error("session error: {0}")]
    Session(#[from] SessionError),

    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),
}

/// Callback type for handling Claude status updates.
///
/// When the server receives a `ClaudeStatusUpdate` IPC request it invokes
/// this callback so the caller (daemon or app) can forward the event to the
/// Claude session monitor's hooks store.
pub type ClaudeStatusCallback = Arc<dyn Fn(ClaudeStatusEvent) + Send + Sync>;

/// Structured event extracted from a `ClaudeStatusUpdate` IPC request.
#[derive(Debug, Clone)]
pub struct ClaudeStatusEvent {
    pub session_id: Option<String>,
    pub state: String,
    pub agent_id: Option<String>,
    pub agent_type: Option<String>,
    pub description: Option<String>,
    pub pid: Option<i32>,
}

/// The IPC server that wraps a `SessionManager` and handles JSON requests.
pub struct IpcServer {
    manager: Arc<Mutex<SessionManager>>,
    socket_path: String,
    claude_status_cb: Option<ClaudeStatusCallback>,
}

impl IpcServer {
    /// Create a new IPC server.
    pub fn new(manager: Arc<Mutex<SessionManager>>, socket_path: String) -> Self {
        Self {
            manager,
            socket_path,
            claude_status_cb: None,
        }
    }

    /// Set a callback for handling Claude Code status updates.
    pub fn set_claude_status_callback(&mut self, cb: ClaudeStatusCallback) {
        self.claude_status_cb = Some(cb);
    }

    /// Start listening for IPC connections.
    ///
    /// This will clean up any stale socket file, bind to the socket path,
    /// and loop accepting connections.
    pub async fn run(&self) -> Result<(), ServerError> {
        // Clean up stale socket.
        if std::path::Path::new(&self.socket_path).exists() {
            std::fs::remove_file(&self.socket_path).ok();
        }

        // Ensure parent directory exists.
        if let Some(parent) = std::path::Path::new(&self.socket_path).parent() {
            std::fs::create_dir_all(parent)?;
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(parent, std::fs::Permissions::from_mode(0o700))?;
        }

        let listener = UnixListener::bind(&self.socket_path)?;

        // Restrict socket file permissions to owner only.
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(
                &self.socket_path,
                std::fs::Permissions::from_mode(0o600),
            )?;
        }
        log::info!("IPC server listening on {}", self.socket_path);

        loop {
            match listener.accept().await {
                Ok((stream, _addr)) => {
                    let manager = self.manager.clone();
                    let cb = self.claude_status_cb.clone();
                    tokio::spawn(async move {
                        if let Err(e) = handle_connection(stream, manager, cb).await {
                            log::error!("IPC connection error: {e}");
                        }
                    });
                }
                Err(e) => {
                    log::error!("IPC accept error: {e}");
                }
            }
        }
    }
}

impl Drop for IpcServer {
    fn drop(&mut self) {
        std::fs::remove_file(&self.socket_path).ok();
    }
}

/// Handle a single client connection.
///
/// Reads one JSON line, dispatches the request, and writes back a JSON response.
async fn handle_connection(
    stream: UnixStream,
    manager: Arc<Mutex<SessionManager>>,
    claude_status_cb: Option<ClaudeStatusCallback>,
) -> Result<(), ServerError> {
    let (reader, mut writer) = stream.into_split();
    let mut buf_reader = BufReader::new(reader.take(1_048_576));
    let mut line = String::new();

    let n = buf_reader.read_line(&mut line).await?;
    if n == 0 {
        return Ok(());
    }

    log::debug!("IPC request: {}", line.trim());

    let response = match serde_json::from_str::<IpcRequest>(line.trim()) {
        Ok(request) => dispatch(request, &manager, &claude_status_cb).await,
        Err(e) => IpcResponse::err(format!("invalid request: {e}")),
    };

    let mut response_json = serde_json::to_string(&response)?;
    response_json.push('\n');
    writer.write_all(response_json.as_bytes()).await?;

    Ok(())
}

/// Dispatch an IPC request to the session manager and return a response.
async fn dispatch(
    request: IpcRequest,
    manager: &Arc<Mutex<SessionManager>>,
    claude_status_cb: &Option<ClaudeStatusCallback>,
) -> IpcResponse {
    match request {
        IpcRequest::Ping => IpcResponse::ok(serde_json::json!({"status": "pong"})),

        IpcRequest::ListSessions => {
            let mgr = manager.lock().await;
            let ids: Vec<&str> = mgr.list();
            let id_list: Vec<String> = ids.into_iter().map(|s| s.to_string()).collect();
            IpcResponse::ok(serde_json::json!({"sessions": id_list}))
        }

        IpcRequest::ListSessionDetails => {
            let mgr = manager.lock().await;
            let details: Vec<serde_json::Value> = mgr
                .list_details_with_attached()
                .iter()
                .map(|(s, attached)| {
                    serde_json::json!({
                        "id": s.id,
                        "name": s.name,
                        "shell": s.shell,
                        "cwd": s.cwd,
                        "pid": s.pid,
                        "cols": s.cols,
                        "rows": s.rows,
                        "created_at": s.created_at.to_rfc3339(),
                        "attached": attached,
                        "workspace_name": s.workspace_name,
                    })
                })
                .collect();
            IpcResponse::ok(serde_json::json!({"sessions": details}))
        }

        IpcRequest::CreateSession {
            shell,
            cwd,
            cols,
            rows,
        } => {
            let shell = shell.unwrap_or_else(|| {
                std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".to_string())
            });
            let cwd = cwd.unwrap_or_else(|| {
                std::env::current_dir()
                    .map(|p| p.to_string_lossy().to_string())
                    .unwrap_or_else(|_| "/".to_string())
            });
            let cols = cols.unwrap_or(80);
            let rows = rows.unwrap_or(24);
            let mut mgr = manager.lock().await;
            match mgr.create_session(&shell, &cwd, cols, rows) {
                Ok(session) => IpcResponse::ok(serde_json::json!({
                    "id": session.state.id,
                    "name": session.state.name,
                    "pid": session.state.pid,
                })),
                Err(e) => IpcResponse::err(format!("failed to create session: {e}")),
            }
        }

        IpcRequest::KillSession { id } => {
            let mut mgr = manager.lock().await;
            match mgr.remove(&id) {
                Ok(()) => IpcResponse::ok_empty(),
                Err(e) => IpcResponse::err(format!("failed to kill session: {e}")),
            }
        }

        IpcRequest::ResizeSession { id, cols, rows } => {
            let mut mgr = manager.lock().await;
            match mgr.resize_session(&id, cols, rows).await {
                Ok(()) => IpcResponse::ok_empty(),
                Err(e) => IpcResponse::err(format!("resize failed: {e}")),
            }
        }

        IpcRequest::FocusPane { id } => {
            log::info!("focus pane {id} (not yet implemented)");
            IpcResponse::ok(serde_json::json!({"pane": id}))
        }

        IpcRequest::SplitPane { direction } => {
            if direction != "horizontal" && direction != "vertical" {
                return IpcResponse::err(format!(
                    "invalid direction: {direction} (expected 'horizontal' or 'vertical')"
                ));
            }
            log::info!("split pane {direction} (not yet implemented)");
            IpcResponse::ok(serde_json::json!({"direction": direction}))
        }

        IpcRequest::ClosePane => {
            log::info!("close pane (not yet implemented)");
            IpcResponse::ok_empty()
        }

        IpcRequest::RegisterSession {
            pane_id,
            pid,
            shell,
            cwd,
            cols,
            rows,
        } => {
            let mut mgr = manager.lock().await;
            let id = mgr.register_external_session(pane_id, pid, &shell, &cwd, cols, rows);
            log::info!("registered external session: pane_id={pane_id}, pid={pid}, id={id}");
            IpcResponse::ok(serde_json::json!({"id": id, "pane_id": pane_id}))
        }

        IpcRequest::UnregisterSession { pane_id } => {
            let mut mgr = manager.lock().await;
            let removed = mgr.unregister_external_session(pane_id);
            log::info!("unregistered external session: pane_id={pane_id}, removed={removed}");
            IpcResponse::ok(serde_json::json!({"removed": removed}))
        }

        IpcRequest::AttachSession { id } => {
            // AttachSession is handled by the binary frame protocol in the daemon.
            // If it arrives via JSON, just acknowledge it.
            log::info!("attach_session via JSON (no-op): {id}");
            IpcResponse::ok(serde_json::json!({"id": id}))
        }

        IpcRequest::DetachSession { id } => {
            log::info!("detach_session via JSON: {id}");
            IpcResponse::ok(serde_json::json!({"id": id}))
        }

        IpcRequest::ExitSession { id } => {
            let mut mgr = manager.lock().await;
            match mgr.exit_session(&id) {
                Ok(None) => {
                    log::info!("exited session: {id}");
                    IpcResponse::ok_empty()
                }
                Ok(Some(proc_name)) => {
                    log::info!("session {id} has running process: {proc_name}");
                    IpcResponse::ok(serde_json::json!({"running_process": proc_name}))
                }
                Err(e) => IpcResponse::err(format!("failed to exit session: {e}")),
            }
        }

        IpcRequest::KillAll => {
            let mut mgr = manager.lock().await;
            let count = mgr.kill_all();
            log::info!("killed all {count} sessions");
            IpcResponse::ok(serde_json::json!({"killed": count}))
        }

        IpcRequest::UpdateSessionWorkspace { id, workspace_name } => {
            let mut mgr = manager.lock().await;
            match mgr.update_session_workspace(&id, &workspace_name) {
                Ok(()) => IpcResponse::ok_empty(),
                Err(e) => IpcResponse::err(format!("failed to update workspace: {e}")),
            }
        }

        IpcRequest::ClaudeStatusUpdate {
            session_id,
            state,
            agent_id,
            agent_type,
            description,
            pid,
        } => {
            log::info!(
                "claude status update: state={state}, pid={pid:?}, session_id={session_id:?}, agent_id={agent_id:?}"
            );
            if let Some(cb) = claude_status_cb {
                cb(ClaudeStatusEvent {
                    session_id,
                    state,
                    agent_id,
                    agent_type,
                    description,
                    pid,
                });
            }
            IpcResponse::ok_empty()
        }
    }
}
