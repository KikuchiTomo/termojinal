//! App IPC handler and listener.

use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc as std_mpsc;
use std::sync::{Arc, Mutex};

use serde_json::json;
use winit::event_loop::{ActiveEventLoop, EventLoopProxy};

use termojinal_ipc::app_protocol::{AppIpcRequest, AppIpcResponse};
use termojinal_ipc::daemon_connection::daemon_pty_write;
use termojinal_ipc::keybinding::Action;
use termojinal_layout::PaneId;

use crate::{
    active_tab, active_tab_mut, allow_flow, cleanup_session_to_workspace, dispatch_action,
    notification, toggle_quick_terminal, update_window_title, AgentSessionInfo, AgentState,
    AppState, UserEvent,
};

pub(crate) fn handle_app_ipc_request(
    state: &mut AppState,
    request: &AppIpcRequest,
    proxy: &EventLoopProxy<UserEvent>,
    buffers: &Arc<Mutex<HashMap<PaneId, VecDeque<Vec<u8>>>>>,
    event_loop: &ActiveEventLoop,
    response_tx: &std_mpsc::Sender<AppIpcResponse>,
    connection_alive: Option<Arc<AtomicBool>>,
) -> Option<AppIpcResponse> {
    // PermissionRequest is deferred: store the response_tx and return None.
    if let AppIpcRequest::PermissionRequest {
        tool_name,
        tool_input,
        session_id,
    } = request
    {
        use termojinal_claude::request::{AllowRequest, DetectionSource};

        let action = tool_input
            .get("command")
            .or(tool_input.get("file_path"))
            .and_then(|v| v.as_str())
            .unwrap_or("tool use")
            .to_string();
        let detail = serde_json::to_string(&tool_input).unwrap_or_default();

        // Resolve workspace index from session mapping, falling back to active.
        let ws_idx = session_id
            .as_ref()
            .and_then(|sid| state.session_to_workspace.get(sid).copied())
            .unwrap_or(state.active_workspace);

        // Record the mapping for future requests from this session.
        if let Some(sid) = session_id.as_ref() {
            if !state.session_to_workspace.contains_key(sid) {
                state.session_to_workspace.insert(sid.clone(), ws_idx);
            }
        }

        // Update agent session info for this workspace.
        while state.agent_infos.len() <= ws_idx {
            state.agent_infos.push(AgentSessionInfo::default());
        }
        let agent = &mut state.agent_infos[ws_idx];
        agent.active = true;
        agent.state = AgentState::WaitingForPermission;
        agent.session_id = session_id.clone();
        agent.summary = format!("{}: {}", tool_name, &action);
        agent.last_updated = std::time::Instant::now();

        let notif_msg = format!("Permission: {} {}", tool_name, action);
        let request = AllowRequest::new(
            0, // no specific pane for IPC-originated requests
            ws_idx,
            tool_name.clone(),
            action,
            detail,
            DetectionSource::Ipc,
            String::new(), // yes_response: not used for IPC (decision goes back via channel)
            String::new(), // no_response: not used for IPC
        );

        if let Some(req) = state.allow_flow.engine.add_request(request) {
            let req_id = req.id;
            let alive = connection_alive.unwrap_or_else(|| Arc::new(AtomicBool::new(true)));
            state
                .pending_ipc_responses
                .insert(req_id, (response_tx.clone(), alive));
            state.allow_flow.pane_hint_visible = true;
            notification::send_notification(
                "Claude Code",
                &notif_msg,
                state.config.notifications.sound,
            );
            state.window.request_redraw();
            return None; // Deferred — response sent when user decides
        } else {
            // Auto-resolved by a rule.
            return Some(AppIpcResponse::ok(json!({"decision": "allow"})));
        }
    }

    Some(match request {
        AppIpcRequest::Ping => AppIpcResponse::ok(json!("pong")),

        AppIpcRequest::GetStatus => {
            let ws_idx = state.active_workspace;
            let ws = &state.workspaces[ws_idx];
            let tab = &ws.tabs[ws.active_tab];
            let focused_id = tab.layout.focused();
            AppIpcResponse::ok(json!({
                "active_workspace": ws_idx,
                "workspace_name": ws.name,
                "active_tab": ws.active_tab,
                "focused_pane": focused_id,
                "workspace_count": state.workspaces.len(),
            }))
        }

        AppIpcRequest::GetConfig => AppIpcResponse::ok(json!({
            "font_size": state.config.font.size,
            "opacity": state.config.window.opacity,
            "theme_bg": state.config.theme.background,
            "theme_fg": state.config.theme.foreground,
        })),

        AppIpcRequest::ListWorkspaces => {
            let workspaces: Vec<_> = state
                .workspaces
                .iter()
                .enumerate()
                .map(|(i, ws)| {
                    json!({
                        "index": i,
                        "name": ws.name,
                        "tab_count": ws.tabs.len(),
                        "active_tab": ws.active_tab,
                        "is_active": i == state.active_workspace,
                    })
                })
                .collect();
            AppIpcResponse::ok(json!({ "workspaces": workspaces }))
        }

        AppIpcRequest::CreateWorkspace { name, .. } => {
            dispatch_action(state, &Action::NewWorkspace, proxy, buffers, event_loop);
            // Optionally set the workspace name.
            if let Some(name) = name {
                if let Some(ws) = state.workspaces.last_mut() {
                    ws.name = name.clone();
                }
            }
            AppIpcResponse::ok(json!({ "index": state.workspaces.len() - 1 }))
        }

        AppIpcRequest::SwitchWorkspace { index } => {
            if *index < state.workspaces.len() {
                state.active_workspace = *index;
                update_window_title(state);
                state.window.request_redraw();
                AppIpcResponse::ok_empty()
            } else {
                AppIpcResponse::err(format!("workspace index {} out of range", index))
            }
        }

        AppIpcRequest::CloseWorkspace { index } => {
            if *index < state.workspaces.len() && state.workspaces.len() > 1 {
                // Clean up PTY buffers and daemon registrations for all
                // panes in the workspace being removed.
                {
                    let ws = &state.workspaces[*index];
                    let pane_ids: Vec<PaneId> = ws
                        .tabs
                        .iter()
                        .flat_map(|t| t.panes.keys().copied())
                        .collect();
                    if let Ok(mut bufs) = buffers.lock() {
                        for pid in &pane_ids {
                            bufs.remove(pid);
                        }
                    }
                }
                state.workspaces.remove(*index);
                if *index < state.workspace_infos.len() {
                    state.workspace_infos.remove(*index);
                }
                if *index < state.agent_infos.len() {
                    state.agent_infos.remove(*index);
                }
                if *index < state.dir_trees.len() {
                    state.dir_trees.remove(*index);
                }
                cleanup_session_to_workspace(state, *index);
                if state.active_workspace >= state.workspaces.len() {
                    state.active_workspace = state.workspaces.len() - 1;
                }
                update_window_title(state);
                state.window.request_redraw();
                AppIpcResponse::ok_empty()
            } else {
                AppIpcResponse::err("cannot close workspace")
            }
        }

        AppIpcRequest::ListTabs { workspace } => {
            let ws_idx = workspace.unwrap_or(state.active_workspace);
            if let Some(ws) = state.workspaces.get(ws_idx) {
                let tabs: Vec<_> = ws
                    .tabs
                    .iter()
                    .enumerate()
                    .map(|(i, tab)| {
                        json!({
                            "index": i,
                            "name": tab.name,
                            "pane_count": tab.panes.len(),
                            "is_active": i == ws.active_tab,
                        })
                    })
                    .collect();
                AppIpcResponse::ok(json!({ "tabs": tabs }))
            } else {
                AppIpcResponse::err("invalid workspace index")
            }
        }

        AppIpcRequest::CreateTab { .. } => {
            dispatch_action(state, &Action::NewTab, proxy, buffers, event_loop);
            AppIpcResponse::ok_empty()
        }

        AppIpcRequest::SwitchTab { workspace, index } => {
            let ws_idx = workspace.unwrap_or(state.active_workspace);
            if let Some(ws) = state.workspaces.get_mut(ws_idx) {
                if *index < ws.tabs.len() {
                    ws.active_tab = *index;
                    update_window_title(state);
                    state.window.request_redraw();
                    AppIpcResponse::ok_empty()
                } else {
                    AppIpcResponse::err("tab index out of range")
                }
            } else {
                AppIpcResponse::err("invalid workspace index")
            }
        }

        AppIpcRequest::CloseTab { .. } => {
            dispatch_action(state, &Action::CloseTab, proxy, buffers, event_loop);
            AppIpcResponse::ok_empty()
        }

        AppIpcRequest::ListPanes { workspace, tab } => {
            let ws_idx = workspace.unwrap_or(state.active_workspace);
            if let Some(ws) = state.workspaces.get(ws_idx) {
                let tab_idx = tab.unwrap_or(ws.active_tab);
                if let Some(tab) = ws.tabs.get(tab_idx) {
                    let focused = tab.layout.focused();
                    let panes: Vec<_> = tab
                        .panes
                        .iter()
                        .map(|(id, pane)| {
                            json!({
                                "pane_id": id,
                                "cols": pane.terminal.cols(),
                                "rows": pane.terminal.rows(),
                                "is_focused": *id == focused,
                                "cwd": pane.terminal.osc.cwd,
                            })
                        })
                        .collect();
                    AppIpcResponse::ok(json!({ "panes": panes }))
                } else {
                    AppIpcResponse::err("invalid tab index")
                }
            } else {
                AppIpcResponse::err("invalid workspace index")
            }
        }

        AppIpcRequest::SplitPane { direction, .. } => {
            let action = match direction.as_str() {
                "horizontal" | "right" => Action::SplitRight,
                "vertical" | "down" => Action::SplitDown,
                _ => {
                    return Some(AppIpcResponse::err(
                        "direction must be 'horizontal' or 'vertical'",
                    ))
                } // early return still needs Some
            };
            dispatch_action(state, &action, proxy, buffers, event_loop);
            AppIpcResponse::ok_empty()
        }

        AppIpcRequest::ClosePane { .. } => {
            dispatch_action(state, &Action::CloseTab, proxy, buffers, event_loop);
            AppIpcResponse::ok_empty()
        }

        AppIpcRequest::FocusPane { pane_id } => {
            let ws = &mut state.workspaces[state.active_workspace];
            let tab = &mut ws.tabs[ws.active_tab];
            let target = *pane_id;
            if tab.panes.contains_key(&target) {
                tab.layout = tab.layout.focus(target);
                state.window.request_redraw();
                AppIpcResponse::ok_empty()
            } else {
                AppIpcResponse::err(format!("pane {} not found", pane_id))
            }
        }

        AppIpcRequest::ZoomPane { .. } => {
            dispatch_action(state, &Action::ZoomPane, proxy, buffers, event_loop);
            AppIpcResponse::ok_empty()
        }

        AppIpcRequest::ExtractPaneToTab { .. } => {
            dispatch_action(state, &Action::ExtractPaneToTab, proxy, buffers, event_loop);
            AppIpcResponse::ok_empty()
        }

        AppIpcRequest::SendKeys { pane_id, keys } => {
            let ws = &state.workspaces[state.active_workspace];
            let tab = &ws.tabs[ws.active_tab];
            let target = pane_id.unwrap_or_else(|| tab.layout.focused());
            if let Some(pane) = tab.panes.get(&target) {
                let bytes = unescape_keys(keys);
                let _ = daemon_pty_write(&pane.session_id, &bytes);
                AppIpcResponse::ok_empty()
            } else {
                AppIpcResponse::err("pane not found")
            }
        }

        AppIpcRequest::RunCommand { pane_id, command } => {
            let ws = &state.workspaces[state.active_workspace];
            let tab = &ws.tabs[ws.active_tab];
            let target = pane_id.unwrap_or_else(|| tab.layout.focused());
            if let Some(pane) = tab.panes.get(&target) {
                let mut cmd = command.clone();
                if !cmd.ends_with('\n') {
                    cmd.push('\n');
                }
                let _ = daemon_pty_write(&pane.session_id, cmd.as_bytes());
                AppIpcResponse::ok_empty()
            } else {
                AppIpcResponse::err("pane not found")
            }
        }

        AppIpcRequest::GetTerminalContent { pane_id } => {
            let ws = &state.workspaces[state.active_workspace];
            let tab = &ws.tabs[ws.active_tab];
            let target = pane_id.unwrap_or_else(|| tab.layout.focused());
            if let Some(pane) = tab.panes.get(&target) {
                let grid = pane.terminal.grid();
                let cols = grid.cols();
                let rows = grid.rows();
                let mut lines = Vec::new();
                for row in 0..rows {
                    let mut line = String::new();
                    for col in 0..cols {
                        let cell = grid.cell(col, row);
                        line.push(if cell.c == '\0' { ' ' } else { cell.c });
                    }
                    lines.push(line.trim_end().to_string());
                }
                AppIpcResponse::ok(json!({
                    "lines": lines,
                    "cols": cols,
                    "rows": rows,
                    "cursor_row": pane.terminal.cursor_row,
                    "cursor_col": pane.terminal.cursor_col,
                }))
            } else {
                AppIpcResponse::err("pane not found")
            }
        }

        AppIpcRequest::GetScrollback { pane_id, lines } => {
            let ws = &state.workspaces[state.active_workspace];
            let tab = &ws.tabs[ws.active_tab];
            let target = pane_id.unwrap_or_else(|| tab.layout.focused());
            if let Some(pane) = tab.panes.get(&target) {
                let max_lines = lines.unwrap_or(100).min(5000);
                let scrollback_len = pane.terminal.scrollback_len();
                let start = scrollback_len.saturating_sub(max_lines);
                let mut result_lines = Vec::new();
                for i in start..scrollback_len {
                    if let Some(row) = pane.terminal.scrollback_row(i) {
                        let line: String = row
                            .iter()
                            .map(|c| if c.c == '\0' { ' ' } else { c.c })
                            .collect();
                        result_lines.push(line.trim_end().to_string());
                    }
                }
                AppIpcResponse::ok(json!({
                    "lines": result_lines,
                    "total": scrollback_len,
                }))
            } else {
                AppIpcResponse::err("pane not found")
            }
        }

        AppIpcRequest::ListPendingRequests { workspace } => {
            let ws_idx = workspace.unwrap_or(state.active_workspace);
            let pending = state.allow_flow.pending_for_workspace(ws_idx);
            let requests: Vec<_> = pending
                .iter()
                .map(|r| {
                    json!({
                        "id": r.id,
                        "tool": r.tool_name,
                        "action": r.action,
                        "detail": r.detail,
                        "pane_id": r.pane_id,
                    })
                })
                .collect();
            AppIpcResponse::ok(json!({ "requests": requests }))
        }

        AppIpcRequest::ApproveRequest { request_id } => {
            let pane_sessions: HashMap<u64, String> = {
                let mut m = std::collections::HashMap::new();
                for ws in &state.workspaces {
                    for tab in &ws.tabs {
                        for (pid, pane) in &tab.panes {
                            m.insert(*pid, pane.session_id.clone());
                        }
                    }
                }
                m
            };
            if let Some(response) = state
                .allow_flow
                .engine
                .respond(*request_id, termojinal_claude::AllowDecision::Allow)
            {
                allow_flow::AllowFlowUI::write_to_pty(
                    &pane_sessions,
                    response.pane_id,
                    &response.pty_write,
                );
                // Also resolve deferred IPC response if this was hook-originated.
                if let Some((tx, _alive)) = state.pending_ipc_responses.remove(request_id) {
                    let _ = tx.send(AppIpcResponse::ok(json!({"decision": "allow"})));
                }
                AppIpcResponse::ok_empty()
            } else {
                AppIpcResponse::err("request not found or already resolved")
            }
        }

        AppIpcRequest::DenyRequest { request_id } => {
            let pane_sessions: HashMap<u64, String> = {
                let mut m = std::collections::HashMap::new();
                for ws in &state.workspaces {
                    for tab in &ws.tabs {
                        for (pid, pane) in &tab.panes {
                            m.insert(*pid, pane.session_id.clone());
                        }
                    }
                }
                m
            };
            if let Some(response) = state
                .allow_flow
                .engine
                .respond(*request_id, termojinal_claude::AllowDecision::Deny)
            {
                allow_flow::AllowFlowUI::write_to_pty(
                    &pane_sessions,
                    response.pane_id,
                    &response.pty_write,
                );
                if let Some((tx, _alive)) = state.pending_ipc_responses.remove(request_id) {
                    let _ = tx.send(AppIpcResponse::ok(json!({"decision": "deny"})));
                }
                AppIpcResponse::ok_empty()
            } else {
                AppIpcResponse::err("request not found or already resolved")
            }
        }

        AppIpcRequest::ApproveAll { workspace } => {
            let pane_sessions: HashMap<u64, String> = {
                let mut m = std::collections::HashMap::new();
                for ws in &state.workspaces {
                    for tab in &ws.tabs {
                        for (pid, pane) in &tab.panes {
                            m.insert(*pid, pane.session_id.clone());
                        }
                    }
                }
                m
            };
            let resolved = state
                .allow_flow
                .allow_all_for_workspace(*workspace, &pane_sessions);
            for (req_id, _) in &resolved {
                if let Some((tx, _alive)) = state.pending_ipc_responses.remove(req_id) {
                    let _ = tx.send(AppIpcResponse::ok(json!({"decision": "allow"})));
                }
            }
            AppIpcResponse::ok_empty()
        }

        AppIpcRequest::Notify {
            title,
            body,
            subtitle: _,
            notification_type,
        } => {
            // 1. Send macOS desktop notification.
            let notif_title = title.as_deref().unwrap_or("termojinal");
            let notif_body = body.as_deref().unwrap_or("");
            notification::send_notification(
                notif_title,
                notif_body,
                state.config.notifications.sound,
            );

            // 2. Mark the active workspace as having unread activity.
            if let Some(info) = state.workspace_infos.get_mut(state.active_workspace) {
                info.has_unread = true;
            }

            // 3. If it's a permission_prompt, show Allow Flow pane hint.
            if notification_type.as_deref() == Some("permission_prompt") {
                state.allow_flow.pane_hint_visible = true;
            }

            state.window.request_redraw();
            AppIpcResponse::ok_empty()
        }

        AppIpcRequest::UpdateAgentStatus {
            session_id,
            pane_id: ipc_pane_id,
            subagent_count,
            state: agent_state_str,
            summary,
            title,
        } => {
            let ws_idx = session_id
                .as_ref()
                .and_then(|sid| state.session_to_workspace.get(sid).copied())
                .unwrap_or(state.active_workspace);

            while state.agent_infos.len() <= ws_idx {
                state.agent_infos.push(AgentSessionInfo::default());
            }
            let agent = &mut state.agent_infos[ws_idx];

            if let Some(pid) = ipc_pane_id {
                agent.pane_id = Some(*pid);
            }
            if let Some(count) = subagent_count {
                agent.subagent_count = *count;
            }
            if let Some(s) = agent_state_str {
                agent.state = match s.as_str() {
                    "running" => AgentState::Running,
                    "waiting" | "waiting_for_permission" => AgentState::WaitingForPermission,
                    "idle" => AgentState::Idle,
                    "inactive" => AgentState::Inactive,
                    _ => agent.state.clone(),
                };
                agent.active = !matches!(agent.state, AgentState::Inactive);
            }
            if let Some(s) = summary {
                agent.summary = s.clone();
            }
            if let Some(t) = title {
                agent.title = Some(t.clone());
            }
            agent.last_updated = std::time::Instant::now();

            if let Some(sid) = session_id.as_ref() {
                if !state.session_to_workspace.contains_key(sid) {
                    state.session_to_workspace.insert(sid.clone(), ws_idx);
                }
            }

            state.window.request_redraw();
            AppIpcResponse::ok_empty()
        }

        AppIpcRequest::ToggleQuickTerminal => {
            toggle_quick_terminal(state);
            AppIpcResponse::ok_empty()
        }

        AppIpcRequest::ShowPalette => {
            state.command_palette.toggle();
            state.window.request_redraw();
            AppIpcResponse::ok_empty()
        }

        // --- Time Travel IPC ---
        AppIpcRequest::GetCommandHistory { pane_id, limit } => {
            let focused_id = pane_id.unwrap_or_else(|| active_tab(state).layout.focused());
            if let Some(pane) = active_tab(state).panes.get(&focused_id) {
                let history = pane.terminal.command_history();
                let limit = limit.unwrap_or(history.len());
                let records: Vec<_> = history.iter().rev().take(limit).collect();
                AppIpcResponse::ok(serde_json::to_value(&records).unwrap_or_default())
            } else {
                AppIpcResponse::err("pane not found")
            }
        }
        AppIpcRequest::JumpToCommand {
            pane_id,
            command_id,
        } => {
            let focused_id = pane_id.unwrap_or_else(|| active_tab(state).layout.focused());
            if let Some(pane) = active_tab_mut(state).panes.get_mut(&focused_id) {
                if pane.terminal.jump_to_command(*command_id).is_some() {
                    state.window.request_redraw();
                    AppIpcResponse::ok_empty()
                } else {
                    AppIpcResponse::err("command not found")
                }
            } else {
                AppIpcResponse::err("pane not found")
            }
        }
        AppIpcRequest::ToggleTimeline => {
            state.timeline_visible = !state.timeline_visible;
            if state.timeline_visible {
                state.timeline_input.clear();
                state.timeline_selected = 0;
                state.timeline_scroll_offset = 0;
            }
            state.window.request_redraw();
            AppIpcResponse::ok_empty()
        }

        // Handled by the early-return above; unreachable in practice.
        AppIpcRequest::PermissionRequest { .. } => unreachable!(),
    })
}

