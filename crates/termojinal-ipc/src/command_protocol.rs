//! JSON protocol types for command plugin communication.
//!
//! Commands are external scripts that communicate with termojinal via
//! line-delimited JSON over stdin/stdout. The protocol defines 7 message
//! types that a command can send (via its stdout) and 5 response types
//! that termojinal sends back (via the command's stdin).

use serde::{Deserialize, Serialize};

/// Message sent FROM a command script TO termojinal (via the script's stdout).
///
/// Each variant maps to a UI interaction that termojinal presents to the user.
/// The command writes one JSON line per message and then waits for termojinal
/// to respond before continuing (except for `Info`, `Done`, and `Error`
/// which are fire-and-forget).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type")]
#[serde(rename_all = "snake_case")]
pub enum CommandMessage {
    /// Show an incremental fuzzy-filter list. The user selects one item.
    Fuzzy {
        prompt: String,
        items: Vec<FuzzyItem>,
        #[serde(default)]
        preview: bool,
    },

    /// Show a multi-select fuzzy-filter list. The user selects one or more items.
    Multi {
        prompt: String,
        items: Vec<FuzzyItem>,
    },

    /// Show a yes/no confirmation dialog.
    Confirm {
        message: String,
        #[serde(default)]
        default: bool,
    },

    /// Show a free-text input field.
    Text {
        label: String,
        #[serde(default)]
        placeholder: String,
        #[serde(default)]
        default: String,
        #[serde(default)]
        completions: Vec<String>,
    },

    /// Show a progress/information message. No user interaction required;
    /// the command continues by sending the next message.
    Info { message: String },

    /// Signal that the command has finished successfully.
    /// An optional `notify` field triggers a macOS notification.
    Done {
        #[serde(default)]
        notify: Option<String>,
    },

    /// Signal that the command has encountered an error.
    Error { message: String },
}

/// A single item in a fuzzy/multi-select list.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct FuzzyItem {
    /// The value returned when this item is selected.
    pub value: String,

    /// Display text shown in the list (defaults to `value` if `None`).
    #[serde(default)]
    pub label: Option<String>,

    /// Secondary description text.
    #[serde(default)]
    pub description: Option<String>,

    /// Content shown in the preview pane.
    #[serde(default)]
    pub preview: Option<String>,

    /// SF Symbol name for the item icon.
    #[serde(default)]
    pub icon: Option<String>,
}

/// Response sent FROM termojinal TO the command script (via the script's stdin).
///
/// termojinal writes one JSON line per response after the user completes the
/// interaction requested by the corresponding [`CommandMessage`].
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type")]
#[serde(rename_all = "snake_case")]
pub enum CommandResponse {
    /// The user selected a single item from a `Fuzzy` list.
    Selected { value: String },

    /// The user selected one or more items from a `Multi` list.
    MultiSelected { values: Vec<String> },

    /// The user answered a `Confirm` dialog.
    Confirmed { yes: bool },

    /// The user submitted a `Text` input.
    TextInput { value: String },

    /// The user cancelled the current interaction (e.g. pressed Escape).
    Cancelled {},
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── CommandMessage serialization ────────────────────────────────

