//! Unix domain socket IPC client.
//!
//! Connects to the termojinald daemon socket and sends [`IpcRequest`] messages,
//! receiving [`IpcResponse`] replies.

use crate::protocol::{IpcRequest, IpcResponse};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;

/// Errors that can occur in the IPC client.
#[derive(Debug, thiserror::Error)]
pub enum ClientError {
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),

    #[error("connection failed: is termojinald running? (socket: {0})")]
    ConnectionFailed(String),

    #[error("empty response from daemon")]
    EmptyResponse,
}

/// An IPC client that connects to the termojinald daemon.
pub struct IpcClient {
    socket_path: String,
}

impl IpcClient {
    /// Create a new client targeting the given socket path.
    pub fn new(socket_path: String) -> Self {
        Self { socket_path }
    }

    /// Create a client using the default socket path from termojinal-session.
    pub fn default_path() -> Self {
        Self::new(termojinal_session::daemon::socket_path())
    }

    /// Send a request and wait for the response.
    pub async fn send(&self, request: &IpcRequest) -> Result<IpcResponse, ClientError> {
        let stream = UnixStream::connect(&self.socket_path)
            .await
            .map_err(|_| ClientError::ConnectionFailed(self.socket_path.clone()))?;

        let (reader, mut writer) = stream.into_split();

        // Write the request as a JSON line.
        let mut request_json = serde_json::to_string(request)?;
        request_json.push('\n');
        writer.write_all(request_json.as_bytes()).await?;

        // Read the response line.
        let mut buf_reader = BufReader::new(reader);
        let mut line = String::new();
        let n = buf_reader.read_line(&mut line).await?;
        if n == 0 {
            return Err(ClientError::EmptyResponse);
        }

        let response: IpcResponse = serde_json::from_str(line.trim())?;
        Ok(response)
    }

    /// Convenience: send a Ping request.
    pub async fn ping(&self) -> Result<IpcResponse, ClientError> {
        self.send(&IpcRequest::Ping).await
    }

    /// Convenience: list sessions.
    pub async fn list_sessions(&self) -> Result<IpcResponse, ClientError> {
        self.send(&IpcRequest::ListSessions).await
    }

    /// Convenience: create a session.
    pub async fn create_session(
        &self,
        shell: Option<String>,
        cwd: Option<String>,
    ) -> Result<IpcResponse, ClientError> {
        self.send(&IpcRequest::CreateSession {
            shell,
            cwd,
            cols: None,
            rows: None,
        })
        .await
    }

    /// Convenience: kill a session.
    pub async fn kill_session(&self, id: String) -> Result<IpcResponse, ClientError> {
        self.send(&IpcRequest::KillSession { id }).await
    }

    /// Convenience: resize a session.
    pub async fn resize_session(
        &self,
        id: String,
        cols: u16,
        rows: u16,
    ) -> Result<IpcResponse, ClientError> {
        self.send(&IpcRequest::ResizeSession { id, cols, rows })
            .await
    }
}
