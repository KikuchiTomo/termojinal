//! App-level IPC protocol for controlling the termojinal GUI.
//!
//! These types are used for communication between external tools (MCP server,
//! CLI) and the running termojinal GUI application via the app Unix socket.

use serde::{Deserialize, Serialize};

/// Request sent to the termojinal GUI app.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AppIpcRequest {
    // --- Status ---
    /// Check that the app is alive.
    Ping,
    /// Get current app status (workspaces, tabs, panes, etc.).
    GetStatus,
    /// Get the current configuration.
    GetConfig,

    // --- Workspace ---
    /// List all workspaces.
    ListWorkspaces,
    /// Create a new workspace.
    CreateWorkspace {
        name: Option<String>,
        cwd: Option<String>,
    },
    /// Switch to a workspace by index.
    SwitchWorkspace { index: usize },
    /// Close a workspace by index.
    CloseWorkspace { index: usize },

    // --- Tab ---
    /// List tabs in a workspace (defaults to active workspace).
    ListTabs { workspace: Option<usize> },
    /// Create a new tab.
    CreateTab { workspace: Option<usize> },
    /// Switch to a tab by index.
    SwitchTab {
        workspace: Option<usize>,
        index: usize,
    },
    /// Close a tab by index.
    CloseTab {
        workspace: Option<usize>,
        index: usize,
    },

    // --- Pane ---
    /// List panes in a tab.
    ListPanes {
        workspace: Option<usize>,
        tab: Option<usize>,
    },
    /// Split a pane.
    SplitPane {
        /// "horizontal" or "vertical"
        direction: String,
        pane_id: Option<u64>,
    },
    /// Close a pane.
    ClosePane { pane_id: Option<u64> },
    /// Focus a specific pane.
    FocusPane { pane_id: u64 },
    /// Toggle zoom on a pane.
    ZoomPane { pane_id: Option<u64> },

    // --- Terminal ---
    /// Send keystrokes to a pane.
    SendKeys {
        pane_id: Option<u64>,
        keys: String,
    },
    /// Run a command in a pane.
    RunCommand {
        pane_id: Option<u64>,
        command: String,
    },
    /// Get the visible terminal content of a pane.
    GetTerminalContent { pane_id: Option<u64> },
    /// Get scrollback buffer content.
    GetScrollback {
        pane_id: Option<u64>,
        lines: Option<usize>,
    },

    // --- Allow Flow ---
    /// Permission request from Claude Code PermissionRequest hook.
    /// The connection stays open until the user makes a decision.
    PermissionRequest {
        /// Tool name (e.g. "Bash", "Edit", "Write").
        tool_name: String,
        /// Tool input parameters as raw JSON.
        tool_input: serde_json::Value,
        /// Claude Code session ID for correlation.
        #[serde(default)]
        session_id: Option<String>,
    },
    /// List pending approval requests.
    ListPendingRequests { workspace: Option<usize> },
    /// Approve a pending request by ID.
    ApproveRequest { request_id: u64 },
    /// Deny a pending request by ID.
    DenyRequest { request_id: u64 },
    /// Approve all pending requests in a workspace.
    ApproveAll { workspace: usize },

    // --- Notification ---
    /// Send a notification to the app.
    Notify {
        title: Option<String>,
        body: Option<String>,
        subtitle: Option<String>,
        #[serde(default)]
        notification_type: Option<String>,
    },

    // --- Time Travel ---
    /// Get the command history for a pane.
    GetCommandHistory {
        pane_id: Option<u64>,
        /// Maximum number of records to return (newest first).
        limit: Option<usize>,
    },
    /// Jump to a specific command by ID.
    JumpToCommand {
        pane_id: Option<u64>,
        command_id: u64,
    },
    /// Open/close the command timeline UI.
    ToggleTimeline,

    // --- Legacy ---
    /// Toggle the quick terminal overlay.
    ToggleQuickTerminal,
    /// Show the command palette.
    ShowPalette,
}

