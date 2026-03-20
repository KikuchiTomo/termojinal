//! Allow Flow engine for AI agent coordination in jterm.
//!
//! Detects when AI tools (Claude Code, Codex, Aider) need human permission and
//! provides a mechanism to approve or deny requests. The engine manages:
//!
//! - **Detection**: scanning terminal output and OSC notifications for permission prompts
//! - **Rules**: auto-allowing or auto-denying requests based on user-configured rules
//! - **Response**: generating the appropriate PTY write to approve or deny a request

use serde::Deserialize;

pub mod detector;
pub mod request;
pub mod rules;

pub use detector::{AllowFlowDetector, DetectionPattern, PatternConfig};
pub use request::{AllowDecision, AllowRequest, AllowResponse, AllowStatus, DetectionSource};
pub use rules::{AllowRule, RuleScope, RuleStore};

/// Configuration for the Allow Flow engine (loaded from TOML).
#[derive(Debug, Clone, Deserialize)]
pub struct AllowFlowConfig {
    /// Show an overlay in the terminal pane when a request is pending.
    #[serde(default = "default_true")]
    pub overlay_enabled: bool,
    /// Show pending requests in the side panel.
    #[serde(default = "default_true")]
    pub side_panel_enabled: bool,
    /// Auto-focus the pane when a permission request is detected.
    #[serde(default = "default_false")]
    pub auto_focus: bool,
    /// Play a sound when a permission request is detected.
    #[serde(default = "default_false")]
    pub sound: bool,
    /// Custom detection patterns.
    #[serde(default)]
    pub patterns: Vec<PatternConfig>,
}

fn default_true() -> bool {
    true
}
fn default_false() -> bool {
    false
}

impl Default for AllowFlowConfig {
    fn default() -> Self {
        Self {
            overlay_enabled: true,
            side_panel_enabled: true,
            auto_focus: false,
            sound: false,
            patterns: Vec::new(),
        }
    }
}

/// The top-level Allow Flow engine.
///
/// Owns the detector, rule store, and request queue. This is the main
/// entry point that the jterm UI layer interacts with.
pub struct AllowFlowEngine {
    detector: AllowFlowDetector,
    rule_store: RuleStore,
    config: AllowFlowConfig,
    requests: Vec<AllowRequest>,
}

impl AllowFlowEngine {
    /// Create a new engine with the given configuration.
    pub fn new(config: AllowFlowConfig) -> Self {
        let detector = AllowFlowDetector::new(&config.patterns);
        let rule_store = RuleStore::new();
        Self {
            detector,
            rule_store,
            config,
            requests: Vec::new(),
        }
    }

    /// Create a new engine with a custom rule store path (useful for testing).
    pub fn with_rule_store(config: AllowFlowConfig, rule_store: RuleStore) -> Self {
        let detector = AllowFlowDetector::new(&config.patterns);
        Self {
            detector,
            rule_store,
            config,
            requests: Vec::new(),
        }
    }

    /// Process an OSC notification from a pane.
    ///
    /// Returns a reference to the newly created request if a permission prompt
    /// was detected, or `None` if it was auto-resolved by a rule.
    pub fn process_osc(
        &mut self,
        pane_id: u64,
        ws_idx: usize,
        notification: &str,
    ) -> Option<&AllowRequest> {
        let request = self.detector.scan_osc(pane_id, ws_idx, notification)?;
        self.handle_detected_request(request)
    }

    /// Process visible terminal output from a pane.
    ///
    /// Returns a reference to the newly created request if a permission prompt
    /// was detected, or `None` if no prompt was found or it was auto-resolved.
    pub fn process_output(
        &mut self,
        pane_id: u64,
        ws_idx: usize,
        lines: &[&str],
    ) -> Option<&AllowRequest> {
        let request = self.detector.scan_output(pane_id, ws_idx, lines)?;
        self.handle_detected_request(request)
    }

    /// Respond to a pending request with the given decision.
    ///
    /// Returns an `AllowResponse` containing the string to write to the PTY,
    /// or `None` if the request ID was not found or already resolved.
    pub fn respond(&mut self, request_id: u64, decision: AllowDecision) -> Option<AllowResponse> {
        let idx = self
            .requests
            .iter()
            .position(|r| r.id == request_id && r.status == AllowStatus::Pending)?;

        let request = &mut self.requests[idx];
        let pty_write = match decision {
            AllowDecision::Allow => {
                request.status = AllowStatus::Allowed;
                request.yes_response.clone()
            }
            AllowDecision::Deny => {
                request.status = AllowStatus::Denied;
                request.no_response.clone()
            }
        };

        Some(AllowResponse {
            pane_id: request.pane_id,
            pty_write,
        })
    }

