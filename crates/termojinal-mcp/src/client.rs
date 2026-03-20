//! Simple synchronous Unix socket client for the termojinal app IPC.
//!
//! Sends JSON requests to the app socket (`termojinal-app.sock`) and reads
//! back JSON responses.  Uses blocking I/O — each tool call opens a fresh
//! connection, sends a single request, and reads a single response line.

use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;

/// Client that talks to the termojinal GUI process over its Unix socket.
pub struct AppClient {
    socket_path: String,
}

impl AppClient {
    /// Create a new client using the standard app socket path
    /// (same as `app_socket_path()` in `termojinal-session`).
    pub fn new() -> Self {
        let dir = dirs::data_local_dir()
            .unwrap_or_else(|| std::path::PathBuf::from("/tmp"))
            .join("termojinal");
        Self {
            socket_path: dir
                .join("termojinal-app.sock")
                .to_string_lossy()
                .into_owned(),
        }
    }

    /// Send a raw JSON string and return the raw response line.
    pub fn send_raw(&self, request_json: &str) -> Result<String, String> {
        let mut stream = UnixStream::connect(&self.socket_path)
            .map_err(|e| format!("failed to connect to termojinal: {e}"))?;

        let mut msg = request_json.to_string();
        if !msg.ends_with('\n') {
            msg.push('\n');
        }
        stream
            .write_all(msg.as_bytes())
            .map_err(|e| format!("write error: {e}"))?;
        stream
            .flush()
            .map_err(|e| format!("flush error: {e}"))?;

        // Shutdown write side so server knows we're done sending.
        stream
            .shutdown(std::net::Shutdown::Write)
            .map_err(|e| format!("shutdown error: {e}"))?;

        let mut reader = BufReader::new(stream);
        let mut response = String::new();
        reader
            .read_line(&mut response)
            .map_err(|e| format!("read error: {e}"))?;
        Ok(response.trim().to_string())
    }

    /// Send a JSON value and parse the response as JSON.
    pub fn send(&self, request: &serde_json::Value) -> Result<serde_json::Value, String> {
        let json = serde_json::to_string(request).map_err(|e| e.to_string())?;
        let resp = self.send_raw(&json)?;
        serde_json::from_str(&resp).map_err(|e| format!("invalid response JSON: {e}"))
    }
}
