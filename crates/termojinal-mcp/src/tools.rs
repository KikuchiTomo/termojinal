//! MCP tool definitions and dispatch.
//!
//! Each tool maps to an `AppIpcRequest` sent over the Unix socket to the
//! running termojinal GUI.  The tool handler builds the request JSON,
//! forwards it via [`AppClient`], and translates the response into an
//! MCP [`ToolResult`].

use crate::client::AppClient;
use crate::mcp_types::{Tool, ToolResult};
use serde_json::json;

/// Return the full list of tools this MCP server exposes.
pub fn list_tools() -> Vec<Tool> {
    vec![
        Tool {
            name: "list_workspaces".into(),
            description: "List all workspaces with their state".into(),
            input_schema: json!({ "type": "object", "properties": {} }),
        },
        Tool {
            name: "create_workspace".into(),
            description: "Create a new workspace".into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "name": { "type": "string", "description": "Workspace name" },
                    "cwd": { "type": "string", "description": "Working directory" }
                }
            }),
        },
        Tool {
            name: "switch_workspace".into(),
            description: "Switch to a workspace by index".into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "index": { "type": "integer", "description": "Workspace index (0-based)" }
                },
                "required": ["index"]
            }),
        },
        Tool {
            name: "close_workspace".into(),
            description: "Close a workspace".into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "index": { "type": "integer" }
                },
                "required": ["index"]
            }),
        },
        Tool {
            name: "create_tab".into(),
            description: "Create a new tab in a workspace".into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "workspace": { "type": "integer", "description": "Workspace index (default: active)" }
                }
            }),
        },
        Tool {
            name: "list_tabs".into(),
            description: "List tabs in a workspace".into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "workspace": { "type": "integer" }
                }
            }),
        },
        Tool {
            name: "switch_tab".into(),
            description: "Switch to a tab by index".into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "workspace": { "type": "integer" },
                    "index": { "type": "integer" }
                },
                "required": ["index"]
            }),
        },
        Tool {
            name: "close_tab".into(),
            description: "Close a tab".into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "workspace": { "type": "integer" },
                    "index": { "type": "integer" }
                },
                "required": ["index"]
            }),
        },
        Tool {
            name: "split_pane".into(),
            description: "Split the focused pane".into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "direction": {
                        "type": "string",
                        "enum": ["horizontal", "vertical"],
                        "description": "Split direction"
                    }
                },
                "required": ["direction"]
            }),
        },
        Tool {
            name: "list_panes".into(),
            description: "List panes with their dimensions and state".into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "workspace": { "type": "integer" },
                    "tab": { "type": "integer" }
                }
            }),
        },
        Tool {
            name: "focus_pane".into(),
            description: "Focus a specific pane by ID".into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "pane_id": { "type": "integer" }
                },
                "required": ["pane_id"]
            }),
        },
        Tool {
            name: "close_pane".into(),
            description: "Close a pane".into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "pane_id": { "type": "integer" }
                }
            }),
        },
        Tool {
            name: "zoom_pane".into(),
            description: "Toggle pane zoom".into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "pane_id": { "type": "integer" }
                }
            }),
        },
        Tool {
            name: "run_command".into(),
            description: "Run a shell command in a pane (sends text + Enter)".into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "command": { "type": "string", "description": "Command to run" },
                    "pane_id": { "type": "integer", "description": "Target pane (default: focused)" }
                },
                "required": ["command"]
            }),
        },
        Tool {
            name: "send_keys".into(),
            description: "Send raw keystrokes to a pane".into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "keys": {
                        "type": "string",
                        "description": "Keys to send (supports \\n, \\x03 for Ctrl+C, etc.)"
                    },
                    "pane_id": { "type": "integer" }
                },
                "required": ["keys"]
            }),
        },
        Tool {
            name: "get_terminal_content".into(),
            description: "Read the visible content of a terminal pane".into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "pane_id": { "type": "integer", "description": "Target pane (default: focused)" }
                }
            }),
        },
        Tool {
            name: "get_scrollback".into(),
            description: "Read scrollback buffer content".into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "pane_id": { "type": "integer" },
                    "lines": { "type": "integer", "description": "Number of lines (default: 100, max: 5000)" }
                }
            }),
        },
        Tool {
            name: "get_status".into(),
            description: "Get current termojinal status (active workspace, git info, etc.)".into(),
            input_schema: json!({ "type": "object", "properties": {} }),
        },
        Tool {
            name: "list_pending_requests".into(),
            description: "List pending AI permission requests (Allow Flow)".into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "workspace": { "type": "integer" }
                }
            }),
        },
        Tool {
            name: "approve_request".into(),
            description: "Approve an AI permission request".into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "request_id": { "type": "integer" }
                },
                "required": ["request_id"]
            }),
        },
        Tool {
            name: "deny_request".into(),
            description: "Deny an AI permission request".into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "request_id": { "type": "integer" }
                },
                "required": ["request_id"]
            }),
        },
        Tool {
            name: "approve_all".into(),
            description: "Approve all pending requests for a workspace".into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "workspace": { "type": "integer" }
                },
                "required": ["workspace"]
            }),
        },
        Tool {
            name: "update_agent_status".into(),
            description:
                "Report Claude Code session status to the sidebar (title, state, subagents)".into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "session_id": { "type": "string", "description": "Claude Code session ID" },
                    "pane_id": { "type": "integer", "description": "Pane where agent is running" },
                    "state": {
                        "type": "string",
                        "enum": ["running", "idle", "waiting", "inactive"],
                        "description": "Agent state"
                    },
                    "title": { "type": "string", "description": "Task title / description" },
                    "summary": { "type": "string", "description": "Current activity summary" },
                    "subagent_count": { "type": "integer", "description": "Number of active subagents" }
                }
            }),
        },
    ]
}

