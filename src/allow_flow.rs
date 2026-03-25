//! Allow Flow UI module for AI agent permission management.
//!
//! Provides inline sidebar integration and a minimal pane hint bar for
//! reviewing and responding to AI tool permission requests detected by
//! the `termojinal_claude` engine.
//!
//! The sidebar is the primary interaction surface: pending requests are
//! shown expanded for the active workspace and collapsed for inactive
//! workspaces.  A thin 1-line hint bar at the bottom of the focused pane
//! reminds the user that a decision is needed.

use termojinal_claude::{AllowDecision, AllowFlowConfig, AllowFlowEngine, AllowRequest, RuleScope};
use winit::keyboard::{Key, NamedKey};

/// Result of processing a key in the Allow Flow UI.
pub enum AllowFlowKeyResult {
    /// Key was not consumed by Allow Flow.
    NotConsumed,
    /// Key was consumed but no requests were resolved (e.g. Esc to dismiss).
    Consumed,
    /// One or more requests were resolved. Contains (request_id, decision) pairs.
    Resolved(Vec<(u64, AllowDecision)>),
}

/// Allow Flow UI state.
///
/// The old overlay + side-panel design has been replaced with sidebar-
/// integrated notifications.  `pane_hint_visible` controls whether the
/// thin reminder bar appears at the bottom of the focused pane.
pub struct AllowFlowUI {
    pub engine: AllowFlowEngine,
    /// Whether the inline 1-line hint at the bottom of the active pane is shown.
    pub pane_hint_visible: bool,
}

impl AllowFlowUI {
    pub fn new(config: AllowFlowConfig) -> Self {
        Self {
            engine: AllowFlowEngine::new(config),
            pane_hint_visible: false,
        }
    }

    // -----------------------------------------------------------------
    // Workspace-scoped queries
    // -----------------------------------------------------------------

    /// Get pending requests for a specific workspace.
    pub fn pending_for_workspace(&self, ws_idx: usize) -> Vec<&AllowRequest> {
        self.engine
            .pending_requests()
            .into_iter()
            .filter(|r| r.workspace_idx == ws_idx)
            .collect()
    }

    /// Count of pending requests for a specific workspace.
    pub fn pending_count_for_workspace(&self, ws_idx: usize) -> usize {
        self.engine
            .pending_requests()
            .iter()
            .filter(|r| r.workspace_idx == ws_idx)
            .count()
    }

    /// Whether the given workspace has any pending requests.
    pub fn has_pending_for_workspace(&self, ws_idx: usize) -> bool {
        self.engine
            .pending_requests()
            .iter()
            .any(|r| r.workspace_idx == ws_idx)
    }

    /// Find the index of the first workspace that has pending requests.
    pub fn first_workspace_with_pending(&self) -> Option<usize> {
        self.engine
            .pending_requests()
            .first()
            .map(|r| r.workspace_idx)
    }

    // -----------------------------------------------------------------
    // Batch operations -- Allow All / Deny All for a workspace
    // -----------------------------------------------------------------

    /// Allow ALL pending requests for a workspace.
    pub fn allow_all_for_workspace(
        &mut self,
        ws_idx: usize,
        pane_sessions: &std::collections::HashMap<u64, String>,
    ) -> Vec<(u64, AllowDecision)> {
        let ids: Vec<(u64, u64)> = self
            .pending_for_workspace(ws_idx)
            .iter()
            .map(|r| (r.id, r.pane_id))
            .collect();
        let mut resolved = Vec::with_capacity(ids.len());
        for (req_id, pane_id) in ids {
            if let Some(response) = self.engine.respond(req_id, AllowDecision::Allow) {
                Self::write_to_pty(pane_sessions, pane_id, &response.pty_write);
                resolved.push((req_id, AllowDecision::Allow));
            }
        }
        self.update_hint_visibility();
        resolved
    }

    /// Deny ALL pending requests for a workspace.
    pub fn deny_all_for_workspace(
        &mut self,
        ws_idx: usize,
        pane_sessions: &std::collections::HashMap<u64, String>,
    ) -> Vec<(u64, AllowDecision)> {
        let ids: Vec<(u64, u64)> = self
            .pending_for_workspace(ws_idx)
            .iter()
            .map(|r| (r.id, r.pane_id))
            .collect();
        let mut resolved = Vec::with_capacity(ids.len());
        for (req_id, pane_id) in ids {
            if let Some(response) = self.engine.respond(req_id, AllowDecision::Deny) {
                Self::write_to_pty(pane_sessions, pane_id, &response.pty_write);
                resolved.push((req_id, AllowDecision::Deny));
            }
        }
        self.update_hint_visibility();
        resolved
    }