/// Response from the termojinal GUI app.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AppIpcResponse {
    /// Whether the request was processed successfully.
    pub success: bool,

    /// Optional JSON payload (varies by request type).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<serde_json::Value>,

    /// Error message if `success` is false.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

impl AppIpcResponse {
    /// Create a successful response with data.
    pub fn ok(data: serde_json::Value) -> Self {
        Self {
            success: true,
            data: Some(data),
            error: None,
        }
    }

    /// Create a successful response with no data.
    pub fn ok_empty() -> Self {
        Self {
            success: true,
            data: None,
            error: None,
        }
    }

    /// Create an error response.
    pub fn err(msg: impl Into<String>) -> Self {
        Self {
            success: false,
            data: None,
            error: Some(msg.into()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    // --- Serialization format verification ---

    #[test]
    fn test_serialize_ping() {
        let req = AppIpcRequest::Ping;
        let json = serde_json::to_string(&req).unwrap();
        assert_eq!(json, r#"{"type":"ping"}"#);
    }

    #[test]
    fn test_serialize_get_status() {
        let req = AppIpcRequest::GetStatus;
        let json = serde_json::to_string(&req).unwrap();
        assert_eq!(json, r#"{"type":"get_status"}"#);
    }

    #[test]
    fn test_serialize_get_config() {
        let req = AppIpcRequest::GetConfig;
        let json = serde_json::to_string(&req).unwrap();
        assert_eq!(json, r#"{"type":"get_config"}"#);
    }

    #[test]
    fn test_serialize_list_workspaces() {
        let req = AppIpcRequest::ListWorkspaces;
        let json = serde_json::to_string(&req).unwrap();
        assert_eq!(json, r#"{"type":"list_workspaces"}"#);
    }

    #[test]
    fn test_serialize_create_workspace_full() {
        let req = AppIpcRequest::CreateWorkspace {
            name: Some("dev".to_string()),
            cwd: Some("/home/user/project".to_string()),
        };
        let json = serde_json::to_string(&req).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed["type"], "create_workspace");
        assert_eq!(parsed["name"], "dev");
        assert_eq!(parsed["cwd"], "/home/user/project");
    }

    #[test]
    fn test_serialize_create_workspace_defaults() {
        let req = AppIpcRequest::CreateWorkspace {
            name: None,
            cwd: None,
        };
        let json = serde_json::to_string(&req).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed["type"], "create_workspace");
        assert_eq!(parsed["name"], serde_json::Value::Null);
        assert_eq!(parsed["cwd"], serde_json::Value::Null);
    }

    #[test]
    fn test_serialize_switch_workspace() {
        let req = AppIpcRequest::SwitchWorkspace { index: 2 };
        let json = serde_json::to_string(&req).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed["type"], "switch_workspace");
        assert_eq!(parsed["index"], 2);
    }

    #[test]
    fn test_serialize_close_workspace() {
        let req = AppIpcRequest::CloseWorkspace { index: 0 };
        let json = serde_json::to_string(&req).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed["type"], "close_workspace");
        assert_eq!(parsed["index"], 0);
    }

    #[test]
    fn test_serialize_list_tabs() {
        let req = AppIpcRequest::ListTabs {
            workspace: Some(1),
        };
        let json = serde_json::to_string(&req).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed["type"], "list_tabs");
        assert_eq!(parsed["workspace"], 1);
    }

    #[test]
    fn test_serialize_create_tab() {
        let req = AppIpcRequest::CreateTab { workspace: None };
        let json = serde_json::to_string(&req).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed["type"], "create_tab");
        assert_eq!(parsed["workspace"], serde_json::Value::Null);
    }

    #[test]
    fn test_serialize_switch_tab() {
        let req = AppIpcRequest::SwitchTab {
            workspace: Some(0),
            index: 3,
        };
        let json = serde_json::to_string(&req).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed["type"], "switch_tab");
        assert_eq!(parsed["workspace"], 0);
        assert_eq!(parsed["index"], 3);
    }

