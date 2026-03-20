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
use termojinal_pty::Pty;
use winit::keyboard::{Key, NamedKey};

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

    /// Total number of pending requests across all workspaces.
    #[allow(dead_code)]
    pub fn total_pending(&self) -> usize {
        self.engine.pending_requests().len()
    }

    /// Find the index of the first workspace that has pending requests.
    pub fn first_workspace_with_pending(&self) -> Option<usize> {
        self.engine
            .pending_requests()
            .first()
            .map(|r| r.workspace_idx)
    }

    // -----------------------------------------------------------------
    // Batch operations — Allow All / Deny All for a workspace
    // -----------------------------------------------------------------

    /// Allow ALL pending requests for a workspace. Returns the count approved.
    pub fn allow_all_for_workspace(
        &mut self,
        ws_idx: usize,
        pane_ptys: &mut std::collections::HashMap<u64, *mut Pty>,
    ) -> usize {
        let ids: Vec<(u64, u64)> = self
            .pending_for_workspace(ws_idx)
            .iter()
            .map(|r| (r.id, r.pane_id))
            .collect();
        let count = ids.len();
        for (req_id, pane_id) in ids {
            if let Some(response) = self.engine.respond(req_id, AllowDecision::Allow) {
                Self::write_to_pty(pane_ptys, pane_id, &response.pty_write);
            }
        }
        self.update_hint_visibility();
        count
    }

    /// Deny ALL pending requests for a workspace. Returns the count denied.
    pub fn deny_all_for_workspace(
        &mut self,
        ws_idx: usize,
        pane_ptys: &mut std::collections::HashMap<u64, *mut Pty>,
    ) -> usize {
        let ids: Vec<(u64, u64)> = self
            .pending_for_workspace(ws_idx)
            .iter()
            .map(|r| (r.id, r.pane_id))
            .collect();
        let count = ids.len();
        for (req_id, pane_id) in ids {
            if let Some(response) = self.engine.respond(req_id, AllowDecision::Deny) {
                Self::write_to_pty(pane_ptys, pane_id, &response.pty_write);
            }
        }
        self.update_hint_visibility();
        count
    }

    // -----------------------------------------------------------------
    // Key handling (Y / N / A / Esc) — works when pane is focused
    //
    //   Y = allow first pending     Shift+Y = allow ALL
    //   N = deny first pending      Shift+N = deny ALL
    //   A = allow + remember rule
    //   Esc = dismiss hint bar
    // -----------------------------------------------------------------

    /// Process a key event when there are pending requests for the active
    /// workspace.  Returns `true` if the key was consumed.
    pub fn process_key(
        &mut self,
        key: &Key,
        active_ws: usize,
        pane_ptys: &mut std::collections::HashMap<u64, *mut Pty>,
    ) -> bool {
        // Only intercept keys when there are pending requests for the
        // active workspace.
        if !self.has_pending_for_workspace(active_ws) {
            return false;
        }

        match key {
            Key::Character(c) => {
                match c.as_str() {
                    // --- Batch: uppercase = ALL ---
                    "Y" => {
                        let count = self.allow_all_for_workspace(active_ws, pane_ptys);
                        log::info!("allow-all: approved {} requests for workspace {}", count, active_ws);
                        true
                    }
                    "N" => {
                        let count = self.deny_all_for_workspace(active_ws, pane_ptys);
                        log::info!("deny-all: denied {} requests for workspace {}", count, active_ws);
                        true
                    }
                    // --- Single: lowercase ---
                    "y" => {
                        if let Some(req) = self.first_pending_for_workspace(active_ws) {
                            let req_id = req.id;
                            let pane_id = req.pane_id;
                            if let Some(response) = self.engine.respond(req_id, AllowDecision::Allow) {
                                Self::write_to_pty(pane_ptys, pane_id, &response.pty_write);
                            }
                            self.update_hint_visibility();
                        }
                        true
                    }
                    "n" => {
                        if let Some(req) = self.first_pending_for_workspace(active_ws) {
                            let req_id = req.id;
                            let pane_id = req.pane_id;
                            if let Some(response) = self.engine.respond(req_id, AllowDecision::Deny) {
                                Self::write_to_pty(pane_ptys, pane_id, &response.pty_write);
                            }
                            self.update_hint_visibility();
                        }
                        true
                    }
                    "a" | "A" => {
                        // Allow and remember as persistent rule (works for first pending).
                        if let Some(req) = self.first_pending_for_workspace(active_ws) {
                            let req_id = req.id;
                            let pane_id = req.pane_id;
                            if let Some(response) = self.engine.respond(req_id, AllowDecision::Allow) {
                                Self::write_to_pty(pane_ptys, pane_id, &response.pty_write);
                            }
                            self.engine.apply_rule(req_id, RuleScope::Persistent);
                            self.update_hint_visibility();
                        }
                        true
                    }
                    _ => false,
                }
            }
            Key::Named(NamedKey::Escape) => {
                // Dismiss the pane hint bar (requests remain pending).
                self.pane_hint_visible = false;
                true
            }
            _ => false,
        }
    }

    // -----------------------------------------------------------------
    // Internals
    // -----------------------------------------------------------------

    /// Get the first (oldest) pending request for a specific workspace.
    fn first_pending_for_workspace(&self, ws_idx: usize) -> Option<&AllowRequest> {
        self.engine
            .pending_requests()
            .into_iter()
            .find(|r| r.workspace_idx == ws_idx)
    }

    /// Hide the pane hint bar if no more pending requests remain.
    fn update_hint_visibility(&mut self) {
        if self.engine.pending_requests().is_empty() {
            self.pane_hint_visible = false;
        }
    }

    /// Write a response string to the appropriate pane's PTY.
    pub fn write_to_pty(
        pane_ptys: &mut std::collections::HashMap<u64, *mut Pty>,
        pane_id: u64,
        data: &str,
    ) {
        if let Some(&mut pty_ptr) = pane_ptys.get_mut(&pane_id) {
            // SAFETY: the caller ensures the Pty pointer is valid for the duration.
            let pty = unsafe { &*pty_ptr };
            let _ = pty.write(data.as_bytes());
        }
    }
}