/// Unescape common key sequences: `\n`, `\r`, `\t`, `\xNN`, `\\`.
pub(crate) fn unescape_keys(s: &str) -> Vec<u8> {
    let mut result = Vec::new();
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        if c == '\\' {
            match chars.next() {
                Some('n') => result.push(b'\n'),
                Some('r') => result.push(b'\r'),
                Some('t') => result.push(b'\t'),
                Some('\\') => result.push(b'\\'),
                Some('x') => {
                    let hex: String = chars.by_ref().take(2).collect();
                    if let Ok(byte) = u8::from_str_radix(&hex, 16) {
                        result.push(byte);
                    }
                }
                Some(other) => {
                    result.push(b'\\');
                    let mut buf = [0u8; 4];
                    result.extend_from_slice(other.encode_utf8(&mut buf).as_bytes());
                }
                None => result.push(b'\\'),
            }
        } else {
            let mut buf = [0u8; 4];
            result.extend_from_slice(c.encode_utf8(&mut buf).as_bytes());
        }
    }
    result
}

// ---------------------------------------------------------------------------
// App-side IPC listener (receives commands from the daemon)
// ---------------------------------------------------------------------------

/// Get the app IPC socket path (matches `termojinal_session::daemon::app_socket_path`).
pub(crate) fn app_ipc_socket_path() -> std::path::PathBuf {
    let data_dir = dirs::data_local_dir().unwrap_or_else(|| std::path::PathBuf::from("/tmp"));
    data_dir.join("termojinal").join("termojinal-app.sock")
}