    // -----------------------------------------------------------------
    // Key handling (Y / N / A / Esc)
    // -----------------------------------------------------------------

    pub fn process_key(
        &mut self,
        key: &Key,
        _active_ws: usize,
        pane_sessions: &std::collections::HashMap<u64, String>,
    ) -> AllowFlowKeyResult {
        let target_ws = match self.first_workspace_with_pending() {
            Some(ws) => ws,
            None => return AllowFlowKeyResult::NotConsumed,
        };

        match key {
            Key::Character(c) => match c.as_str() {
                "Y" => {
                    let resolved = self.allow_all_for_workspace(target_ws, pane_sessions);
                    log::info!(
                        "allow-all: approved {} requests for workspace {}",
                        resolved.len(),
                        target_ws
                    );
                    AllowFlowKeyResult::Resolved(resolved)
                }
                "N" => {
                    let resolved = self.deny_all_for_workspace(target_ws, pane_sessions);
                    log::info!(
                        "deny-all: denied {} requests for workspace {}",
                        resolved.len(),
                        target_ws
                    );
                    AllowFlowKeyResult::Resolved(resolved)
                }
                "y" => {
                    let mut resolved = Vec::new();
                    if let Some(req) = self.first_pending_for_workspace(target_ws) {
                        let req_id = req.id;
                        let pane_id = req.pane_id;
                        if let Some(response) = self.engine.respond(req_id, AllowDecision::Allow) {
                            Self::write_to_pty(pane_sessions, pane_id, &response.pty_write);
                            resolved.push((req_id, AllowDecision::Allow));
                        }
                        self.update_hint_visibility();
                    }
                    AllowFlowKeyResult::Resolved(resolved)
                }
                "n" => {
                    let mut resolved = Vec::new();
                    if let Some(req) = self.first_pending_for_workspace(target_ws) {
                        let req_id = req.id;
                        let pane_id = req.pane_id;
                        if let Some(response) = self.engine.respond(req_id, AllowDecision::Deny) {
                            Self::write_to_pty(pane_sessions, pane_id, &response.pty_write);
                            resolved.push((req_id, AllowDecision::Deny));
                        }
                        self.update_hint_visibility();
                    }
                    AllowFlowKeyResult::Resolved(resolved)
                }
                "a" | "A" => {
                    let mut resolved = Vec::new();
                    if let Some(req) = self.first_pending_for_workspace(target_ws) {
                        let req_id = req.id;
                        let pane_id = req.pane_id;
                        if let Some(response) = self.engine.respond(req_id, AllowDecision::Allow) {
                            Self::write_to_pty(pane_sessions, pane_id, &response.pty_write);
                            resolved.push((req_id, AllowDecision::Allow));
                        }
                        self.engine.apply_rule(req_id, RuleScope::Persistent);
                        self.update_hint_visibility();
                    }
                    AllowFlowKeyResult::Resolved(resolved)
                }
                _ => AllowFlowKeyResult::NotConsumed,
            },
            Key::Named(NamedKey::Escape) => {
                self.pane_hint_visible = false;
                AllowFlowKeyResult::Consumed
            }
            _ => AllowFlowKeyResult::NotConsumed,
        }
    }

    // -----------------------------------------------------------------
    // Internals
    // -----------------------------------------------------------------

    fn first_pending_for_workspace(&self, ws_idx: usize) -> Option<&AllowRequest> {
        self.engine
            .pending_requests()
            .into_iter()
            .find(|r| r.workspace_idx == ws_idx)
    }

    fn update_hint_visibility(&mut self) {
        if self.engine.pending_requests().is_empty() {
            self.pane_hint_visible = false;
        }
    }

    /// Write a response string to the appropriate pane's PTY via the daemon.
    pub fn write_to_pty(
        pane_sessions: &std::collections::HashMap<u64, String>,
        pane_id: u64,
        data: &str,
    ) {
        if let Some(session_id) = pane_sessions.get(&pane_id) {
            crate::daemon_pty_write(session_id, data.as_bytes());
        }
    }
}
