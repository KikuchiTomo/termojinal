//! Data model for Allow Flow permission requests.

use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

/// Global auto-incrementing counter for request IDs.
static NEXT_REQUEST_ID: AtomicU64 = AtomicU64::new(1);

/// Generate a unique request ID.
pub fn next_id() -> u64 {
    NEXT_REQUEST_ID.fetch_add(1, Ordering::Relaxed)
}

/// How the permission prompt was detected.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum DetectionSource {
    /// Detected via an OSC escape sequence (9, 99, or 777).
    Osc,
    /// Detected via regex matching on visible terminal output.
    Regex,
}

/// Current status of a permission request.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum AllowStatus {
    Pending,
    Allowed,
    Denied,
}

/// Represents a permission request from an AI tool.
#[derive(Debug, Clone)]
pub struct AllowRequest {
    /// Unique identifier for this request.
    pub id: u64,
    /// Maps to PaneId in the main application.
    pub pane_id: u64,
    /// Workspace index containing the pane.
    pub workspace_idx: usize,
    /// Name of the AI tool (e.g. "Claude Code", "Codex", "Aider").
    pub tool_name: String,
    /// Action being requested (e.g. "execute bash command", "write file").
    pub action: String,
    /// Specific detail (e.g. the command or file path).
    pub detail: String,
    /// When the request was created.
    pub timestamp: Instant,
    /// Current status of the request.
    pub status: AllowStatus,
    /// How the request was detected.
    pub source: DetectionSource,
    /// The string to write to the PTY to approve (e.g. "y\n").
    pub yes_response: String,
    /// The string to write to the PTY to deny (e.g. "n\n").
    pub no_response: String,
}

impl AllowRequest {
    /// Create a new pending request with an auto-assigned ID.
    pub fn new(
        pane_id: u64,
        workspace_idx: usize,
        tool_name: String,
        action: String,
        detail: String,
        source: DetectionSource,
        yes_response: String,
        no_response: String,
    ) -> Self {
        Self {
            id: next_id(),
            pane_id,
            workspace_idx,
            tool_name,
            action,
            detail,
            timestamp: Instant::now(),
            status: AllowStatus::Pending,
            source,
            yes_response,
            no_response,
        }
    }
}

/// The decision made on a permission request.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AllowDecision {
    Allow,
    Deny,
}

/// Response to write to the PTY after a decision is made.
#[derive(Debug, Clone)]
pub struct AllowResponse {
    /// The pane to write the response to.
    pub pane_id: u64,
    /// Bytes to write to the PTY.
    pub pty_write: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_auto_incrementing_ids() {
        let r1 = AllowRequest::new(
            1, 0, "Claude Code".into(), "execute".into(),
            "ls".into(), DetectionSource::Osc, "y\n".into(), "n\n".into(),
        );
        let r2 = AllowRequest::new(
            1, 0, "Claude Code".into(), "execute".into(),
            "pwd".into(), DetectionSource::Regex, "y\n".into(), "n\n".into(),
        );
        assert!(r2.id > r1.id);
    }

    #[test]
    fn test_new_request_is_pending() {
        let r = AllowRequest::new(
            1, 0, "Codex".into(), "write file".into(),
            "main.rs".into(), DetectionSource::Osc, "y\n".into(), "n\n".into(),
        );
        assert_eq!(r.status, AllowStatus::Pending);
    }
}