    #[test]
    fn test_serialize_close_tab() {
        let req = AppIpcRequest::CloseTab {
            workspace: None,
            index: 1,
        };
        let json = serde_json::to_string(&req).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed["type"], "close_tab");
        assert_eq!(parsed["index"], 1);
    }

    #[test]
    fn test_serialize_list_panes() {
        let req = AppIpcRequest::ListPanes {
            workspace: Some(0),
            tab: Some(1),
        };
        let json = serde_json::to_string(&req).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed["type"], "list_panes");
        assert_eq!(parsed["workspace"], 0);
        assert_eq!(parsed["tab"], 1);
    }

    #[test]
    fn test_serialize_split_pane() {
        let req = AppIpcRequest::SplitPane {
            direction: "horizontal".to_string(),
            pane_id: Some(42),
        };
        let json = serde_json::to_string(&req).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed["type"], "split_pane");
        assert_eq!(parsed["direction"], "horizontal");
        assert_eq!(parsed["pane_id"], 42);
    }

    #[test]
    fn test_serialize_close_pane() {
        let req = AppIpcRequest::ClosePane { pane_id: None };
        let json = serde_json::to_string(&req).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed["type"], "close_pane");
        assert_eq!(parsed["pane_id"], serde_json::Value::Null);
    }

    #[test]
    fn test_serialize_focus_pane() {
        let req = AppIpcRequest::FocusPane { pane_id: 7 };
        let json = serde_json::to_string(&req).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed["type"], "focus_pane");
        assert_eq!(parsed["pane_id"], 7);
    }

    #[test]
    fn test_serialize_zoom_pane() {
        let req = AppIpcRequest::ZoomPane { pane_id: Some(3) };
        let json = serde_json::to_string(&req).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed["type"], "zoom_pane");
        assert_eq!(parsed["pane_id"], 3);
    }

    #[test]
    fn test_serialize_send_keys() {
        let req = AppIpcRequest::SendKeys {
            pane_id: Some(1),
            keys: "ls -la\n".to_string(),
        };
        let json = serde_json::to_string(&req).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed["type"], "send_keys");
        assert_eq!(parsed["pane_id"], 1);
        assert_eq!(parsed["keys"], "ls -la\n");
    }

    #[test]
    fn test_serialize_run_command() {
        let req = AppIpcRequest::RunCommand {
            pane_id: None,
            command: "cargo build".to_string(),
        };
        let json = serde_json::to_string(&req).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed["type"], "run_command");
        assert_eq!(parsed["command"], "cargo build");
    }

    #[test]
    fn test_serialize_get_terminal_content() {
        let req = AppIpcRequest::GetTerminalContent { pane_id: Some(5) };
        let json = serde_json::to_string(&req).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed["type"], "get_terminal_content");
        assert_eq!(parsed["pane_id"], 5);
    }

    #[test]
    fn test_serialize_get_scrollback() {
        let req = AppIpcRequest::GetScrollback {
            pane_id: Some(2),
            lines: Some(100),
        };
        let json = serde_json::to_string(&req).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed["type"], "get_scrollback");
        assert_eq!(parsed["pane_id"], 2);
        assert_eq!(parsed["lines"], 100);
    }

    #[test]
    fn test_serialize_list_pending_requests() {
        let req = AppIpcRequest::ListPendingRequests {
            workspace: Some(0),
        };
        let json = serde_json::to_string(&req).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed["type"], "list_pending_requests");
        assert_eq!(parsed["workspace"], 0);
    }

    #[test]
    fn test_serialize_approve_request() {
        let req = AppIpcRequest::ApproveRequest { request_id: 99 };
        let json = serde_json::to_string(&req).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed["type"], "approve_request");
        assert_eq!(parsed["request_id"], 99);
    }

    #[test]
    fn test_serialize_deny_request() {
        let req = AppIpcRequest::DenyRequest { request_id: 50 };
        let json = serde_json::to_string(&req).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed["type"], "deny_request");
        assert_eq!(parsed["request_id"], 50);
    }

    #[test]
    fn test_serialize_approve_all() {
        let req = AppIpcRequest::ApproveAll { workspace: 1 };
        let json = serde_json::to_string(&req).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed["type"], "approve_all");
        assert_eq!(parsed["workspace"], 1);
    }

    #[test]
    fn test_serialize_notify() {
        let req = AppIpcRequest::Notify {
            title: Some("Claude Code".to_string()),
            body: Some("Task complete".to_string()),
            subtitle: None,
            notification_type: Some("permission_prompt".to_string()),
        };
        let json = serde_json::to_string(&req).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed["type"], "notify");
        assert_eq!(parsed["title"], "Claude Code");
        assert_eq!(parsed["body"], "Task complete");
        assert_eq!(parsed["notification_type"], "permission_prompt");
    }

    #[test]
    fn test_deserialize_notify_minimal() {
        let req: AppIpcRequest =
            serde_json::from_str(r#"{"type":"notify"}"#).unwrap();
        assert_eq!(
            req,
            AppIpcRequest::Notify {
                title: None,
                body: None,
                subtitle: None,
                notification_type: None,
            }
        );
    }

    #[test]
    fn test_serialize_toggle_quick_terminal() {
        let req = AppIpcRequest::ToggleQuickTerminal;
        let json = serde_json::to_string(&req).unwrap();
        assert_eq!(json, r#"{"type":"toggle_quick_terminal"}"#);
    }

    #[test]
    fn test_serialize_show_palette() {
        let req = AppIpcRequest::ShowPalette;
        let json = serde_json::to_string(&req).unwrap();
        assert_eq!(json, r#"{"type":"show_palette"}"#);
    }

    // --- Deserialization ---

    #[test]
    fn test_deserialize_ping() {
        let req: AppIpcRequest = serde_json::from_str(r#"{"type":"ping"}"#).unwrap();
        assert_eq!(req, AppIpcRequest::Ping);
    }

    #[test]
    fn test_deserialize_send_keys() {
        let req: AppIpcRequest =
            serde_json::from_str(r#"{"type":"send_keys","pane_id":1,"keys":"hello"}"#).unwrap();
        assert_eq!(
            req,
            AppIpcRequest::SendKeys {
                pane_id: Some(1),
                keys: "hello".to_string(),
            }
        );
    }

    // --- Response construction helpers ---

    #[test]
    fn test_response_ok() {
        let resp = AppIpcResponse::ok(json!({"workspaces": ["default", "dev"]}));
        assert!(resp.success);
        assert!(resp.data.is_some());
        assert!(resp.error.is_none());

        let json = serde_json::to_string(&resp).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed["success"], true);
        assert!(parsed.get("error").is_none()); // skip_serializing_if
    }

    #[test]
    fn test_response_ok_empty() {
        let resp = AppIpcResponse::ok_empty();
        assert!(resp.success);
        assert!(resp.data.is_none());
        assert!(resp.error.is_none());

        let json = serde_json::to_string(&resp).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed["success"], true);
        assert!(parsed.get("data").is_none());
        assert!(parsed.get("error").is_none());
    }

    #[test]
    fn test_response_err() {
        let resp = AppIpcResponse::err("workspace not found");
        assert!(!resp.success);
        assert!(resp.data.is_none());
        assert_eq!(resp.error.as_deref(), Some("workspace not found"));

        let json = serde_json::to_string(&resp).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed["success"], false);
        assert_eq!(parsed["error"], "workspace not found");
        assert!(parsed.get("data").is_none());
    }

    // --- Roundtrip tests ---

    #[test]
    fn test_roundtrip_all_requests() {
        let requests = vec![
            // Status
            AppIpcRequest::Ping,
            AppIpcRequest::GetStatus,
            AppIpcRequest::GetConfig,
            // Workspace
            AppIpcRequest::ListWorkspaces,
            AppIpcRequest::CreateWorkspace {
                name: Some("test".to_string()),
                cwd: Some("/tmp".to_string()),
            },
            AppIpcRequest::CreateWorkspace {
                name: None,
                cwd: None,
            },
            AppIpcRequest::SwitchWorkspace { index: 0 },
            AppIpcRequest::CloseWorkspace { index: 1 },
            // Tab
            AppIpcRequest::ListTabs {
                workspace: Some(0),
            },
            AppIpcRequest::ListTabs { workspace: None },
            AppIpcRequest::CreateTab {
                workspace: Some(1),
            },
            AppIpcRequest::SwitchTab {
                workspace: Some(0),
                index: 2,
            },
            AppIpcRequest::CloseTab {
                workspace: None,
                index: 0,
            },
            // Pane
            AppIpcRequest::ListPanes {
                workspace: Some(0),
                tab: Some(1),
            },
            AppIpcRequest::SplitPane {
                direction: "horizontal".to_string(),
                pane_id: Some(10),
            },
            AppIpcRequest::SplitPane {
                direction: "vertical".to_string(),
                pane_id: None,
            },
            AppIpcRequest::ClosePane {
                pane_id: Some(42),
            },
            AppIpcRequest::ClosePane { pane_id: None },
            AppIpcRequest::FocusPane { pane_id: 5 },
            AppIpcRequest::ZoomPane { pane_id: Some(3) },
            AppIpcRequest::ZoomPane { pane_id: None },
            // Terminal
            AppIpcRequest::SendKeys {
                pane_id: Some(1),
                keys: "hello\n".to_string(),
            },
            AppIpcRequest::RunCommand {
                pane_id: None,
                command: "cargo test".to_string(),
            },
            AppIpcRequest::GetTerminalContent { pane_id: Some(2) },
            AppIpcRequest::GetScrollback {
                pane_id: Some(1),
                lines: Some(500),
            },
            AppIpcRequest::GetScrollback {
                pane_id: None,
                lines: None,
            },
            // Allow Flow
            AppIpcRequest::PermissionRequest {
                tool_name: "Bash".to_string(),
                tool_input: json!({"command": "cargo test"}),
                session_id: Some("abc123".to_string()),
            },
            AppIpcRequest::PermissionRequest {
                tool_name: "Edit".to_string(),
                tool_input: json!({}),
                session_id: None,
            },
            AppIpcRequest::ListPendingRequests {
                workspace: Some(0),
            },
            AppIpcRequest::ApproveRequest { request_id: 1 },
            AppIpcRequest::DenyRequest { request_id: 2 },
            AppIpcRequest::ApproveAll { workspace: 0 },
            // Notification
            AppIpcRequest::Notify {
                title: Some("Claude Code".to_string()),
                body: Some("Task complete".to_string()),
                subtitle: Some("sub".to_string()),
                notification_type: Some("permission_prompt".to_string()),
            },
            AppIpcRequest::Notify {
                title: None,
                body: None,
                subtitle: None,
                notification_type: None,
            },
            // Legacy
            AppIpcRequest::ToggleQuickTerminal,
            AppIpcRequest::ShowPalette,
        ];

        for req in requests {
            let json = serde_json::to_string(&req).unwrap();
            let deserialized: AppIpcRequest = serde_json::from_str(&json).unwrap();
            assert_eq!(req, deserialized, "roundtrip failed for: {json}");
        }
    }

    #[test]
    fn test_roundtrip_responses() {
        let responses = vec![
            AppIpcResponse::ok(json!({"key": "value"})),
            AppIpcResponse::ok(json!([])),
            AppIpcResponse::ok(json!(42)),
            AppIpcResponse::ok_empty(),
            AppIpcResponse::err("something went wrong"),
        ];

        for resp in responses {
            let json = serde_json::to_string(&resp).unwrap();
            let deserialized: AppIpcResponse = serde_json::from_str(&json).unwrap();
            assert_eq!(resp, deserialized, "roundtrip failed for: {json}");
        }
    }

    // --- JSON format verification (tagged enum) ---

    #[test]
    fn test_tagged_enum_type_field() {
        // Verify that each variant produces the correct snake_case "type" field
        let cases: Vec<(AppIpcRequest, &str)> = vec![
            (AppIpcRequest::Ping, "ping"),
            (AppIpcRequest::GetStatus, "get_status"),
            (AppIpcRequest::GetConfig, "get_config"),
            (AppIpcRequest::ListWorkspaces, "list_workspaces"),
            (
                AppIpcRequest::CreateWorkspace {
                    name: None,
                    cwd: None,
                },
                "create_workspace",
            ),
            (AppIpcRequest::SwitchWorkspace { index: 0 }, "switch_workspace"),
            (AppIpcRequest::CloseWorkspace { index: 0 }, "close_workspace"),
            (AppIpcRequest::ListTabs { workspace: None }, "list_tabs"),
            (AppIpcRequest::CreateTab { workspace: None }, "create_tab"),
            (
                AppIpcRequest::SwitchTab {
                    workspace: None,
                    index: 0,
                },
                "switch_tab",
            ),
            (
                AppIpcRequest::CloseTab {
                    workspace: None,
                    index: 0,
                },
                "close_tab",
            ),
            (
                AppIpcRequest::ListPanes {
                    workspace: None,
                    tab: None,
                },
                "list_panes",
            ),
            (
                AppIpcRequest::SplitPane {
                    direction: "h".to_string(),
                    pane_id: None,
                },
                "split_pane",
            ),
            (AppIpcRequest::ClosePane { pane_id: None }, "close_pane"),
            (AppIpcRequest::FocusPane { pane_id: 0 }, "focus_pane"),
            (AppIpcRequest::ZoomPane { pane_id: None }, "zoom_pane"),
            (
                AppIpcRequest::SendKeys {
                    pane_id: None,
                    keys: "".to_string(),
                },
                "send_keys",
            ),
            (
                AppIpcRequest::RunCommand {
                    pane_id: None,
                    command: "".to_string(),
                },
                "run_command",
            ),
            (
                AppIpcRequest::GetTerminalContent { pane_id: None },
                "get_terminal_content",
            ),
            (
                AppIpcRequest::GetScrollback {
                    pane_id: None,
                    lines: None,
                },
                "get_scrollback",
            ),
            (
                AppIpcRequest::PermissionRequest {
                    tool_name: "Bash".into(),
                    tool_input: json!({}),
                    session_id: None,
                },
                "permission_request",
            ),
            (
                AppIpcRequest::ListPendingRequests { workspace: None },
                "list_pending_requests",
            ),
            (AppIpcRequest::ApproveRequest { request_id: 0 }, "approve_request"),
            (AppIpcRequest::DenyRequest { request_id: 0 }, "deny_request"),
            (AppIpcRequest::ApproveAll { workspace: 0 }, "approve_all"),
            (
                AppIpcRequest::Notify {
                    title: None,
                    body: None,
                    subtitle: None,
                    notification_type: None,
                },
                "notify",
            ),
            (AppIpcRequest::ToggleQuickTerminal, "toggle_quick_terminal"),
            (AppIpcRequest::ShowPalette, "show_palette"),
        ];

        for (req, expected_type) in cases {
            let json = serde_json::to_string(&req).unwrap();
            let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
            assert_eq!(
                parsed["type"].as_str().unwrap(),
                expected_type,
                "wrong type tag for variant that should be '{expected_type}'"
            );
        }
    }
}
