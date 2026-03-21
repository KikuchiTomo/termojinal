//! Unix domain socket IPC server.
//!
//! The server listens on the termojinald socket and dispatches incoming
//! [`IpcRequest`] messages to the [`SessionManager`], returning
//! [`IpcResponse`] results.

use crate::protocol::{IpcRequest, IpcResponse};
use termojinal_session::{SessionError, SessionManager};
use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
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

/// The IPC server that wraps a `SessionManager` and handles JSON requests.
pub struct IpcServer {
    manager: Arc<Mutex<SessionManager>>,
    socket_path: String,
}

impl IpcServer {
    /// Create a new IPC server.
    pub fn new(manager: Arc<Mutex<SessionManager>>, socket_path: String) -> Self {
        Self {
            manager,
            socket_path,
        }
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
        }

        let listener = UnixListener::bind(&self.socket_path)?;
        log::info!("IPC server listening on {}", self.socket_path);

        loop {
            match listener.accept().await {
                Ok((stream, _addr)) => {
                    let manager = self.manager.clone();
                    tokio::spawn(async move {
                        if let Err(e) = handle_connection(stream, manager).await {
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
) -> Result<(), ServerError> {
    let (reader, mut writer) = stream.into_split();
    let mut buf_reader = BufReader::new(reader);
    let mut line = String::new();

    let n = buf_reader.read_line(&mut line).await?;
    if n == 0 {
        return Ok(());
    }

    log::debug!("IPC request: {}", line.trim());

    let response = match serde_json::from_str::<IpcRequest>(line.trim()) {
        Ok(request) => dispatch(request, &manager).await,
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
) -> IpcResponse {
    match request {
        IpcRequest::Ping => IpcResponse::ok(serde_json::json!({"status": "pong"})),

        IpcRequest::ListSessions => {
            let mgr = manager.lock().await;
            let ids: Vec<&str> = mgr.list();
            let id_list: Vec<String> = ids.into_iter().map(|s| s.to_string()).collect();
            IpcResponse::ok(serde_json::json!({"sessions": id_list}))
        }

        IpcRequest::CreateSession { shell, cwd } => {
            let shell = shell.unwrap_or_else(|| {
                std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".to_string())
            });
            let cwd = cwd.unwrap_or_else(|| {
                std::env::current_dir()
                    .map(|p| p.to_string_lossy().to_string())
                    .unwrap_or_else(|_| "/".to_string())
            });
            let mut mgr = manager.lock().await;
            match mgr.create_session(&shell, &cwd, 80, 24) {
                Ok(session) => IpcResponse::ok(serde_json::json!({
                    "id": session.state.id,
                    "name": session.state.name,
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
            match mgr.get_mut(&id) {
                Some(session) => match session.resize(cols, rows) {
                    Ok(()) => IpcResponse::ok_empty(),
                    Err(e) => IpcResponse::err(format!("resize failed: {e}")),
                },
                None => IpcResponse::err(format!("session not found: {id}")),
            }
        }

        IpcRequest::FocusPane { id } => {
            // Pane management is a future feature; acknowledge for now.
            log::info!("focus pane {id} (not yet implemented)");
            IpcResponse::ok(serde_json::json!({"pane": id}))
        }

        IpcRequest::SplitPane { direction } => {
            if direction != "horizontal" && direction != "vertical" {
                return IpcResponse::err(format!(
                    "invalid direction: {direction} (expected 'horizontal' or 'vertical')"
                ));
            }
            // Pane splitting is a future feature; acknowledge for now.
            log::info!("split pane {direction} (not yet implemented)");
            IpcResponse::ok(serde_json::json!({"direction": direction}))
        }

        IpcRequest::ClosePane => {
            // Pane management is a future feature; acknowledge for now.
            log::info!("close pane (not yet implemented)");
            IpcResponse::ok_empty()
        }

        IpcRequest::RegisterSession { pane_id, pid, shell, cwd, cols, rows } => {
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
    }
}