/// Listen for IPC commands from the daemon (e.g., toggle_quick_terminal).
///
/// Binds a Unix domain socket at `~/.local/share/termojinal/termojinal-app.sock` and
/// dispatches incoming line-delimited commands as `UserEvent`s on the winit
/// event loop.
pub(crate) fn app_ipc_listener(
    proxy: EventLoopProxy<UserEvent>,
    shutdown: Arc<AtomicBool>,
) {
    use std::io::{BufRead, BufReader, Write};
    use std::os::unix::net::UnixListener;

    let sock_path = app_ipc_socket_path();

    // Ensure parent directory exists.
    if let Some(parent) = sock_path.parent() {
        std::fs::create_dir_all(parent).ok();
        use std::os::unix::fs::PermissionsExt;
        if let Err(e) = std::fs::set_permissions(parent, std::fs::Permissions::from_mode(0o700)) {
            log::warn!("failed to set parent dir permissions: {e}");
        }
    }

    // Remove stale socket from a previous run.
    let _ = std::fs::remove_file(&sock_path);

    let listener = match UnixListener::bind(&sock_path) {
        Ok(l) => l,
        Err(e) => {
            log::error!(
                "failed to bind app IPC socket at {}: {e}",
                sock_path.display()
            );
            return;
        }
    };

    // Restrict socket file permissions to owner only.
    {
        use std::os::unix::fs::PermissionsExt;
        if let Err(e) =
            std::fs::set_permissions(&sock_path, std::fs::Permissions::from_mode(0o600))
        {
            log::warn!("failed to set socket permissions: {e}");
        }
    }
    // Non-blocking so the loop can check shutdown flag.
    listener
        .set_nonblocking(true)
        .unwrap_or_else(|e| log::warn!("failed to set IPC listener non-blocking: {e}"));
    log::info!("app IPC listener started at {}", sock_path.display());

    loop {
        if shutdown.load(std::sync::atomic::Ordering::Relaxed) {
            break;
        }
        let stream = match listener.accept() {
            Ok((s, _)) => {
                // Set the accepted stream back to blocking for normal I/O.
                s.set_nonblocking(false).ok();
                s
            }
            Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                std::thread::sleep(std::time::Duration::from_millis(100));
                continue;
            }
            Err(e) => {
                log::debug!("app IPC accept error: {e}");
                continue;
            }
        };

    {
        let mut stream = stream;
        let reader = match stream.try_clone() {
            Ok(s) => BufReader::new(s),
            Err(_) => continue,
        };
        // Track the connection alive flag for this client.
        let connection_alive = Arc::new(AtomicBool::new(true));
        let mut had_permission_request = false;

        for line in reader.lines() {
            if shutdown.load(std::sync::atomic::Ordering::Relaxed) {
                return;
            }
            let line = match line {
                Ok(l) => l,
                Err(e) => {
                    log::debug!("app IPC read error: {e}");
                    break;
                }
            };
            let cmd = line.trim();
            if cmd.is_empty() {
                continue;
            }

            // Try JSON protocol first
            if let Ok(request) = serde_json::from_str::<AppIpcRequest>(cmd) {
                let is_permission_request =
                    matches!(&request, AppIpcRequest::PermissionRequest { .. });
                let alive_for_event = if is_permission_request {
                    had_permission_request = true;
                    Some(Arc::clone(&connection_alive))
                } else {
                    None
                };
                let (tx, rx) = std_mpsc::channel();
                if proxy
                    .send_event(UserEvent::AppIpc {
                        request,
                        response_tx: tx,
                        connection_alive: alive_for_event,
                    })
                    .is_err()
                {
                    break;
                }
                // PermissionRequest waits up to 10 minutes (matches hook timeout).
                let timeout = if is_permission_request {
                    std::time::Duration::from_secs(600)
                } else {
                    std::time::Duration::from_secs(5)
                };
                match rx.recv_timeout(timeout) {
                    Ok(response) => {
                        let json_str = serde_json::to_string(&response).unwrap_or_default();
                        let _ = stream.write_all(json_str.as_bytes());
                        let _ = stream.write_all(b"\n");
                        let _ = stream.flush();
                    }
                    Err(_) => {
                        let timeout_resp = AppIpcResponse::err("request timed out");
                        let json_str =
                            serde_json::to_string(&timeout_resp).unwrap_or_default();
                        let _ = stream.write_all(json_str.as_bytes());
                        let _ = stream.write_all(b"\n");
                        let _ = stream.flush();
                    }
                }
            } else {
                // Legacy text protocol
                match cmd {
                    "toggle_quick_terminal" => {
                        let _ = proxy.send_event(UserEvent::ToggleQuickTerminal);
                    }
                    "show_palette" => {
                        log::debug!("app IPC: show_palette (not yet wired)");
                    }
                    _ => {
                        log::debug!("unknown app IPC command: {cmd}");
                    }
                }
            }
        }
        // Client disconnected (reader.lines() ended).
        // Mark the connection as dead and notify the GUI to clean up
        // any stale pending permission requests or agent state.
        connection_alive.store(false, Ordering::SeqCst);
        if had_permission_request {
            let _ = proxy.send_event(UserEvent::IpcClientDisconnected);
        }
    }
    } // end loop
}
