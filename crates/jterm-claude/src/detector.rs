//! Detection engine for AI tool permission prompts.
//!
//! Scans terminal output (both OSC notifications and visible lines) to detect
//! when an AI tool is requesting human permission.

use regex::Regex;
use serde::{Deserialize, Serialize};

use crate::request::{AllowRequest, DetectionSource};

/// A compiled detection pattern.
#[derive(Debug, Clone)]
pub struct DetectionPattern {
    /// Name of the tool this pattern matches (e.g. "Claude Code").
    pub tool_name: String,
    /// Human-readable description of the action (e.g. "tool use").
    pub action: String,
    /// The compiled regex.
    pub regex: Regex,
    /// String to write to the PTY to approve.
    pub yes_response: String,
    /// String to write to the PTY to deny.
    pub no_response: String,
}

/// A user-configurable pattern definition (for TOML config).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PatternConfig {
    /// Name of the tool this pattern matches.
    pub tool: String,
    /// Human-readable description of the action.
    pub action: String,
    /// Regex pattern string.
    pub pattern: String,
    /// String to write to approve.
    #[serde(default = "default_yes")]
    pub yes_response: String,
    /// String to write to deny.
    #[serde(default = "default_no")]
    pub no_response: String,
}

fn default_yes() -> String {
    "y\n".into()
}
fn default_no() -> String {
    "n\n".into()
}

/// The Allow Flow detection engine.
///
/// Holds compiled regex patterns and scans terminal output for permission prompts.
pub struct AllowFlowDetector {
    patterns: Vec<DetectionPattern>,
}

impl AllowFlowDetector {
    /// Create a new detector with built-in patterns plus optional custom patterns.
    pub fn new(custom_patterns: &[PatternConfig]) -> Self {
        let mut patterns = builtin_patterns();

        for pc in custom_patterns {
            match Regex::new(&pc.pattern) {
                Ok(regex) => {
                    patterns.push(DetectionPattern {
                        tool_name: pc.tool.clone(),
                        action: pc.action.clone(),
                        regex,
                        yes_response: pc.yes_response.clone(),
                        no_response: pc.no_response.clone(),
                    });
                }
                Err(e) => {
                    log::warn!("invalid custom pattern '{}': {e}", pc.pattern);
                }
            }
        }

        Self { patterns }
    }

    /// Scan an OSC notification string for a permission prompt.
    ///
    /// OSC 9/99/777 notifications are sometimes used by AI tools to signal
    /// that they need permission. The notification text is matched against
    /// all known patterns.
    pub fn scan_osc(
        &self,
        pane_id: u64,
        workspace_idx: usize,
        notification: &str,
    ) -> Option<AllowRequest> {
        for pat in &self.patterns {
            if let Some(caps) = pat.regex.captures(notification) {
                let detail = caps
                    .get(1)
                    .map(|m| m.as_str().to_string())
                    .unwrap_or_else(|| caps.get(0).unwrap().as_str().to_string());

                return Some(AllowRequest::new(
                    pane_id,
                    workspace_idx,
                    pat.tool_name.clone(),
                    pat.action.clone(),
                    detail,
                    DetectionSource::Osc,
                    pat.yes_response.clone(),
                    pat.no_response.clone(),
                ));
            }
        }
        None
    }

    /// Scan visible terminal lines for a permission prompt.
    ///
    /// Returns the first match found across all lines and patterns.
    pub fn scan_output(
        &self,
        pane_id: u64,
        workspace_idx: usize,
        lines: &[&str],
    ) -> Option<AllowRequest> {
        for line in lines {
            for pat in &self.patterns {
                if let Some(caps) = pat.regex.captures(line) {
                    let detail = caps
                        .get(1)
                        .map(|m| m.as_str().to_string())
                        .unwrap_or_else(|| caps.get(0).unwrap().as_str().to_string());

                    return Some(AllowRequest::new(
                        pane_id,
                        workspace_idx,
                        pat.tool_name.clone(),
                        pat.action.clone(),
                        detail,
                        DetectionSource::Regex,
                        pat.yes_response.clone(),
                        pat.no_response.clone(),
                    ));
                }
            }
        }
        None
    }

    /// Return the number of active patterns (built-in + custom).
    pub fn pattern_count(&self) -> usize {
        self.patterns.len()
    }
}