    /// Get all pending requests.
    pub fn pending_requests(&self) -> Vec<&AllowRequest> {
        self.requests
            .iter()
            .filter(|r| r.status == AllowStatus::Pending)
            .collect()
    }

    /// Get all requests (pending, allowed, and denied).
    pub fn all_requests(&self) -> &[AllowRequest] {
        &self.requests
    }

    /// Remember the decision for a request as a rule for future auto-matching.
    ///
    /// The rule is created from the request's tool name and action.
    pub fn apply_rule(&mut self, request_id: u64, scope: RuleScope) {
        if let Some(request) = self.requests.iter().find(|r| r.id == request_id) {
            let decision = match request.status {
                AllowStatus::Allowed => AllowDecision::Allow,
                AllowStatus::Denied => AllowDecision::Deny,
                AllowStatus::Pending => return,
            };

            self.rule_store.add_rule(AllowRule {
                tool: request.tool_name.clone(),
                action: request.action.clone(),
                decision,
                scope,
            });
        }
    }

    /// Access the underlying rule store.
    pub fn rule_store(&self) -> &RuleStore {
        &self.rule_store
    }

    /// Access the underlying rule store mutably.
    pub fn rule_store_mut(&mut self) -> &mut RuleStore {
        &mut self.rule_store
    }

    /// Access the engine configuration.
    pub fn config(&self) -> &AllowFlowConfig {
        &self.config
    }

