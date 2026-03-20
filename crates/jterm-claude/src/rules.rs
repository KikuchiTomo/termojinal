//! Persistent rules for auto-allowing or auto-denying permission requests.
//!
//! Rules are stored in `~/.config/jterm/allow_rules.toml` and can be
//! scoped to a single session or persisted across restarts.

use regex::Regex;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

use crate::request::{AllowDecision, AllowRequest};

/// Scope of a rule: session-only or persisted to disk.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RuleScope {
    Session,
    Persistent,
}

/// A single allow/deny rule.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AllowRule {
    /// Tool name pattern (exact match or regex).
    pub tool: String,
    /// Action pattern (exact match or regex).
    pub action: String,
    /// The decision to apply.
    pub decision: AllowDecision,
    /// Whether this rule is session-only or persistent.
    pub scope: RuleScope,
}

/// Serialization wrapper for the rules TOML file.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct RulesFile {
    #[serde(default, rename = "rules")]
    rules: Vec<AllowRule>,
}

/// Manage a collection of allow/deny rules.
pub struct RuleStore {
    rules: Vec<AllowRule>,
    path: PathBuf,
}

impl RuleStore {
    /// Create a new rule store, loading persistent rules from the default path.
    pub fn new() -> Self {
        let path = rules_path();
        let rules = load_rules_from(&path);
        Self { rules, path }
    }

    /// Create a rule store with a custom file path (useful for testing).
    pub fn with_path(path: PathBuf) -> Self {
        let rules = load_rules_from(&path);
        Self { rules, path }
    }

    /// Check if any rule matches the given request.
    ///
    /// Returns the first matching rule's decision, or `None` if no rule matches.
    pub fn match_request(&self, request: &AllowRequest) -> Option<AllowDecision> {
        for rule in &self.rules {
            if matches_pattern(&rule.tool, &request.tool_name)
                && matches_pattern(&rule.action, &request.action)
            {
                return Some(rule.decision);
            }
        }
        None
    }

    /// Add a new rule. Persistent rules are saved to disk immediately.
    pub fn add_rule(&mut self, rule: AllowRule) {
        self.rules.push(rule);
        self.save_persistent();
    }

    /// Remove the rule at the given index. Returns `true` if removed.
    pub fn remove_rule(&mut self, index: usize) -> bool {
        if index < self.rules.len() {
            self.rules.remove(index);
            self.save_persistent();
            true
        } else {
            false
        }
    }

    /// List all rules.
    pub fn list_rules(&self) -> &[AllowRule] {
        &self.rules
    }

    /// Remove all session-scoped rules (called on application exit).
    pub fn clear_session_rules(&mut self) {
        self.rules.retain(|r| r.scope == RuleScope::Persistent);
    }

    /// Save only persistent rules to disk.
    fn save_persistent(&self) {
        let persistent: Vec<AllowRule> = self
            .rules
            .iter()
            .filter(|r| r.scope == RuleScope::Persistent)
            .cloned()
            .collect();

        let file = RulesFile { rules: persistent };
        match toml::to_string_pretty(&file) {
            Ok(content) => {
                if let Some(parent) = self.path.parent() {
                    let _ = std::fs::create_dir_all(parent);
                }
                if let Err(e) = std::fs::write(&self.path, content) {
                    log::error!("failed to save rules to {}: {e}", self.path.display());
                }
            }
            Err(e) => {
                log::error!("failed to serialize rules: {e}");
            }
        }
    }
}

impl Default for RuleStore {
    fn default() -> Self {
        Self::new()
    }
}

/// Check whether a pattern (which may be a regex) matches a value.
///
/// First tries an exact case-insensitive match, then falls back to regex.
fn matches_pattern(pattern: &str, value: &str) -> bool {
    if pattern.eq_ignore_ascii_case(value) {
        return true;
    }
    match Regex::new(pattern) {
        Ok(re) => re.is_match(value),
        Err(_) => false,
    }
}