/// Handle a tool call by building the appropriate app IPC request and forwarding
/// it through the [`AppClient`].
pub fn handle_tool(client: &AppClient, tool_name: &str, args: &serde_json::Value) -> ToolResult {
    // Build the AppIpcRequest JSON to send over the Unix socket.
    let request = match tool_name {
        "list_workspaces" => json!({"type": "list_workspaces"}),
        "create_workspace" => json!({
            "type": "create_workspace",
            "name": args.get("name"),
            "cwd": args.get("cwd"),
        }),
        "switch_workspace" => json!({
            "type": "switch_workspace",
            "index": args["index"],
        }),
        "close_workspace" => json!({
            "type": "close_workspace",
            "index": args["index"],
        }),
        "list_tabs" => json!({
            "type": "list_tabs",
            "workspace": args.get("workspace"),
        }),
        "create_tab" => json!({
            "type": "create_tab",
            "workspace": args.get("workspace"),
        }),
        "switch_tab" => json!({
            "type": "switch_tab",
            "workspace": args.get("workspace"),
            "index": args["index"],
        }),
        "close_tab" => json!({
            "type": "close_tab",
            "workspace": args.get("workspace"),
            "index": args["index"],
        }),
        "list_panes" => json!({
            "type": "list_panes",
            "workspace": args.get("workspace"),
            "tab": args.get("tab"),
        }),
        "split_pane" => json!({
            "type": "split_pane",
            "direction": args["direction"],
            "pane_id": args.get("pane_id"),
        }),
        "close_pane" => json!({
            "type": "close_pane",
            "pane_id": args.get("pane_id"),
        }),
        "focus_pane" => json!({
            "type": "focus_pane",
            "pane_id": args["pane_id"],
        }),
        "zoom_pane" => json!({
            "type": "zoom_pane",
            "pane_id": args.get("pane_id"),
        }),
        "send_keys" => json!({
            "type": "send_keys",
            "pane_id": args.get("pane_id"),
            "keys": args["keys"],
        }),
        "run_command" => json!({
            "type": "run_command",
            "pane_id": args.get("pane_id"),
            "command": args["command"],
        }),
        "get_terminal_content" => json!({
            "type": "get_terminal_content",
            "pane_id": args.get("pane_id"),
        }),
        "get_scrollback" => json!({
            "type": "get_scrollback",
            "pane_id": args.get("pane_id"),
            "lines": args.get("lines"),
        }),
        "get_status" => json!({"type": "get_status"}),
        "list_pending_requests" => json!({
            "type": "list_pending_requests",
            "workspace": args.get("workspace"),
        }),
        "approve_request" => json!({
            "type": "approve_request",
            "request_id": args["request_id"],
        }),
        "deny_request" => json!({
            "type": "deny_request",
            "request_id": args["request_id"],
        }),
        "approve_all" => json!({
            "type": "approve_all",
            "workspace": args["workspace"],
        }),
        "update_agent_status" => json!({
            "type": "update_agent_status",
            "session_id": args.get("session_id"),
            "pane_id": args.get("pane_id"),
            "state": args.get("state"),
            "title": args.get("title"),
            "summary": args.get("summary"),
            "subagent_count": args.get("subagent_count"),
        }),
        _ => return ToolResult::error(format!("unknown tool: {tool_name}")),
    };

    // Send the request to the termojinal app and translate the response.
    match client.send(&request) {
        Ok(resp) => {
            if resp
                .get("success")
                .and_then(|v| v.as_bool())
                .unwrap_or(false)
            {
                if let Some(data) = resp.get("data") {
                    ToolResult::json(data)
                } else {
                    ToolResult::text("OK")
                }
            } else {
                let err = resp
                    .get("error")
                    .and_then(|v| v.as_str())
                    .unwrap_or("unknown error");
                ToolResult::error(err)
            }
        }
        Err(e) => ToolResult::error(format!("termojinal connection error: {e}")),
    }
}