    #[test]
    fn test_serialize_fuzzy() {
        let msg = CommandMessage::Fuzzy {
            prompt: "Pick a branch".to_string(),
            items: vec![
                FuzzyItem {
                    value: "main".to_string(),
                    label: None,
                    description: Some("default branch".to_string()),
                    preview: None,
                    icon: None,
                },
                FuzzyItem {
                    value: "feature/foo".to_string(),
                    label: Some("feature/foo".to_string()),
                    description: None,
                    preview: Some("diff preview here".to_string()),
                    icon: Some("arrow.triangle.branch".to_string()),
                },
            ],
            preview: true,
        };
        let json = serde_json::to_string(&msg).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed["type"], "fuzzy");
        assert_eq!(parsed["prompt"], "Pick a branch");
        assert_eq!(parsed["preview"], true);
        assert_eq!(parsed["items"].as_array().unwrap().len(), 2);
        assert_eq!(parsed["items"][0]["value"], "main");
        assert_eq!(parsed["items"][1]["icon"], "arrow.triangle.branch");
    }

    #[test]
    fn test_serialize_multi() {
        let msg = CommandMessage::Multi {
            prompt: "Select files".to_string(),
            items: vec![FuzzyItem {
                value: "src/main.rs".to_string(),
                label: None,
                description: None,
                preview: None,
                icon: None,
            }],
        };
        let json = serde_json::to_string(&msg).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed["type"], "multi");
        assert_eq!(parsed["prompt"], "Select files");
    }

    #[test]
    fn test_serialize_confirm() {
        let msg = CommandMessage::Confirm {
            message: "Are you sure?".to_string(),
            default: true,
        };
        let json = serde_json::to_string(&msg).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed["type"], "confirm");
        assert_eq!(parsed["message"], "Are you sure?");
        assert_eq!(parsed["default"], true);
    }

    #[test]
    fn test_serialize_confirm_default_false() {
        let msg = CommandMessage::Confirm {
            message: "Delete?".to_string(),
            default: false,
        };
        let json = serde_json::to_string(&msg).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed["default"], false);
    }

    #[test]
    fn test_serialize_text() {
        let msg = CommandMessage::Text {
            label: "Branch name".to_string(),
            placeholder: "feature/...".to_string(),
            default: "feature/".to_string(),
            completions: vec!["feature/foo".to_string(), "feature/bar".to_string()],
        };
        let json = serde_json::to_string(&msg).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed["type"], "text");
        assert_eq!(parsed["label"], "Branch name");
        assert_eq!(parsed["placeholder"], "feature/...");
        assert_eq!(parsed["completions"].as_array().unwrap().len(), 2);
    }

    #[test]
    fn test_serialize_text_defaults() {
        let msg = CommandMessage::Text {
            label: "Name".to_string(),
            placeholder: String::new(),
            default: String::new(),
            completions: vec![],
        };
        let json = serde_json::to_string(&msg).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed["type"], "text");
        assert_eq!(parsed["label"], "Name");
    }

    #[test]
    fn test_serialize_info() {
        let msg = CommandMessage::Info {
            message: "Fetching PRs...".to_string(),
        };
        let json = serde_json::to_string(&msg).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed["type"], "info");
        assert_eq!(parsed["message"], "Fetching PRs...");
    }

    #[test]
    fn test_serialize_done() {
        let msg = CommandMessage::Done {
            notify: Some("PR review started".to_string()),
        };
        let json = serde_json::to_string(&msg).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed["type"], "done");
        assert_eq!(parsed["notify"], "PR review started");
    }

    #[test]
    fn test_serialize_done_no_notify() {
        let msg = CommandMessage::Done { notify: None };
        let json = serde_json::to_string(&msg).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed["type"], "done");
        assert_eq!(parsed["notify"], serde_json::Value::Null);
    }

    #[test]
    fn test_serialize_error() {
        let msg = CommandMessage::Error {
            message: "gh not found".to_string(),
        };
        let json = serde_json::to_string(&msg).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed["type"], "error");
        assert_eq!(parsed["message"], "gh not found");
    }

    // ── CommandMessage deserialization ──────────────────────────────

    #[test]
    fn test_deserialize_fuzzy() {
        let json = r#"{"type":"fuzzy","prompt":"Pick","items":[{"value":"a"}],"preview":false}"#;
        let msg: CommandMessage = serde_json::from_str(json).unwrap();
        match msg {
            CommandMessage::Fuzzy {
                prompt,
                items,
                preview,
            } => {
                assert_eq!(prompt, "Pick");
                assert_eq!(items.len(), 1);
                assert_eq!(items[0].value, "a");
                assert!(!preview);
            }
            _ => panic!("expected Fuzzy"),
        }
    }

    #[test]
    fn test_deserialize_fuzzy_preview_default() {
        // preview field omitted; should default to false.
        let json = r#"{"type":"fuzzy","prompt":"Pick","items":[]}"#;
        let msg: CommandMessage = serde_json::from_str(json).unwrap();
        match msg {
            CommandMessage::Fuzzy { preview, .. } => assert!(!preview),
            _ => panic!("expected Fuzzy"),
        }
    }

    #[test]
    fn test_deserialize_confirm_default() {
        // default field omitted; should default to false.
        let json = r#"{"type":"confirm","message":"OK?"}"#;
        let msg: CommandMessage = serde_json::from_str(json).unwrap();
        match msg {
            CommandMessage::Confirm { default, .. } => assert!(!default),
            _ => panic!("expected Confirm"),
        }
    }

    #[test]
    fn test_deserialize_text_minimal() {
        let json = r#"{"type":"text","label":"Name"}"#;
        let msg: CommandMessage = serde_json::from_str(json).unwrap();
        match msg {
            CommandMessage::Text {
                label,
                placeholder,
                default,
                completions,
            } => {
                assert_eq!(label, "Name");
                assert_eq!(placeholder, "");
                assert_eq!(default, "");
                assert!(completions.is_empty());
            }
            _ => panic!("expected Text"),
        }
    }

    #[test]
    fn test_deserialize_done_no_notify() {
        let json = r#"{"type":"done"}"#;
        let msg: CommandMessage = serde_json::from_str(json).unwrap();
        match msg {
            CommandMessage::Done { notify } => assert!(notify.is_none()),
            _ => panic!("expected Done"),
        }
    }

    // ── CommandResponse serialization ──────────────────────────────

    #[test]
    fn test_serialize_selected() {
        let resp = CommandResponse::Selected {
            value: "main".to_string(),
        };
        let json = serde_json::to_string(&resp).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed["type"], "selected");
        assert_eq!(parsed["value"], "main");
    }

    #[test]
    fn test_serialize_multi_selected() {
        let resp = CommandResponse::MultiSelected {
            values: vec!["a.rs".to_string(), "b.rs".to_string()],
        };
        let json = serde_json::to_string(&resp).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed["type"], "multi_selected");
        assert_eq!(parsed["values"].as_array().unwrap().len(), 2);
    }

    #[test]
    fn test_serialize_confirmed() {
        let resp = CommandResponse::Confirmed { yes: true };
        let json = serde_json::to_string(&resp).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed["type"], "confirmed");
        assert_eq!(parsed["yes"], true);
    }

    #[test]
    fn test_serialize_text_input() {
        let resp = CommandResponse::TextInput {
            value: "feature/new".to_string(),
        };
        let json = serde_json::to_string(&resp).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed["type"], "text_input");
        assert_eq!(parsed["value"], "feature/new");
    }

    #[test]
    fn test_serialize_cancelled() {
        let resp = CommandResponse::Cancelled {};
        let json = serde_json::to_string(&resp).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed["type"], "cancelled");
    }

    // ── CommandResponse deserialization ─────────────────────────────

    #[test]
    fn test_deserialize_selected() {
        let json = r#"{"type":"selected","value":"v1"}"#;
        let resp: CommandResponse = serde_json::from_str(json).unwrap();
        assert_eq!(
            resp,
            CommandResponse::Selected {
                value: "v1".to_string()
            }
        );
    }

    #[test]
    fn test_deserialize_multi_selected() {
        let json = r#"{"type":"multi_selected","values":["x","y"]}"#;
        let resp: CommandResponse = serde_json::from_str(json).unwrap();
        assert_eq!(
            resp,
            CommandResponse::MultiSelected {
                values: vec!["x".to_string(), "y".to_string()],
            }
        );
    }

    #[test]
    fn test_deserialize_confirmed() {
        let json = r#"{"type":"confirmed","yes":false}"#;
        let resp: CommandResponse = serde_json::from_str(json).unwrap();
        assert_eq!(resp, CommandResponse::Confirmed { yes: false });
    }

    #[test]
    fn test_deserialize_text_input() {
        let json = r#"{"type":"text_input","value":"hello"}"#;
        let resp: CommandResponse = serde_json::from_str(json).unwrap();
        assert_eq!(
            resp,
            CommandResponse::TextInput {
                value: "hello".to_string()
            }
        );
    }

    #[test]
    fn test_deserialize_cancelled() {
        let json = r#"{"type":"cancelled"}"#;
        let resp: CommandResponse = serde_json::from_str(json).unwrap();
        assert_eq!(resp, CommandResponse::Cancelled {});
    }

    // ── Round-trip tests ───────────────────────────────────────────

    #[test]
    fn test_roundtrip_all_messages() {
        let messages = vec![
            CommandMessage::Fuzzy {
                prompt: "Select".to_string(),
                items: vec![
                    FuzzyItem {
                        value: "v1".to_string(),
                        label: Some("Label 1".to_string()),
                        description: Some("Desc".to_string()),
                        preview: Some("Preview text".to_string()),
                        icon: Some("star".to_string()),
                    },
                    FuzzyItem {
                        value: "v2".to_string(),
                        label: None,
                        description: None,
                        preview: None,
                        icon: None,
                    },
                ],
                preview: true,
            },
            CommandMessage::Multi {
                prompt: "Multi".to_string(),
                items: vec![],
            },
            CommandMessage::Confirm {
                message: "OK?".to_string(),
                default: true,
            },
            CommandMessage::Confirm {
                message: "Delete?".to_string(),
                default: false,
            },
            CommandMessage::Text {
                label: "Input".to_string(),
                placeholder: "ph".to_string(),
                default: "def".to_string(),
                completions: vec!["c1".to_string()],
            },
            CommandMessage::Text {
                label: "Bare".to_string(),
                placeholder: String::new(),
                default: String::new(),
                completions: vec![],
            },
            CommandMessage::Info {
                message: "Loading...".to_string(),
            },
            CommandMessage::Done {
                notify: Some("Done!".to_string()),
            },
            CommandMessage::Done { notify: None },
            CommandMessage::Error {
                message: "oops".to_string(),
            },
        ];

        for msg in messages {
            let json = serde_json::to_string(&msg).unwrap();
            let deserialized: CommandMessage = serde_json::from_str(&json).unwrap();
            assert_eq!(msg, deserialized, "roundtrip failed for: {json}");
        }
    }

    #[test]
    fn test_roundtrip_all_responses() {
        let responses = vec![
            CommandResponse::Selected {
                value: "v".to_string(),
            },
            CommandResponse::MultiSelected {
                values: vec!["a".to_string(), "b".to_string()],
            },
            CommandResponse::Confirmed { yes: true },
            CommandResponse::Confirmed { yes: false },
            CommandResponse::TextInput {
                value: "text".to_string(),
            },
            CommandResponse::Cancelled {},
        ];

        for resp in responses {
            let json = serde_json::to_string(&resp).unwrap();
            let deserialized: CommandResponse = serde_json::from_str(&json).unwrap();
            assert_eq!(resp, deserialized, "roundtrip failed for: {json}");
        }
    }

    // ── FuzzyItem tests ────────────────────────────────────────────

    #[test]
    fn test_fuzzy_item_minimal() {
        let json = r#"{"value":"x"}"#;
        let item: FuzzyItem = serde_json::from_str(json).unwrap();
        assert_eq!(item.value, "x");
        assert!(item.label.is_none());
        assert!(item.description.is_none());
        assert!(item.preview.is_none());
        assert!(item.icon.is_none());
    }

    #[test]
    fn test_fuzzy_item_full() {
        let json =
            r#"{"value":"x","label":"X","description":"desc","preview":"prev","icon":"star"}"#;
        let item: FuzzyItem = serde_json::from_str(json).unwrap();
        assert_eq!(item.value, "x");
        assert_eq!(item.label.as_deref(), Some("X"));
        assert_eq!(item.description.as_deref(), Some("desc"));
        assert_eq!(item.preview.as_deref(), Some("prev"));
        assert_eq!(item.icon.as_deref(), Some("star"));
    }
}