/// Default path for the rules file.
fn rules_path() -> PathBuf {
    dirs::home_dir()
        .map(|h| h.join(".config").join("jterm").join("allow_rules.toml"))
        .unwrap_or_else(|| PathBuf::from("allow_rules.toml"))
}

/// Load rules from a TOML file.
fn load_rules_from(path: &PathBuf) -> Vec<AllowRule> {
    match std::fs::read_to_string(path) {
        Ok(content) => match toml::from_str::<RulesFile>(&content) {
            Ok(file) => {
                log::info!(
                    "loaded {} allow rules from {}",
                    file.rules.len(),
                    path.display()
                );
                file.rules
            }
            Err(e) => {
                log::error!("failed to parse rules file {}: {e}", path.display());
                Vec::new()
            }
        },
        Err(_) => {
            log::debug!("no rules file at {}, starting empty", path.display());
            Vec::new()
        }
    }
}

// Custom Serialize/Deserialize for AllowDecision uses lowercase.
// We define them here since AllowDecision lives in request.rs but needs
// serde support for the rules TOML format.
impl std::fmt::Display for AllowDecision {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AllowDecision::Allow => write!(f, "allow"),
            AllowDecision::Deny => write!(f, "deny"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::request::DetectionSource;
    use std::io::Write;
    use tempfile::NamedTempFile;

    /// Helper to create a request for testing.
    fn test_request(tool: &str, action: &str) -> AllowRequest {
        AllowRequest::new(
            1,
            0,
            tool.into(),
            action.into(),
            "test".into(),
            DetectionSource::Regex,
            "y\n".into(),
            "n\n".into(),
        )
    }

    /// Create a rule store backed by a fresh temporary file.
    fn temp_store() -> (RuleStore, NamedTempFile) {
        let tmp = NamedTempFile::new().unwrap();
        // Truncate so it starts empty.
        std::fs::write(tmp.path(), "").unwrap();
        let store = RuleStore::with_path(tmp.path().to_path_buf());
        (store, tmp)
    }

    #[test]
    fn test_exact_match() {
        let (mut store, _tmp) = temp_store();
        store.add_rule(AllowRule {
            tool: "Claude Code".into(),
            action: "read file".into(),
            decision: AllowDecision::Allow,
            scope: RuleScope::Session,
        });

        let req = test_request("Claude Code", "read file");
        assert_eq!(store.match_request(&req), Some(AllowDecision::Allow));
    }

    #[test]
    fn test_regex_match() {
        let (mut store, _tmp) = temp_store();
        store.add_rule(AllowRule {
            tool: "Claude Code".into(),
            action: "execute.*".into(),
            decision: AllowDecision::Deny,
            scope: RuleScope::Session,
        });

        let req = test_request("Claude Code", "execute bash command");
        assert_eq!(store.match_request(&req), Some(AllowDecision::Deny));
    }

    #[test]
    fn test_no_match() {
        let (store, _tmp) = temp_store();
        let req = test_request("Codex", "write file");
        assert_eq!(store.match_request(&req), None);
    }

    #[test]
    fn test_case_insensitive_exact() {
        let (mut store, _tmp) = temp_store();
        store.add_rule(AllowRule {
            tool: "claude code".into(),
            action: "Read File".into(),
            decision: AllowDecision::Allow,
            scope: RuleScope::Session,
        });

        let req = test_request("Claude Code", "read file");
        assert_eq!(store.match_request(&req), Some(AllowDecision::Allow));
    }

    #[test]
    fn test_first_match_wins() {
        let (mut store, _tmp) = temp_store();
        store.add_rule(AllowRule {
            tool: "Claude Code".into(),
            action: "execute.*".into(),
            decision: AllowDecision::Allow,
            scope: RuleScope::Session,
        });
        store.add_rule(AllowRule {
            tool: "Claude Code".into(),
            action: "execute bash.*".into(),
            decision: AllowDecision::Deny,
            scope: RuleScope::Session,
        });

        let req = test_request("Claude Code", "execute bash command");
        assert_eq!(store.match_request(&req), Some(AllowDecision::Allow));
    }

    #[test]
    fn test_remove_rule() {
        let (mut store, _tmp) = temp_store();
        store.add_rule(AllowRule {
            tool: "Claude Code".into(),
            action: "read file".into(),
            decision: AllowDecision::Allow,
            scope: RuleScope::Session,
        });
        assert_eq!(store.list_rules().len(), 1);
        assert!(store.remove_rule(0));
        assert_eq!(store.list_rules().len(), 0);
        assert!(!store.remove_rule(0));
    }

    #[test]
    fn test_clear_session_rules() {
        let (mut store, _tmp) = temp_store();
        store.add_rule(AllowRule {
            tool: "Claude Code".into(),
            action: "read file".into(),
            decision: AllowDecision::Allow,
            scope: RuleScope::Session,
        });
        store.add_rule(AllowRule {
            tool: "Claude Code".into(),
            action: "write file".into(),
            decision: AllowDecision::Deny,
            scope: RuleScope::Persistent,
        });
        assert_eq!(store.list_rules().len(), 2);
        store.clear_session_rules();
        assert_eq!(store.list_rules().len(), 1);
        assert_eq!(store.list_rules()[0].action, "write file");
    }

    #[test]
    fn test_serialize_deserialize_toml() {
        let rules = RulesFile {
            rules: vec![
                AllowRule {
                    tool: "Claude Code".into(),
                    action: "read file".into(),
                    decision: AllowDecision::Allow,
                    scope: RuleScope::Persistent,
                },
                AllowRule {
                    tool: "Claude Code".into(),
                    action: "execute.*".into(),
                    decision: AllowDecision::Deny,
                    scope: RuleScope::Session,
                },
            ],
        };

        let serialized = toml::to_string_pretty(&rules).unwrap();
        assert!(serialized.contains("tool = \"Claude Code\""));
        assert!(serialized.contains("decision = \"allow\""));
        assert!(serialized.contains("decision = \"deny\""));

        let deserialized: RulesFile = toml::from_str(&serialized).unwrap();
        assert_eq!(deserialized.rules.len(), 2);
        assert_eq!(deserialized.rules[0].decision, AllowDecision::Allow);
        assert_eq!(deserialized.rules[1].decision, AllowDecision::Deny);
    }

    #[test]
    fn test_load_from_toml_file() {
        let toml_content = r#"
[[rules]]
tool = "Claude Code"
action = "read file"
decision = "allow"
scope = "persistent"

[[rules]]
tool = "Claude Code"
action = "execute.*"
decision = "deny"
scope = "session"
"#;
        let mut tmpfile = NamedTempFile::new().unwrap();
        write!(tmpfile, "{}", toml_content).unwrap();
        tmpfile.flush().unwrap();

        let store = RuleStore::with_path(tmpfile.path().to_path_buf());
        assert_eq!(store.list_rules().len(), 2);
        assert_eq!(store.list_rules()[0].tool, "Claude Code");
        assert_eq!(store.list_rules()[0].decision, AllowDecision::Allow);
        assert_eq!(store.list_rules()[1].scope, RuleScope::Session);
    }

    #[test]
    fn test_save_persistent_rules() {
        let tmpfile = NamedTempFile::new().unwrap();
        let path = tmpfile.path().to_path_buf();

        let mut store = RuleStore::with_path(path.clone());
        store.add_rule(AllowRule {
            tool: "Claude Code".into(),
            action: "read file".into(),
            decision: AllowDecision::Allow,
            scope: RuleScope::Persistent,
        });
        store.add_rule(AllowRule {
            tool: "Codex".into(),
            action: "execute".into(),
            decision: AllowDecision::Deny,
            scope: RuleScope::Session,
        });

        // Only persistent rules should be saved.
        let content = std::fs::read_to_string(&path).unwrap();
        assert!(content.contains("Claude Code"));
        // Session rule should NOT be persisted to file.
        assert!(!content.contains("Codex"));
    }
}