/// Build the set of built-in detection patterns.
fn builtin_patterns() -> Vec<DetectionPattern> {
    let specs: &[(&str, &str, &str, &str, &str)] = &[
        // Claude Code permission prompts
        (
            "Claude Code",
            "tool use",
            r"(?i)Do you want to (.+)\?",
            "y\n",
            "n\n",
        ),
        (
            "Claude Code",
            "tool use",
            r"(?i)Allow (.+) to run",
            "y\n",
            "n\n",
        ),
        (
            "Claude Code",
            "tool use",
            r"(?i)Allow (.+) to execute",
            "y\n",
            "n\n",
        ),
        (
            "Claude Code",
            "tool use",
            r"(?i)Allow (.+) to write",
            "y\n",
            "n\n",
        ),
        (
            "Claude Code",
            "tool use",
            r"(?i)Allow (.+) to read",
            "y\n",
            "n\n",
        ),
        // Generic Y/N prompts
        (
            "generic",
            "confirmation",
            r"\(Y\)es/\(N\)o",
            "Y\n",
            "N\n",
        ),
        (
            "generic",
            "confirmation",
            r"\[y/N\]",
            "y\n",
            "n\n",
        ),
        (
            "generic",
            "confirmation",
            r"\[Y/n\]",
            "y\n",
            "n\n",
        ),
    ];

    specs
        .iter()
        .map(|(tool, action, pattern, yes, no)| DetectionPattern {
            tool_name: tool.to_string(),
            action: action.to_string(),
            regex: Regex::new(pattern).expect("built-in pattern must compile"),
            yes_response: yes.to_string(),
            no_response: no.to_string(),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn detector() -> AllowFlowDetector {
        AllowFlowDetector::new(&[])
    }

    #[test]
    fn test_detect_claude_code_do_you_want() {
        let d = detector();
        let lines = &["Do you want to execute bash command?"];
        let req = d.scan_output(1, 0, lines);
        assert!(req.is_some());
        let req = req.unwrap();
        assert_eq!(req.tool_name, "Claude Code");
        assert_eq!(req.detail, "execute bash command");
        assert_eq!(req.source, DetectionSource::Regex);
    }

    #[test]
    fn test_detect_claude_code_allow_to_run() {
        let d = detector();
        let lines = &["Allow Claude Code to run 'cargo build'"];
        let req = d.scan_output(1, 0, lines);
        assert!(req.is_some());
        let req = req.unwrap();
        assert_eq!(req.tool_name, "Claude Code");
        assert!(req.detail.contains("Claude Code"));
    }

    #[test]
    fn test_detect_generic_yn_bracket() {
        let d = detector();
        let lines = &["Proceed with installation? [y/N]"];
        let req = d.scan_output(1, 0, lines);
        assert!(req.is_some());
        let req = req.unwrap();
        assert_eq!(req.tool_name, "generic");
        assert_eq!(req.action, "confirmation");
        assert_eq!(req.yes_response, "y\n");
    }

    #[test]
    fn test_detect_generic_yes_no_paren() {
        let d = detector();
        let lines = &["Delete all files? (Y)es/(N)o"];
        let req = d.scan_output(1, 0, lines);
        assert!(req.is_some());
        let req = req.unwrap();
        assert_eq!(req.tool_name, "generic");
        assert_eq!(req.yes_response, "Y\n");
    }

    #[test]
    fn test_no_match_plain_text() {
        let d = detector();
        let lines = &["Hello world", "Building project...", "Done."];
        assert!(d.scan_output(1, 0, lines).is_none());
    }

    #[test]
    fn test_osc_detection() {
        let d = detector();
        let req = d.scan_osc(2, 1, "Do you want to write file main.rs?");
        assert!(req.is_some());
        let req = req.unwrap();
        assert_eq!(req.source, DetectionSource::Osc);
        assert_eq!(req.pane_id, 2);
        assert_eq!(req.workspace_idx, 1);
    }

    #[test]
    fn test_osc_no_match() {
        let d = detector();
        assert!(d.scan_osc(1, 0, "Build completed successfully").is_none());
    }

    #[test]
    fn test_custom_pattern() {
        let custom = vec![PatternConfig {
            tool: "Aider".into(),
            action: "edit file".into(),
            pattern: r"(?i)aider wants to edit (.+)".into(),
            yes_response: "yes\n".into(),
            no_response: "no\n".into(),
        }];
        let d = AllowFlowDetector::new(&custom);
        let lines = &["Aider wants to edit src/main.rs"];
        let req = d.scan_output(1, 0, lines);
        assert!(req.is_some());
        let req = req.unwrap();
        assert_eq!(req.tool_name, "Aider");
        assert_eq!(req.detail, "src/main.rs");
        assert_eq!(req.yes_response, "yes\n");
    }

    #[test]
    fn test_builtin_pattern_count() {
        let d = detector();
        assert!(d.pattern_count() >= 8);
    }
}