    /// Handle a detected request: check rules for auto-match, queue if pending.
    fn handle_detected_request(&mut self, request: AllowRequest) -> Option<&AllowRequest> {
        // Check if a rule auto-resolves this request.
        if let Some(decision) = self.rule_store.match_request(&request) {
            log::info!(
                "auto-{} request from '{}' for '{}' (rule match)",
                decision,
                request.tool_name,
                request.action,
            );
            let mut request = request;
            match decision {
                AllowDecision::Allow => request.status = AllowStatus::Allowed,
                AllowDecision::Deny => request.status = AllowStatus::Denied,
            }
            self.requests.push(request);
            // Auto-resolved requests don't need UI interaction.
            return None;
        }

        // No rule matched; queue as pending.
        self.requests.push(request);
        self.requests.last()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::NamedTempFile;

    fn test_engine() -> (AllowFlowEngine, NamedTempFile) {
        let tmp = NamedTempFile::new().unwrap();
        std::fs::write(tmp.path(), "").unwrap();
        let config = AllowFlowConfig::default();
        let rule_store = RuleStore::with_path(tmp.path().to_path_buf());
        (AllowFlowEngine::with_rule_store(config, rule_store), tmp)
    }

    #[test]
    fn test_process_osc_detection() {
        let (mut engine, _tmp) = test_engine();
        let result = engine.process_osc(1, 0, "Do you want to execute bash command?");
        assert!(result.is_some());
        let req = result.unwrap();
        assert_eq!(req.pane_id, 1);
        assert_eq!(req.tool_name, "Claude Code");
        assert_eq!(req.status, AllowStatus::Pending);
    }

    #[test]
    fn test_process_output_detection() {
        let (mut engine, _tmp) = test_engine();
        let lines = &["Allow Claude Code to run 'cargo test'"];
        let result = engine.process_output(2, 1, lines);
        assert!(result.is_some());
        assert_eq!(engine.pending_requests().len(), 1);
    }

    #[test]
    fn test_process_no_detection() {
        let (mut engine, _tmp) = test_engine();
        let lines = &["Building project...", "Compiling jterm v0.1.0"];
        let result = engine.process_output(1, 0, lines);
        assert!(result.is_none());
        assert!(engine.pending_requests().is_empty());
    }

    #[test]
    fn test_respond_allow() {
        let (mut engine, _tmp) = test_engine();
        let result = engine.process_osc(1, 0, "Do you want to execute something?");
        let req_id = result.unwrap().id;

        let response = engine.respond(req_id, AllowDecision::Allow);
        assert!(response.is_some());
        let response = response.unwrap();
        assert_eq!(response.pane_id, 1);
        assert_eq!(response.pty_write, "y\n");
        assert!(engine.pending_requests().is_empty());
    }

    #[test]
    fn test_respond_deny() {
        let (mut engine, _tmp) = test_engine();
        let result = engine.process_osc(1, 0, "Do you want to delete everything?");
        let req_id = result.unwrap().id;

        let response = engine.respond(req_id, AllowDecision::Deny);
        assert!(response.is_some());
        assert_eq!(response.unwrap().pty_write, "n\n");
    }

    #[test]
    fn test_respond_nonexistent_id() {
        let (mut engine, _tmp) = test_engine();
        assert!(engine.respond(999, AllowDecision::Allow).is_none());
    }

    #[test]
    fn test_respond_already_resolved() {
        let (mut engine, _tmp) = test_engine();
        let result = engine.process_osc(1, 0, "Do you want to run tests?");
        let req_id = result.unwrap().id;

        engine.respond(req_id, AllowDecision::Allow);
        // Second response should return None.
        assert!(engine.respond(req_id, AllowDecision::Deny).is_none());
    }

    #[test]
    fn test_auto_match_with_rule() {
        let tmp = NamedTempFile::new().unwrap();
        std::fs::write(tmp.path(), "").unwrap();
        let config = AllowFlowConfig::default();
        let mut rule_store = RuleStore::with_path(tmp.path().to_path_buf());
        rule_store.add_rule(AllowRule {
            tool: "Claude Code".into(),
            action: "tool use".into(),
            decision: AllowDecision::Allow,
            scope: RuleScope::Session,
        });

        let mut engine = AllowFlowEngine::with_rule_store(config, rule_store);

        // This should be auto-resolved by the rule.
        let result = engine.process_osc(1, 0, "Do you want to read file main.rs?");
        assert!(result.is_none(), "auto-matched request should not be returned as pending");

        // The request should still be recorded but not pending.
        assert!(engine.pending_requests().is_empty());
        assert_eq!(engine.all_requests().len(), 1);
        assert_eq!(engine.all_requests()[0].status, AllowStatus::Allowed);
    }

    #[test]
    fn test_apply_rule_from_request() {
        let (mut engine, _tmp) = test_engine();

        let result = engine.process_osc(1, 0, "Do you want to execute build?");
        let req_id = result.unwrap().id;
        engine.respond(req_id, AllowDecision::Allow);
        engine.apply_rule(req_id, RuleScope::Session);

        assert_eq!(engine.rule_store().list_rules().len(), 1);
        assert_eq!(engine.rule_store().list_rules()[0].tool, "Claude Code");
        assert_eq!(engine.rule_store().list_rules()[0].decision, AllowDecision::Allow);
    }

    #[test]
    fn test_apply_rule_pending_ignored() {
        let (mut engine, _tmp) = test_engine();

        let result = engine.process_osc(1, 0, "Do you want to execute build?");
        let req_id = result.unwrap().id;

        // Trying to apply a rule from a pending request should be ignored.
        engine.apply_rule(req_id, RuleScope::Session);
        assert!(engine.rule_store().list_rules().is_empty());
    }

    #[test]
    fn test_integration_full_flow() {
        let (mut engine, _tmp) = test_engine();

        // Step 1: Detect a permission prompt via OSC.
        let result = engine.process_osc(1, 0, "Do you want to write file src/lib.rs?");
        assert!(result.is_some());
        let req_id = result.unwrap().id;
        assert_eq!(engine.pending_requests().len(), 1);

        // Step 2: User allows it.
        let response = engine.respond(req_id, AllowDecision::Allow);
        assert!(response.is_some());
        assert_eq!(response.unwrap().pty_write, "y\n");
        assert!(engine.pending_requests().is_empty());

        // Step 3: User saves a persistent rule for this.
        engine.apply_rule(req_id, RuleScope::Persistent);

        // Step 4: Next time, the same kind of request is auto-allowed.
        let result = engine.process_osc(2, 0, "Do you want to write file README.md?");
        assert!(result.is_none(), "should be auto-resolved by rule");
        assert_eq!(engine.all_requests().len(), 2);
        assert_eq!(engine.all_requests()[1].status, AllowStatus::Allowed);
    }

    #[test]
    fn test_config_deserialization() {
        let toml_str = r#"
overlay_enabled = true
side_panel_enabled = false
auto_focus = true
sound = false

[[patterns]]
tool = "Aider"
action = "edit file"
pattern = "(?i)aider wants to edit (.+)"
yes_response = "yes\n"
no_response = "no\n"
"#;
        let config: AllowFlowConfig = toml::from_str(toml_str).unwrap();
        assert!(config.overlay_enabled);
        assert!(!config.side_panel_enabled);
        assert!(config.auto_focus);
        assert!(!config.sound);
        assert_eq!(config.patterns.len(), 1);
        assert_eq!(config.patterns[0].tool, "Aider");
    }

    #[test]
    fn test_default_config() {
        let config = AllowFlowConfig::default();
        assert!(config.overlay_enabled);
        assert!(config.side_panel_enabled);
        assert!(!config.auto_focus);
        assert!(!config.sound);
        assert!(config.patterns.is_empty());
    }
}
