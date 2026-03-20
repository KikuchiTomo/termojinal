//! termojinal-mcp — MCP server for controlling termojinal from AI tools.
//!
//! Communicates with the MCP client (e.g. Claude Code) over **stdio**
//! using the JSON-RPC 2.0 protocol, and forwards tool calls to the
//! termojinal GUI over a **Unix socket** at
//! `~/.local/share/termojinal/termojinal-app.sock`.

use std::io::{self, BufRead, Write};

mod client;
mod mcp_types;
mod tools;

use mcp_types::*;

fn main() {
    env_logger::init();
    log::info!("termojinal-mcp starting");

    let app_client = client::AppClient::new();

    let stdin = io::stdin();
    let stdout = io::stdout();
    let mut stdout = stdout.lock();

    for line in stdin.lock().lines() {
        let line = match line {
            Ok(l) => l,
            Err(_) => break,
        };
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }

        log::debug!("recv: {trimmed}");

        let request: JsonRpcRequest = match serde_json::from_str(trimmed) {
            Ok(r) => r,
            Err(e) => {
                let resp = JsonRpcResponse {
                    jsonrpc: "2.0".into(),
                    id: None,
                    result: None,
                    error: Some(JsonRpcError {
                        code: -32700,
                        message: format!("parse error: {e}"),
                    }),
                };
                let _ = writeln!(stdout, "{}", serde_json::to_string(&resp).unwrap());
                let _ = stdout.flush();
                continue;
            }
        };

        // "initialized" is a notification (no id) — do not send a response.
        if request.method == "initialized" && request.id.is_none() {
            log::debug!("received 'initialized' notification, no response needed");
            continue;
        }

        let response = handle_request(&app_client, &request);
        let serialized = serde_json::to_string(&response).unwrap();
        log::debug!("send: {serialized}");
        let _ = writeln!(stdout, "{serialized}");
        let _ = stdout.flush();
    }

    log::info!("termojinal-mcp shutting down");
}

fn handle_request(client: &client::AppClient, req: &JsonRpcRequest) -> JsonRpcResponse {
    match req.method.as_str() {
        "initialize" => {
            let result = InitializeResult {
                protocol_version: "2024-11-05".into(),
                capabilities: Capabilities {
                    tools: ToolsCapability {
                        list_changed: false,
                    },
                },
                server_info: ServerInfo {
                    name: "termojinal-mcp".into(),
                    version: env!("CARGO_PKG_VERSION").into(),
                },
            };
            JsonRpcResponse {
                jsonrpc: "2.0".into(),
                id: req.id.clone(),
                result: Some(serde_json::to_value(result).unwrap()),
                error: None,
            }
        }

        "notifications/initialized" | "initialized" => {
            // Notification — return empty result if it had an id.
            JsonRpcResponse {
                jsonrpc: "2.0".into(),
                id: req.id.clone(),
                result: Some(serde_json::json!({})),
                error: None,
            }
        }

        "tools/list" => {
            let tools_list = tools::list_tools();
            JsonRpcResponse {
                jsonrpc: "2.0".into(),
                id: req.id.clone(),
                result: Some(serde_json::json!({ "tools": tools_list })),
                error: None,
            }
        }

        "tools/call" => {
            let tool_name = req
                .params
                .get("name")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let args = req
                .params
                .get("arguments")
                .cloned()
                .unwrap_or(serde_json::json!({}));
            let result = tools::handle_tool(client, tool_name, &args);
            JsonRpcResponse {
                jsonrpc: "2.0".into(),
                id: req.id.clone(),
                result: Some(serde_json::to_value(result).unwrap()),
                error: None,
            }
        }

        _ => JsonRpcResponse {
            jsonrpc: "2.0".into(),
            id: req.id.clone(),
            result: None,
            error: Some(JsonRpcError {
                code: -32601,
                message: format!("method not found: {}", req.method),
            }),
        },
    }
}
