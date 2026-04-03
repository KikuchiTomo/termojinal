use std::time::Instant;
use crate::*;
use config::color_or;
use termojinal_render::RoundedRect;

/// Handle a click in the sidebar area. Determine which workspace was clicked.
/// The new sidebar layout has:
///   - config top_padding
///   - Each workspace: name + cwd + git + ports + allow flow lines + entry_gap
///   - Separator line (1px + 8px padding each side)
///   - "New Workspace" button
pub(crate) fn handle_sidebar_click(state: &mut AppState) -> Option<Action> {
    let cy = state.cursor_pos.1 as f32;
    let cell_h = state.renderer.cell_size().height;
    let sc = &state.config.sidebar;
    let top_pad = sc.top_padding;
    let entry_gap = sc.entry_gap;
    let info_line_gap = sc.info_line_gap;

    let mut entry_y = top_pad;
    for (i, _ws) in state.workspaces.iter().enumerate() {
        let info = state.workspace_infos.get(i);
        let is_active = i == state.active_workspace;
        let has_allow_pending = state.allow_flow.has_pending_for_workspace(i);

        // Name line
        let mut entry_h = cell_h;
        // CWD line
        // Use the cached CWD (resolved from OSC 7 or lsof fallback)
        // instead of reading osc.cwd directly, which may be empty.
        let ws_cwd = state
            .workspace_infos
            .get(i)
            .map(|inf| inf.cwd.clone())
            .unwrap_or_default();
        if !ws_cwd.is_empty() {
            entry_h += info_line_gap + cell_h;
        }
        // Git info line (always present if branch known)
        if let Some(info) = info {
            if info.git_branch.is_some() {
                entry_h += info_line_gap + cell_h;
            }
            // Ports line
            if !info.ports.is_empty() {
                entry_h += info_line_gap + cell_h;
            }
        }
        // Allow Flow indicator line (same compact badge for both active and inactive).
        if has_allow_pending {
            entry_h += info_line_gap + cell_h;
        }
        // Agent status lines.
        let has_agent = state.config.sidebar.agent_status_enabled
            && i < state.agent_infos.len()
            && state.agent_infos[i].active;
        if has_agent {
            entry_h += info_line_gap + cell_h; // agent status line
            if !state.agent_infos[i].summary.is_empty() {
                entry_h += info_line_gap + cell_h; // summary line
            }
            // Note: subagent count is merged into the agent status line (no extra height).
        }

        entry_h += entry_gap; // gap between entries

        if cy >= entry_y && cy < entry_y + entry_h {
            if state.active_workspace != i {
                state.active_workspace = i;
                if i < state.workspace_infos.len() {
                    state.workspace_infos[i].has_unread = false;
                }
                resize_all_panes(state);
                update_window_title(state);
                state.window.request_redraw();
            } else {
                // Click on active workspace outside tree — check if it's the CWD line to toggle tree.
                let cwd_line_y = entry_y + cell_h; // after name line
                if !ws_cwd.is_empty()
                    && cy >= cwd_line_y
                    && cy < cwd_line_y + info_line_gap + cell_h
                {
                    return Some(Action::ToggleDirectoryTree);
                }
            }
            return None;
        }
        entry_y += entry_h;

        // --- Tree block click detection (standalone, after active workspace entry) ---
        let tree_visible_click = is_active
            && i < state.dir_trees.len()
            && state.dir_trees[i].visible
            && !state.dir_trees[i].entries.is_empty();
        if tree_visible_click {
            let tree = &state.dir_trees[i];
            let visible_lines = if tree.current_visible_lines > 0 {
                tree.current_visible_lines
            } else {
                state.config.directory_tree.max_visible_lines.max(1)
            };
            let entry_count = tree.entries.len();
            let actual_visible = entry_count.min(visible_lines);
            let total = 1 + actual_visible + 1; // header + entries + hint
            let tree_area_h = info_line_gap + (total as f32) * cell_h;
            let tree_area_start = entry_y;

            if cy >= tree_area_start && cy < tree_area_start + tree_area_h {
                let tree_content_start = tree_area_start + info_line_gap + cell_h; // skip header
                if cy >= tree_content_start {
                    let line_offset = ((cy - tree_content_start) / cell_h) as usize;
                    let entry_idx = tree.scroll_offset + line_offset;
                    if entry_idx < tree.entries.len() && line_offset < actual_visible {
                        // Double-click detection.
                        let now = Instant::now();
                        let dbl_ms = state.config.directory_tree.double_click_ms;
                        let is_double_click = state.dir_trees[i]
                            .last_click_time
                            .map(|t| now.duration_since(t).as_millis() < dbl_ms as u128)
                            .unwrap_or(false)
                            && state.dir_trees[i].last_click_index == Some(entry_idx);

                        state.dir_trees[i].last_click_time = Some(now);
                        state.dir_trees[i].last_click_index = Some(entry_idx);
                        state.dir_trees[i].selected = entry_idx;
                        state.dir_trees[i].focused = true;

                        if is_double_click {
                            if state.dir_trees[i].entries[entry_idx].is_dir {
                                // Double-click on directory: cd to it.
                                let path = state.dir_trees[i].entries[entry_idx].path.clone();
                                let focused_id = active_tab(state).layout.focused();
                                if let Some(pane) = active_tab_mut(state).panes.get_mut(&focused_id)
                                {
                                    let cmd = format!("cd {}\n", shell_escape(&path));
                                    let _ = daemon_pty_write(&pane.session_id, cmd.as_bytes());
                                }
                            } else {
                                // Double-click on file: mark for opening in editor.
                                state.dir_trees[i].pending_open_in_editor = true;
                            }
                        } else if state.dir_trees[i].entries[entry_idx].is_dir {
                            // Single click on directory: toggle expand/collapse.
                            toggle_tree_entry(&mut state.dir_trees[i], entry_idx);
                        }
                        state.window.request_redraw();
                    }
                }
                return None;
            }
            entry_y += tree_area_h + entry_gap;
        }
    }

    // Check "+ New Workspace" button (below separator).
    // Separator: 8px + 1px + 8px = 17px
    entry_y += 17.0;
    if cy >= entry_y && cy < entry_y + cell_h {
        return Some(Action::NewWorkspace);
    }

    // --- Claudes Summary click handling ---
    // Mirror the layout calculation from render_sidebar to find Claude entry positions.
    let phys_h = state.window.inner_size().height as f32;

    // Collect active Claude sessions (same logic as render_sidebar).
    struct ClickClaudeEntry {
        wi: usize,
        running_sub_count: usize,
    }
    let mut claude_click_entries: Vec<ClickClaudeEntry> = Vec::new();
    for wi in 0..state.workspaces.len() {
        if wi >= state.agent_infos.len() {
            break;
        }
        if state.agent_infos[wi].active {
            let running_sub_count = state.agent_infos[wi]
                .subagents
                .iter()
                .filter(|s| matches!(s.state, AgentState::Running))
                .count();
            claude_click_entries.push(ClickClaudeEntry {
                wi,
                running_sub_count,
            });
        }
    }

    if !claude_click_entries.is_empty() {
        let session_line_h = cell_h;
        let session_gap = info_line_gap;
        let session_pad_y = 3.0;
        let header_h = session_line_h + session_gap;
        let session_entry_gap = 4.0;
        let sessions_sep_h = 1.0 + 10.0 * 2.0;
        let bottom_pad = 8.0;

        // Calculate per-entry height (must match render logic).
        let compute_click_entry_h = |entry: &ClickClaudeEntry| -> f32 {
            let base_lines = 2.0;
            let sub_lines = entry.running_sub_count as f32;
            (base_lines + sub_lines) * session_line_h
                + (base_lines + sub_lines - 1.0).max(0.0) * session_gap
                + session_pad_y * 2.0
        };

        let sessions_total_h = if state.claudes_collapsed {
            header_h
        } else {
            let entries_h: f32 = claude_click_entries
                .iter()
                .map(|e| compute_click_entry_h(e))
                .sum();
            let gaps_h = (claude_click_entries.len().saturating_sub(1)) as f32 * session_entry_gap;
            header_h + entries_h + gaps_h
        };

        let sessions_start_y = phys_h - sessions_total_h - sessions_sep_h - bottom_pad;
        let new_ws_y_end = entry_y + cell_h;
        let min_start_y = new_ws_y_end + 16.0;

        if sessions_start_y >= min_start_y && cy >= sessions_start_y {
            // Check if click is on the header line (toggle collapse).
            let header_y = sessions_start_y + sessions_sep_h;
            if cy >= header_y && cy < header_y + header_h {
                state.claudes_collapsed = !state.claudes_collapsed;
                state.window.request_redraw();
                return None;
            }

            // If not collapsed, check individual Claude entries.
            if !state.claudes_collapsed {
                let mut sy = header_y + header_h;
                for entry in &claude_click_entries {
                    let entry_h = compute_click_entry_h(entry);
                    let session_end = sy + entry_h + session_entry_gap;
                    if cy >= sy && cy < session_end {
                        if state.active_workspace != entry.wi {
                            state.active_workspace = entry.wi;
                            if entry.wi < state.workspace_infos.len() {
                                state.workspace_infos[entry.wi].has_unread = false;
                            }
                            resize_all_panes(state);
                            update_window_title(state);
                            state.window.request_redraw();
                        }
                        // Focus the specific pane where the agent is running.
                        if let Some(target_pane) =
                            state.agent_infos.get(entry.wi).and_then(|a| a.pane_id)
                        {
                            if entry.wi >= state.workspaces.len() {
                                return None;
                            }
                            let ws = &mut state.workspaces[entry.wi];
                            let mut found_tab = None;
                            for (tab_idx, tab) in ws.tabs.iter().enumerate() {
                                if tab.panes.contains_key(&target_pane) {
                                    found_tab = Some(tab_idx);
                                    break;
                                }
                            }
                            if let Some(tab_idx) = found_tab {
                                ws.active_tab = tab_idx;
                                ws.tabs[tab_idx].layout =
                                    ws.tabs[tab_idx].layout.focus(target_pane);
                            }
                        }
                        return None;
                    }
                    sy = session_end;
                }
            }
        }
    }

    None
}

/// Render the sidebar showing workspaces with rich information.
/// Inspired by cmux terminal (minimal vertical tabs) and Arc browser (colorful dots).
pub(crate) fn render_sidebar(state: &mut AppState, view: &wgpu::TextureView, phys_h: f32) {
    // --- Color palette from config ---
    let sc = &state.config.sidebar;
    let sidebar_bg = color_or(&sc.bg, [0.051, 0.051, 0.071, 1.0]);
    let active_entry_bg = color_or(&sc.active_entry_bg, [0.118, 0.118, 0.165, 1.0]);
    let active_fg = color_or(&sc.active_fg, [0.95, 0.95, 0.97, 1.0]);
    let inactive_fg = color_or(&sc.inactive_fg, [0.627, 0.627, 0.675, 1.0]);
    let dim_fg = color_or(&sc.dim_fg, [0.467, 0.467, 0.541, 1.0]);
    let inactive_dot_color = dim_fg;
    let git_branch_fg = color_or(&sc.git_branch_fg, [0.35, 0.70, 0.85, 1.0]);
    let separator_color = color_or(&sc.separator_color, [0.20, 0.20, 0.22, 1.0]);
    let notification_dot = color_or(&sc.notification_dot, [1.0, 0.58, 0.26, 1.0]);
    let yellow_fg = color_or(&sc.git_dirty_color, [0.8, 0.7, 0.3, 1.0]);
    // Allow Flow accent colors.
    let allow_accent_color = color_or(&sc.allow_accent_color, [0.31, 0.76, 1.0, 1.0]);
    let _allow_hint_fg = color_or(&sc.allow_hint_fg, [0.49, 0.78, 1.0, 1.0]);
    // Agent status colors.
    let agent_status_enabled = sc.agent_status_enabled;
    let agent_indicator_style = sc.agent_indicator_style.clone();
    let agent_pulse_speed = sc.agent_pulse_speed;
    let agent_active_color = color_or(&sc.agent_active_color, [0.655, 0.545, 0.98, 1.0]);
    let agent_idle_color = color_or(&sc.agent_idle_color, [0.984, 0.749, 0.141, 1.0]);

    let cell_h = state.renderer.cell_size().height;
    let cell_w = state.renderer.cell_size().width;
    let sidebar_w = state.sidebar_width;

    // Spacing from config.
    let top_pad = sc.top_padding;
    let side_pad = sc.side_padding;
    let entry_gap = sc.entry_gap;
    let info_line_gap = sc.info_line_gap;

    // --- Draw sidebar background (full height) ---
    state
        .renderer
        .submit_separator(view, 0, 0, sidebar_w as u32, phys_h as u32, sidebar_bg);

    // --- Refresh workspace info ---
    while state.workspace_infos.len() < state.workspaces.len() {
        state.workspace_infos.push(WorkspaceInfo::new());
    }
    // Keep agent_infos in sync.
    while state.agent_infos.len() < state.workspaces.len() {
        state.agent_infos.push(AgentSessionInfo::default());
    }
    // Keep dir_trees in sync.
    while state.dir_trees.len() < state.workspaces.len() {
        let mut tree = DirectoryTreeState::new();
        // Apply config: auto-show tree when directory_tree.enabled = true.
        if state.config.directory_tree.enabled {
            tree.visible = true;
        }
        state.dir_trees.push(tree);
    }
    // Ensure trees with enabled=true get loaded if entries are still empty.
    if state.config.directory_tree.enabled {
        for wi in 0..state.workspaces.len() {
            if wi >= state.dir_trees.len() {
                break;
            }
            if state.dir_trees[wi].visible && state.dir_trees[wi].entries.is_empty() {
                let cwd = {
                    let ws = &state.workspaces[wi];
                    let tab = &ws.tabs[ws.active_tab];
                    let fid = tab.layout.focused();
                    let osc_cwd = tab
                        .panes
                        .get(&fid)
                        .map(|p| p.terminal.osc.cwd.clone())
                        .unwrap_or_default();
                    if osc_cwd.is_empty() {
                        std::env::var("HOME").unwrap_or_default()
                    } else {
                        osc_cwd
                    }
                };
                if !cwd.is_empty() {
                    let root = resolve_tree_root(&cwd, &state.config.directory_tree.root_mode);
                    load_tree_root(&mut state.dir_trees[wi], &root);
                }
            }
        }
    }

    // --- Submit workspace refresh requests to background thread (NON-BLOCKING) ---
    // Collect requests for workspaces that need refreshing, then hand them off
    // to the background thread so git/lsof/daemon queries don't freeze the UI.
    {
        let mut refresh_requests: Vec<WorkspaceRefreshRequest> = Vec::new();
        for wi in 0..state.workspaces.len() {
            if wi >= state.workspace_infos.len() {
                break;
            }
            let elapsed = state.workspace_infos[wi].last_updated.elapsed();
            let is_active = wi == state.active_workspace;
            let refresh_interval = if is_active { 5 } else { 30 };
            if elapsed.as_secs() >= refresh_interval || state.workspace_infos[wi].name.is_empty() {
                let (osc_cwd, pty_pid) = {
                    let ws = &state.workspaces[wi];
                    let tab = &ws.tabs[ws.active_tab];
                    let focused_id = tab.layout.focused();
                    let pane = tab.panes.get(&focused_id);
                    let c = pane.map(|p| p.terminal.osc.cwd.clone()).unwrap_or_default();
                    let pid = pane.map(|p| p.shell_pid);
                    (c, pid)
                };
                // Mark as "pending" so we don't re-submit every frame.
                state.workspace_infos[wi].last_updated = Instant::now();
                refresh_requests.push(WorkspaceRefreshRequest {
                    wi,
                    osc_cwd,
                    pty_pid,
                });
            }
        }
        if !refresh_requests.is_empty() {
            state.workspace_refresher.submit(refresh_requests);
        }
    }

    // --- Read latest results from background thread (NON-BLOCKING) ---
    {
        let bg_results = state.workspace_refresher.get_results();
        for (wi, info) in bg_results.into_iter().enumerate() {
            if wi < state.workspace_infos.len() {
                // Preserve has_unread flag (set by PTY output, not by background thread).
                let has_unread = state.workspace_infos[wi].has_unread;
                state.workspace_infos[wi] = info;
                state.workspace_infos[wi].has_unread = has_unread;
            }
        }
        state.daemon_sessions = state.workspace_refresher.get_daemon_sessions();
    }

    // --- Claude Code session monitor: submit pane PIDs and read results ---
    {
        use termojinal_claude::monitor::PaneInfo;
        let mut pane_infos: Vec<PaneInfo> = Vec::new();
        for (wi, ws) in state.workspaces.iter().enumerate() {
            for tab in &ws.tabs {
                for (&pane_id, pane) in &tab.panes {
                    pane_infos.push(PaneInfo {
                        pane_id,
                        workspace_idx: wi,
                        pty_pid: pane.shell_pid,
                    });
                }
            }
        }
        state.claude_monitor.submit_panes(pane_infos);

        // Update agent_infos from monitor results.
        let claude_sessions = state.claude_monitor.get_sessions();
        for cs in &claude_sessions {
            let wi = cs.workspace_idx;
            while state.agent_infos.len() <= wi {
                state.agent_infos.push(AgentSessionInfo::default());
            }
            let agent = &mut state.agent_infos[wi];
            // Only update from monitor if NOT currently in a PermissionRequest wait
            // (PermissionRequest has higher priority — set by IPC handler).
            let is_perm_wait = agent.active
                && matches!(agent.state, AgentState::WaitingForPermission)
                && state.allow_flow.has_pending_for_workspace(wi);
            if !is_perm_wait {
                // Detect session change: if session_id changed, clear stale
                // title from the previous session so the new session gets its
                // own title (from monitor or IPC).
                let session_changed = agent
                    .session_id
                    .as_ref()
                    .map(|old| old != &cs.session_id)
                    .unwrap_or(false);
                if session_changed {
                    agent.title = None;
                }

                agent.active = true;
                agent.pane_id = Some(cs.pane_id);
                agent.session_id = Some(cs.session_id.clone());
                // Only set title from monitor when no title exists yet.
                // IPC-provided titles have higher priority and must NOT be
                // overwritten by monitor (which reads raw JSONL first-message).
                if (agent.title.is_none() || agent.title.as_deref() == Some(""))
                    && !cs.title.is_empty()
                {
                    agent.title = Some(cs.title.clone());
                }
                agent.state = match cs.state {
                    termojinal_claude::monitor::SessionState::Running => AgentState::Running,
                    termojinal_claude::monitor::SessionState::Idle => AgentState::Idle,
                    termojinal_claude::monitor::SessionState::Done => AgentState::Inactive,
                    termojinal_claude::monitor::SessionState::WaitingForPermission => {
                        AgentState::WaitingForPermission
                    }
                };
                agent.subagent_count = cs.subagents.len();
                agent.subagents = cs
                    .subagents
                    .iter()
                    .map(|sa| SubAgentInfo {
                        title: if sa.description.is_empty() {
                            sa.agent_type.clone()
                        } else {
                            sa.description.clone()
                        },
                        state: match sa.state {
                            termojinal_claude::monitor::SessionState::Running => {
                                AgentState::Running
                            }
                            _ => AgentState::Inactive,
                        },
                    })
                    .collect();
                agent.last_updated = Instant::now();
            }
        }
        // Mark agents as inactive if monitor no longer sees them.
        for wi in 0..state.agent_infos.len() {
            let still_active = claude_sessions.iter().any(|cs| cs.workspace_idx == wi);
            if !still_active && state.agent_infos[wi].active {
                // Only mark inactive if it was monitor-tracked (has session_id).
                if state.agent_infos[wi].session_id.is_some() {
                    let is_perm_wait = matches!(
                        state.agent_infos[wi].state,
                        AgentState::WaitingForPermission
                    ) && state.allow_flow.has_pending_for_workspace(wi);
                    if !is_perm_wait {
                        state.agent_infos[wi].state = AgentState::Inactive;
                        state.agent_infos[wi].active = false;
                    }
                }
            }
        }
    }

    // --- Periodically prune stale session_to_workspace entries ---
    // Runs at most once per minute to avoid per-frame HashMap iteration.
    {
        static LAST_CLEANUP: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let now_secs = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        let prev = LAST_CLEANUP.load(std::sync::atomic::Ordering::Relaxed);
        if now_secs.saturating_sub(prev) >= 60 {
            LAST_CLEANUP.store(now_secs, std::sync::atomic::Ordering::Relaxed);
            cleanup_stale_session_mappings(state);
        }
    }

    // --- Draw workspace entries ---
    // Layout: [side_pad] [dot] [gap] [text ...] [side_pad]
    let dot_area = cell_w * 1.5; // dot character + small gap
    let text_left = side_pad + dot_area;
    let max_chars = ((sidebar_w - text_left - side_pad) / cell_w).max(1.0) as usize;
    let num_workspaces = state.workspaces.len();
    let mut entry_y = top_pad;

    // NOTE: The rest of render_sidebar is included via include_sidebar_body!
    // This is a workaround marker — the actual body follows below.

    for i in 0..num_workspaces {
        // Bail if not even the name line fits.
        if entry_y + cell_h > phys_h {
            break;
        }

        let is_active = i == state.active_workspace;
        let info = state.workspace_infos.get(i);
        let ws_color = WORKSPACE_COLORS[i % WORKSPACE_COLORS.len()];

        // Get additional workspace info: cwd and pane count.
        let ws_cwd = state
            .workspace_infos
            .get(i)
            .map(|inf| inf.cwd.clone())
            .unwrap_or_default();
        let ws_pane_count: usize = state.workspaces[i].tabs.iter().map(|t| t.panes.len()).sum();

        // Calculate entry height for active highlight background.
        let has_git = info.map_or(false, |inf| inf.git_branch.is_some());
        let has_ports = info.map_or(false, |inf| !inf.ports.is_empty());
        let has_cwd = !ws_cwd.is_empty();
        let has_allow_pending = state.allow_flow.has_pending_for_workspace(i);
        let mut content_h = cell_h; // name line
        if has_cwd {
            content_h += info_line_gap + cell_h;
        }
        if has_git {
            content_h += info_line_gap + cell_h;
        }
        if has_ports {
            content_h += info_line_gap + cell_h;
        }
        if has_allow_pending {
            content_h += info_line_gap + cell_h;
        }
        let has_agent =
            agent_status_enabled && i < state.agent_infos.len() && state.agent_infos[i].active;
        if has_agent {
            content_h += info_line_gap + cell_h;
            if !state.agent_infos[i].summary.is_empty() {
                content_h += info_line_gap + cell_h;
            }
        }

        let tree_visible = is_active
            && i < state.dir_trees.len()
            && state.dir_trees[i].visible
            && !state.dir_trees[i].entries.is_empty();

        let content_h = content_h.min(phys_h - entry_y);

        let entry_pad_y = entry_gap / 2.0;
        if has_allow_pending {
            state.renderer.submit_separator(
                view,
                0,
                (entry_y - entry_pad_y).max(0.0) as u32,
                3,
                (content_h + entry_pad_y * 2.0) as u32,
                allow_accent_color,
            );
        }

        let bg = if is_active {
            active_entry_bg
        } else {
            sidebar_bg
        };
        if is_active {
            let accent_w: u32 = 3;
            let bg_x = if has_allow_pending {
                accent_w
            } else {
                accent_w
            };
            state.renderer.submit_separator(
                view,
                bg_x,
                (entry_y - entry_pad_y).max(0.0) as u32,
                (sidebar_w as u32).saturating_sub(bg_x),
                (content_h + entry_pad_y * 2.0) as u32,
                active_entry_bg,
            );
            if !has_allow_pending {
                state.renderer.submit_separator(
                    view,
                    0,
                    (entry_y - entry_pad_y).max(0.0) as u32,
                    accent_w,
                    (content_h + entry_pad_y * 2.0) as u32,
                    ws_color,
                );
            }
        }

        // --- Workspace indicator dot ---
        let mut dot_color = if is_active {
            ws_color
        } else if info.map_or(false, |inf| inf.has_unread) {
            notification_dot
        } else {
            inactive_dot_color
        };
        if has_agent && agent_indicator_style == "pulse" {
            let elapsed = state.app_start_time.elapsed().as_secs_f32();
            let alpha = 0.5
                + 0.5 * (2.0 * std::f32::consts::PI * elapsed / agent_pulse_speed.max(0.1)).sin();
            let base = match state.agent_infos[i].state {
                AgentState::WaitingForPermission => agent_idle_color,
                _ => agent_active_color,
            };
            dot_color = [base[0], base[1], base[2], alpha];
        } else if has_agent && agent_indicator_style == "color" {
            dot_color = match state.agent_infos[i].state {
                AgentState::WaitingForPermission => agent_idle_color,
                _ => agent_active_color,
            };
        }
        let dot_diameter = cell_h * 0.50;
        let dot_radius = dot_diameter / 2.0;
        let dot_cx = side_pad + cell_w * 0.5;
        let dot_cy = entry_y + cell_h * 0.5;
        let filled = is_active || has_agent;
        state.renderer.submit_rounded_rects(
            view,
            &[RoundedRect {
                rect: [
                    dot_cx - dot_radius,
                    dot_cy - dot_radius,
                    dot_diameter,
                    dot_diameter,
                ],
                color: if filled {
                    dot_color
                } else {
                    [0.0, 0.0, 0.0, 0.0]
                },
                border_color: if filled { [0.0; 4] } else { dot_color },
                params: [dot_radius, if filled { 0.0 } else { 1.5 }, 0.0, 0.0],
            }],
        );

        // --- Notification dot for unread activity ---
        // Skip when the agent indicator is active — the pulsing/colored
        // dot already draws attention, and the two indicators overlap
        // visually at small cell sizes.
        if !is_active && !has_agent {
            if let Some(inf) = info {
                if inf.has_unread {
                    let notif_d = cell_h * 0.22;
                    let notif_r = notif_d / 2.0;
                    let notif_x = dot_cx + dot_radius * 1.2;
                    let notif_y = dot_cy - dot_radius * 1.2;
                    state.renderer.submit_rounded_rects(
                        view,
                        &[RoundedRect {
                            rect: [notif_x - notif_r, notif_y - notif_r, notif_d, notif_d],
                            color: notification_dot,
                            border_color: [0.0; 4],
                            params: [notif_r, 0.0, 0.0, 0.0],
                        }],
                    );
                }
            }
        }

        // --- Workspace name ---
        let display_name = if let Some(info) = info {
            if !info.name.is_empty() {
                info.name.clone()
            } else {
                if !ws_cwd.is_empty() {
                    std::path::Path::new(&ws_cwd)
                        .file_name()
                        .and_then(|n| n.to_str())
                        .unwrap_or("~")
                        .to_string()
                } else {
                    "~".to_string()
                }
            }
        } else if !ws_cwd.is_empty() {
            std::path::Path::new(&ws_cwd)
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("~")
                .to_string()
        } else {
            "~".to_string()
        };

        let tab_count = state.workspaces[i].tabs.len();
        let badge = if tab_count > 1 || ws_pane_count > 1 {
            let pane_str = if ws_pane_count > 1 {
                format!(" \u{25A8}{ws_pane_count}")
            } else {
                String::new()
            };
            format!(" [{tab_count}\u{25AB}{pane_str}]")
        } else {
            String::new()
        };

        let name_label = format!("{display_name}{badge}");
        let name_display: String = name_label.chars().take(max_chars).collect();
        let name_fg = if is_active { active_fg } else { inactive_fg };
        state
            .renderer
            .render_text(view, &name_display, text_left, entry_y, name_fg, bg);

        let mut line_y = entry_y + cell_h;

        // --- CWD line ---
        if has_cwd {
            line_y += info_line_gap;
            let cwd_short = if let Ok(home) = std::env::var("HOME") {
                if ws_cwd.starts_with(&home) {
                    format!("~{}", &ws_cwd[home.len()..])
                } else {
                    ws_cwd.clone()
                }
            } else {
                ws_cwd.clone()
            };
            let cwd_display: String = {
                let parts: Vec<&str> = cwd_short.rsplitn(3, '/').collect();
                if parts.len() >= 2 {
                    format!("\u{F07B} {}/{}", parts[1], parts[0])
                } else {
                    format!("\u{F07B} {cwd_short}")
                }
            };
            let info_indent = text_left + cell_w * 0.5;
            let cwd_trimmed: String = cwd_display
                .chars()
                .take(max_chars.saturating_sub(1))
                .collect();
            state
                .renderer
                .render_text(view, &cwd_trimmed, info_indent, line_y, dim_fg, bg);
            line_y += cell_h;
        }

        // --- Git info line ---
        if let Some(info) = info {
            if let Some(ref branch) = info.git_branch {
                line_y += info_line_gap;
                let mut git_parts = format!("\u{E0A0} {branch}");
                if info.git_ahead > 0 {
                    git_parts.push_str(&format!(" \u{21E1}{}", info.git_ahead));
                }
                if info.git_behind > 0 {
                    git_parts.push_str(&format!(" \u{21E3}{}", info.git_behind));
                }
                if info.git_dirty > 0 {
                    git_parts.push_str(&format!(" !{}", info.git_dirty));
                }
                if info.git_untracked > 0 {
                    git_parts.push_str(&format!(" ?{}", info.git_untracked));
                }

                let info_indent = text_left + cell_w * 0.5;
                let git_display: String = git_parts
                    .chars()
                    .take(max_chars.saturating_sub(1))
                    .collect();
                let git_fg = if is_active {
                    if info.git_dirty > 0 || info.git_untracked > 0 {
                        yellow_fg
                    } else {
                        git_branch_fg
                    }
                } else {
                    dim_fg
                };
                state
                    .renderer
                    .render_text(view, &git_display, info_indent, line_y, git_fg, bg);
                line_y += cell_h;
            }

            // --- Ports line ---
            if !info.ports.is_empty() {
                line_y += info_line_gap;
                let ports_str: String = format!(
                    "\u{F0AC} {}",
                    info.ports
                        .iter()
                        .map(|p| format!(":{p}"))
                        .collect::<Vec<_>>()
                        .join(" ")
                );
                let info_indent = text_left + cell_w * 0.5;
                let ports_display: String = ports_str
                    .chars()
                    .take(max_chars.saturating_sub(1))
                    .collect();
                state
                    .renderer
                    .render_text(view, &ports_display, info_indent, line_y, dim_fg, bg);
                line_y += cell_h;
            }
        }

        // --- Inline Allow Flow indicator ---
        if has_allow_pending {
            line_y += info_line_gap;
            let count = state.allow_flow.pending_count_for_workspace(i);
            let info_indent = text_left + cell_w * 0.5;
            let badge = format!("\u{26A1} {} pending", count);
            state
                .renderer
                .render_text(view, &badge, info_indent, line_y, allow_accent_color, bg);
            line_y += cell_h;
        }

        // --- Agent status lines ---
        if has_agent {
            let agent = &state.agent_infos[i];
            line_y += info_line_gap;
            let has_pending_inline = state.allow_flow.has_pending_for_workspace(i);
            let (agent_icon, state_label) = match agent.state {
                AgentState::Running => ("\u{26A1}", "running"),
                AgentState::WaitingForPermission => {
                    if has_pending_inline {
                        ("\u{23F3}", "wait you")
                    } else {
                        ("\u{26A1}", "running")
                    }
                }
                AgentState::Idle => {
                    if has_pending_inline {
                        ("\u{23F3}", "wait you")
                    } else {
                        ("\u{25CF}", "idle")
                    }
                }
                AgentState::Inactive => {
                    if has_pending_inline {
                        ("\u{23F3}", "wait you")
                    } else {
                        ("\u{2713}", "done")
                    }
                }
            };
            let done_inline_color: [f32; 4] = [0.45, 0.75, 0.45, 1.0];
            let agent_line = if agent.subagent_count > 0 {
                format!("{} {} (+{})", agent_icon, state_label, agent.subagent_count)
            } else {
                format!("{} {}", agent_icon, state_label)
            };
            let agent_fg = match state_label {
                "running" => agent_active_color,
                "wait you" => agent_idle_color,
                "done" => done_inline_color,
                _ => dim_fg,
            };
            let info_indent = text_left + cell_w * 0.5;
            let agent_display: String = agent_line.chars().take(max_chars).collect();
            state
                .renderer
                .render_text(view, &agent_display, info_indent, line_y, agent_fg, bg);
            line_y += cell_h;

            if !agent.summary.is_empty() {
                line_y += info_line_gap;
                let summary_display: String = agent
                    .summary
                    .chars()
                    .take(max_chars.saturating_sub(1))
                    .collect();
                state
                    .renderer
                    .render_text(view, &summary_display, info_indent, line_y, dim_fg, bg);
                line_y += cell_h;
            }
            let _ = line_y;
        }

        let _ = line_y;

        entry_y += content_h + entry_gap;

        // --- Standalone directory tree block ---
        let tree_visible = tree_visible && !state.dir_trees[i].entries.is_empty();
        if tree_visible {
            let tc = &state.config.directory_tree;
            let dir_fg = color_or(&tc.dir_fg, [0.54, 0.71, 0.98, 1.0]);
            let selected_fg_color = color_or(&tc.selected_fg, [0.95, 0.95, 0.97, 1.0]);
            let selected_bg_color = color_or(&tc.selected_bg, [0.19, 0.20, 0.27, 1.0]);
            let guide_fg = color_or(&tc.guide_fg, [0.42, 0.44, 0.53, 1.0]);
            let info_indent = text_left + cell_w * 0.5;
            let tree_accent_color: [f32; 4] = [0.35, 0.45, 0.55, 0.6];
            let guide_line_color: [f32; 4] = [0.25, 0.28, 0.35, 0.5];

            let mut below_h: f32 = 0.0;
            for j in (i + 1)..num_workspaces {
                let _ = j;
                below_h += cell_h * 2.0 + entry_gap;
            }
            below_h += 8.0 + 1.0 + 8.0 + cell_h;
            below_h += 16.0;

            let tree_start_y = entry_y;
            let tree_available_h = (phys_h - tree_start_y - below_h).max(cell_h * 3.0);
            let tree_lines = ((tree_available_h / cell_h) as usize)
                .saturating_sub(2)
                .max(3);

            state.dir_trees[i].current_visible_lines = tree_lines;

            let tree = &state.dir_trees[i];
            let entry_count = tree.entries.len();
            let visible_lines = entry_count.min(tree_lines);
            let total_lines = 1 + visible_lines + 1;
            let tree_block_h = info_line_gap + (total_lines as f32) * cell_h;
            let tree_bg = sidebar_bg;

            let accent_w: u32 = 3;
            state.renderer.submit_separator(
                view,
                0,
                tree_start_y as u32,
                accent_w,
                tree_block_h as u32,
                tree_accent_color,
            );

            let mut tree_y = tree_start_y + info_line_gap;

            let root_short = if let Ok(home) = std::env::var("HOME") {
                if tree.root_path.starts_with(&home) {
                    format!("~{}", &tree.root_path[home.len()..])
                } else {
                    tree.root_path.clone()
                }
            } else {
                tree.root_path.clone()
            };
            let header = format!("\u{25BE} {}", root_short);
            let header_display: String = header.chars().take(max_chars.saturating_sub(1)).collect();
            state.renderer.render_text(
                view,
                &header_display,
                info_indent,
                tree_y,
                guide_fg,
                tree_bg,
            );
            tree_y += cell_h;

            let scroll = tree.scroll_offset;
            let visible_end = (scroll + visible_lines).min(tree.entries.len());

            let scan_end = tree.entries.len().min(visible_end + 200);
            let max_depth = tree.entries[scroll..scan_end]
                .iter()
                .map(|e| e.depth)
                .max()
                .unwrap_or(0);
            let mut entry_is_last: Vec<bool> = vec![true; visible_end - scroll];
            let mut entry_continuations: Vec<Vec<bool>> =
                vec![vec![false; max_depth + 1]; visible_end - scroll];
            {
                let mut depth_has_more = vec![false; max_depth + 2];
                for j in (scroll..scan_end).rev() {
                    let d = tree.entries[j].depth;
                    if j < visible_end {
                        let vi = j - scroll;
                        entry_is_last[vi] = !depth_has_more[d];
                        for dd in 0..=max_depth {
                            entry_continuations[vi][dd] = depth_has_more[dd];
                        }
                    }
                    depth_has_more[d] = true;
                    for dd in (d + 1)..depth_has_more.len() {
                        depth_has_more[dd] = false;
                    }
                }
            }

            for ei in scroll..visible_end {
                let vi = ei - scroll;
                let entry = &tree.entries[ei];
                let depth_indent = info_indent + (entry.depth as f32) * cell_w * 1.5;
                let is_selected = ei == tree.selected;

                if entry.depth > 0 {
                    for d in 0..entry.depth {
                        let gx = info_indent + (d as f32) * cell_w * 1.5 + cell_w * 0.5;
                        if d < entry_continuations[vi].len() && entry_continuations[vi][d] {
                            state.renderer.submit_separator(
                                view,
                                gx as u32,
                                tree_y as u32,
                                1,
                                cell_h as u32,
                                guide_line_color,
                            );
                        }
                    }
                    let conn_x =
                        info_indent + ((entry.depth - 1) as f32) * cell_w * 1.5 + cell_w * 0.5;
                    let is_last = entry_is_last[vi];
                    let vert_h = if is_last {
                        (cell_h * 0.5) as u32
                    } else {
                        cell_h as u32
                    };
                    state.renderer.submit_separator(
                        view,
                        conn_x as u32,
                        tree_y as u32,
                        1,
                        vert_h,
                        guide_line_color,
                    );
                    let horiz_len = (cell_w * 0.8) as u32;
                    state.renderer.submit_separator(
                        view,
                        conn_x as u32,
                        (tree_y + cell_h * 0.5) as u32,
                        horiz_len,
                        1,
                        guide_line_color,
                    );
                }

                if is_selected && tree.focused {
                    state.renderer.submit_separator(
                        view,
                        info_indent as u32,
                        tree_y as u32,
                        (sidebar_w - info_indent - side_pad) as u32,
                        cell_h as u32,
                        selected_bg_color,
                    );
                }

                let (prefix, icon_color) = if entry.is_dir {
                    if entry.expanded {
                        ("\u{F07C} ", dir_fg)
                    } else {
                        ("\u{F07B} ", dir_fg)
                    }
                } else {
                    let ext_color = file_extension_color(&entry.name);
                    (file_icon(&entry.name), ext_color)
                };

                let fg = if is_selected && tree.focused {
                    selected_fg_color
                } else {
                    icon_color
                };
                let entry_bg = if is_selected && tree.focused {
                    selected_bg_color
                } else {
                    tree_bg
                };

                let label = format!("{}{}", prefix, entry.name);
                let avail = ((sidebar_w - depth_indent - side_pad) / cell_w).max(1.0) as usize;
                let display: String = label.chars().take(avail).collect();

                // Highlight matching prefix when find is active.
                let tree_ref = &state.dir_trees[i];
                if tree_ref.find_active && !tree_ref.find_query.is_empty() {
                    let query_lower = tree_ref.find_query.to_lowercase();
                    let name_lower = entry.name.to_lowercase();
                    if name_lower.starts_with(&query_lower) {
                        let prefix_char_count = prefix.chars().count();
                        let match_len = tree_ref.find_query.chars().count();
                        let hl_x = depth_indent + (prefix_char_count as f32) * cell_w;
                        let hl_w = (match_len as f32) * cell_w;
                        let match_hl_color = [0.45, 0.60, 0.85, 0.25];
                        state.renderer.submit_separator(
                            view,
                            hl_x as u32,
                            tree_y as u32,
                            hl_w as u32,
                            cell_h as u32,
                            match_hl_color,
                        );
                    }
                }

                state
                    .renderer
                    .render_text(view, &display, depth_indent, tree_y, fg, entry_bg);
                tree_y += cell_h;
            }

            let tree = &state.dir_trees[i];
            if tree.find_active {
                // Styled find input with border.
                let find_pad = 3.0_f32;
                let find_x = info_indent;
                let find_y = tree_y;
                let find_w = sidebar_w - info_indent - side_pad;
                let find_h = cell_h + find_pad * 2.0;
                let find_border_color = [0.45, 0.60, 0.85, 0.8];
                let find_bg_color = [0.08, 0.08, 0.12, 1.0];

                // Background fill.
                state.renderer.submit_separator(
                    view,
                    find_x as u32,
                    find_y as u32,
                    find_w as u32,
                    find_h as u32,
                    find_bg_color,
                );
                // Top border.
                state.renderer.submit_separator(
                    view,
                    find_x as u32,
                    find_y as u32,
                    find_w as u32,
                    1,
                    find_border_color,
                );
                // Bottom border.
                state.renderer.submit_separator(
                    view,
                    find_x as u32,
                    (find_y + find_h - 1.0) as u32,
                    find_w as u32,
                    1,
                    find_border_color,
                );
                // Left border.
                state.renderer.submit_separator(
                    view,
                    find_x as u32,
                    find_y as u32,
                    1,
                    find_h as u32,
                    find_border_color,
                );
                // Right border.
                state.renderer.submit_separator(
                    view,
                    (find_x + find_w - 1.0) as u32,
                    find_y as u32,
                    1,
                    find_h as u32,
                    find_border_color,
                );

                let icon = "\u{F002} "; // magnifying glass
                let q = format!("{}{}\u{2588}", icon, tree.find_query);
                let q_display: String = q.chars().take(max_chars.saturating_sub(2)).collect();
                state.renderer.render_text(
                    view,
                    &q_display,
                    find_x + find_pad + 2.0,
                    find_y + find_pad,
                    [0.80, 0.88, 1.0, 1.0],
                    find_bg_color,
                );
            } else {
                let (hint, hint_fg_col) = if tree.focused {
                    (
                        "j/k:nav  \u{21B5}:open  f:find  esc:close".to_string(),
                        [0.40, 0.45, 0.55, 0.8],
                    )
                } else {
                    ("Cmd+Shift+E to focus".to_string(), [0.40, 0.45, 0.55, 0.8])
                };
                let hint_display: String = hint.chars().take(max_chars.saturating_sub(1)).collect();
                state.renderer.render_text(
                    view,
                    &hint_display,
                    info_indent,
                    tree_y,
                    hint_fg_col,
                    tree_bg,
                );
            };

            entry_y += tree_block_h + entry_gap;
        }

        // --- Subtle separator line between entries ---
        if i < num_workspaces - 1 {
            let sep_line_y = (entry_y - entry_gap / 2.0) as u32;
            let sep_dim_color = [
                separator_color[0],
                separator_color[1],
                separator_color[2],
                0.4,
            ];
            if (sep_line_y as f32) + 1.0 < phys_h {
                state.renderer.submit_separator(
                    view,
                    (side_pad + dot_area * 0.5) as u32,
                    sep_line_y,
                    (sidebar_w - side_pad * 2.0 - dot_area * 0.5) as u32,
                    1,
                    sep_dim_color,
                );
            }
        }
    }

    // --- Separator line ---
    let sep_y = entry_y + 8.0;
    if sep_y + 1.0 < phys_h {
        state.renderer.submit_separator(
            view,
            side_pad as u32,
            sep_y as u32,
            (sidebar_w - 2.0 * side_pad) as u32,
            1,
            separator_color,
        );
    }

    // --- "New Workspace" button ---
    let new_ws_y = sep_y + 8.0 + 1.0;
    if new_ws_y + cell_h <= phys_h {
        let new_ws_label = "+ New Workspace";
        state
            .renderer
            .render_text(view, new_ws_label, side_pad, new_ws_y, dim_fg, sidebar_bg);
    }

    // --- Update available notice ---
    if let Some(ref ver) = state.update_checker.available_version {
        let update_y = new_ws_y + cell_h + 4.0;
        if update_y + cell_h <= phys_h {
            let update_fg = [1.0, 0.58, 0.16, 1.0];
            let update_label = format!("\u{F0176} v{ver} available", ver = ver);
            let update_display: String = update_label.chars().take(max_chars).collect();
            state.renderer.render_text(
                view,
                &update_display,
                side_pad,
                update_y,
                update_fg,
                sidebar_bg,
            );
        }
    }

    // --- Claudes Summary (bottom of sidebar) ---
    struct SubAgentEntry {
        title: String,
        state_label: &'static str,
    }
    struct ClaudeEntry {
        wi: usize,
        title: String,
        state_label: &'static str,
        subagents: Vec<SubAgentEntry>,
        worktree_display: String,
        branch: String,
    }
    let mut claude_entries: Vec<ClaudeEntry> = Vec::new();
    let done_color: [f32; 4] = [0.45, 0.75, 0.45, 1.0];
    for wi in 0..state.workspaces.len() {
        if wi >= state.agent_infos.len() {
            break;
        }
        let agent = &state.agent_infos[wi];
        if !agent.active {
            continue;
        }
        let title = if let Some(ref t) = agent.title {
            t.clone()
        } else {
            let ws_info = state.workspace_infos.get(wi);
            let name = ws_info.map(|i| i.name.as_str()).unwrap_or("");
            if !name.is_empty() {
                name.to_string()
            } else {
                let ws = &state.workspaces[wi];
                let tab = &ws.tabs[ws.active_tab];
                if !tab.display_title.is_empty() {
                    tab.display_title.clone()
                } else {
                    format!("Workspace {}", wi + 1)
                }
            }
        };
        let has_pending = state.allow_flow.has_pending_for_workspace(wi);
        let state_label: &'static str = match agent.state {
            AgentState::Running => "running",
            AgentState::WaitingForPermission => {
                if has_pending {
                    "wait you"
                } else {
                    "running"
                }
            }
            AgentState::Idle => {
                if has_pending {
                    "wait you"
                } else {
                    "idle"
                }
            }
            AgentState::Inactive => {
                if has_pending {
                    "wait you"
                } else {
                    "done"
                }
            }
        };
        let subagents: Vec<SubAgentEntry> = agent
            .subagents
            .iter()
            .map(|s| SubAgentEntry {
                title: s.title.clone(),
                state_label: match s.state {
                    AgentState::Running => "Run",
                    _ => "Done",
                },
            })
            .collect();
        let ws_info = state.workspace_infos.get(wi);
        let branch = ws_info
            .and_then(|i| i.git_branch.as_deref())
            .unwrap_or("")
            .to_string();
        let ws_cwd = state
            .workspace_infos
            .get(wi)
            .map(|inf| inf.cwd.clone())
            .unwrap_or_default();
        let worktree_display = if !ws_cwd.is_empty() {
            std::path::Path::new(&ws_cwd)
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_default()
        } else {
            String::new()
        };
        claude_entries.push(ClaudeEntry {
            wi,
            title,
            state_label,
            subagents,
            worktree_display,
            branch,
        });
    }

    if !claude_entries.is_empty() {
        let session_line_h = cell_h;
        let session_gap = info_line_gap;
        let session_pad_y = 3.0;
        let header_h = session_line_h + session_gap;
        let sessions_sep_h = 1.0 + 10.0 * 2.0;
        let bottom_pad = 8.0;
        let session_entry_gap = 4.0;

        let compute_entry_h = |entry: &ClaudeEntry| -> f32 {
            let base_lines = 2.0;
            let sub_lines = entry.subagents.len() as f32;
            (base_lines + sub_lines) * session_line_h
                + (base_lines + sub_lines - 1.0).max(0.0) * session_gap
                + session_pad_y * 2.0
        };

        let sessions_total_h = if state.claudes_collapsed {
            header_h
        } else {
            let entries_h: f32 = claude_entries.iter().map(|e| compute_entry_h(e)).sum();
            let gaps_h = (claude_entries.len().saturating_sub(1)) as f32 * session_entry_gap;
            header_h + entries_h + gaps_h
        };

        let sessions_start_y = phys_h - sessions_total_h - sessions_sep_h - bottom_pad;
        let min_start_y = new_ws_y + cell_h + 16.0;
        if sessions_start_y >= min_start_y {
            let sep_sess_y = sessions_start_y;
            let sep_upper_color = [
                separator_color[0],
                separator_color[1],
                separator_color[2],
                0.25,
            ];
            let sep_lower_color = [
                separator_color[0],
                separator_color[1],
                separator_color[2],
                0.5,
            ];
            state.renderer.submit_separator(
                view,
                (side_pad + dot_area * 0.5) as u32,
                (sep_sess_y + 9.0) as u32,
                (sidebar_w - side_pad * 2.0 - dot_area * 0.5) as u32,
                1,
                sep_upper_color,
            );
            state.renderer.submit_separator(
                view,
                (side_pad + dot_area * 0.5) as u32,
                (sep_sess_y + 11.0) as u32,
                (sidebar_w - side_pad * 2.0 - dot_area * 0.5) as u32,
                1,
                sep_lower_color,
            );

            let header_y = sep_sess_y + sessions_sep_h;
            let collapse_icon = if state.claudes_collapsed {
                "\u{25B8}"
            } else {
                "\u{25BE}"
            };
            let header_icon_fg = agent_active_color;
            state.renderer.render_text(
                view,
                collapse_icon,
                side_pad,
                header_y,
                header_icon_fg,
                sidebar_bg,
            );
            let total_count = claude_entries.len();
            let header_text = format!("Claudes ({})", total_count);
            let header_fg = [
                active_fg[0] * 0.8,
                active_fg[1] * 0.8,
                active_fg[2] * 0.8,
                0.9,
            ];
            state.renderer.render_text(
                view,
                &header_text,
                side_pad + cell_w * 2.0,
                header_y,
                header_fg,
                sidebar_bg,
            );

            if !state.claudes_collapsed {
                let mut sy = header_y + header_h;
                let info_indent = text_left + cell_w * 0.5;

                for (_idx, entry) in claude_entries.iter().enumerate() {
                    let entry_h = compute_entry_h(entry);
                    if sy + entry_h > phys_h - bottom_pad {
                        break;
                    }

                    let is_active_ws = entry.wi == state.active_workspace;

                    let card_bg = if is_active_ws {
                        active_entry_bg
                    } else {
                        [
                            sidebar_bg[0] + 0.02,
                            sidebar_bg[1] + 0.02,
                            sidebar_bg[2] + 0.025,
                            1.0,
                        ]
                    };
                    let accent_w: u32 = 3;
                    let card_x: u32 = accent_w;
                    let card_w = (sidebar_w as u32).saturating_sub(accent_w);
                    state.renderer.submit_separator(
                        view,
                        card_x,
                        sy as u32,
                        card_w,
                        entry_h as u32,
                        card_bg,
                    );

                    let accent_color = if is_active_ws {
                        WORKSPACE_COLORS[entry.wi % WORKSPACE_COLORS.len()]
                    } else {
                        let ws_col = WORKSPACE_COLORS[entry.wi % WORKSPACE_COLORS.len()];
                        [ws_col[0] * 0.5, ws_col[1] * 0.5, ws_col[2] * 0.5, 0.6]
                    };
                    state.renderer.submit_separator(
                        view,
                        0,
                        sy as u32,
                        accent_w,
                        entry_h as u32,
                        accent_color,
                    );

                    let content_y = sy + session_pad_y;

                    let ws_col = WORKSPACE_COLORS[entry.wi % WORKSPACE_COLORS.len()];
                    let mut dot_col = ws_col;
                    if entry.state_label == "running" && agent_indicator_style == "pulse" {
                        let elapsed = state.app_start_time.elapsed().as_secs_f32();
                        let alpha = 0.5
                            + 0.5
                                * (2.0 * std::f32::consts::PI * elapsed
                                    / agent_pulse_speed.max(0.1))
                                .sin();
                        dot_col = [
                            agent_active_color[0],
                            agent_active_color[1],
                            agent_active_color[2],
                            alpha,
                        ];
                    } else if entry.state_label == "wait you" {
                        dot_col = agent_idle_color;
                    } else if entry.state_label == "idle" {
                        dot_col = [dim_fg[0], dim_fg[1], dim_fg[2], 0.6];
                    } else if entry.state_label == "done" {
                        dot_col = done_color;
                    }
                    let sd = cell_h * 0.45;
                    let sr = sd / 2.0;
                    let scx = side_pad + 2.0 + cell_w * 0.5;
                    let scy = content_y + cell_h * 0.5;
                    state.renderer.submit_rounded_rects(
                        view,
                        &[RoundedRect {
                            rect: [scx - sr, scy - sr, sd, sd],
                            color: dot_col,
                            border_color: [0.0; 4],
                            params: [sr, 0.0, 0.0, 0.0],
                        }],
                    );
                    let title_display: String = entry
                        .title
                        .chars()
                        .take(max_chars.saturating_sub(1))
                        .collect();
                    let title_fg = if is_active_ws {
                        active_fg
                    } else {
                        [
                            active_fg[0] * 0.85,
                            active_fg[1] * 0.85,
                            active_fg[2] * 0.85,
                            0.95,
                        ]
                    };
                    state.renderer.render_text(
                        view,
                        &title_display,
                        text_left,
                        content_y,
                        title_fg,
                        card_bg,
                    );

                    let line2_y = content_y + session_line_h + session_gap;
                    let state_color = match entry.state_label {
                        "running" => agent_active_color,
                        "wait you" => agent_idle_color,
                        "idle" => dim_fg,
                        "done" => done_color,
                        _ => dim_fg,
                    };
                    let (state_icon, state_text) = match entry.state_label {
                        "running" => ("\u{25B6}", "Run"),
                        "wait you" => ("\u{23F3}", "Wait"),
                        "idle" => ("\u{25CF}", "Idle"),
                        "done" => ("\u{2713}", "Done"),
                        _ => ("\u{25CF}", entry.state_label),
                    };
                    let label_fg = state_color;
                    let icon_x = info_indent;
                    state
                        .renderer
                        .render_text(view, state_icon, icon_x, line2_y, label_fg, card_bg);
                    let state_text_x = icon_x + cell_w * 2.0;
                    state.renderer.render_text(
                        view,
                        state_text,
                        state_text_x,
                        line2_y,
                        label_fg,
                        card_bg,
                    );
                    let location_info = if !entry.branch.is_empty() {
                        if !entry.worktree_display.is_empty() {
                            format!("({}, {})", entry.worktree_display, entry.branch)
                        } else {
                            format!("({})", entry.branch)
                        }
                    } else if !entry.worktree_display.is_empty() {
                        format!("({})", entry.worktree_display)
                    } else {
                        String::new()
                    };
                    if !location_info.is_empty() {
                        let loc_x = state_text_x + (state_text.len() as f32 + 1.0) * cell_w;
                        let loc_fg = [dim_fg[0], dim_fg[1], dim_fg[2], 0.8];
                        let loc_display: String = location_info
                            .chars()
                            .take(max_chars.saturating_sub(8))
                            .collect();
                        state.renderer.render_text(
                            view,
                            &loc_display,
                            loc_x,
                            line2_y,
                            loc_fg,
                            card_bg,
                        );
                    }

                    // --- Subagent lines ---
                    let sub_indent = info_indent + cell_w * 1.5;
                    let sub_run_color: [f32; 4] = [0.65, 0.55, 0.98, 0.9];
                    let sub_done_color: [f32; 4] = [0.45, 0.72, 0.45, 0.75];
                    for (si, sub) in entry.subagents.iter().enumerate() {
                        let sub_y = line2_y + (si as f32 + 1.0) * (session_line_h + session_gap);
                        let is_running = sub.state_label == "Run";
                        let sub_state_color = if is_running {
                            sub_run_color
                        } else {
                            sub_done_color
                        };
                        let sub_icon = if is_running { "\u{25B6}" } else { "\u{2713}" };
                        let sub_title_fg = if is_running {
                            [0.82, 0.78, 0.95, 0.95]
                        } else {
                            [dim_fg[0] + 0.1, dim_fg[1] + 0.1, dim_fg[2] + 0.1, 0.7]
                        };
                        let tree_char = "\u{2514}";
                        state.renderer.render_text(
                            view,
                            tree_char,
                            sub_indent,
                            sub_y,
                            [dim_fg[0], dim_fg[1], dim_fg[2], 0.5],
                            card_bg,
                        );
                        let icon_x = sub_indent + cell_w * 1.5;
                        state.renderer.render_text(
                            view,
                            sub_icon,
                            icon_x,
                            sub_y,
                            sub_state_color,
                            card_bg,
                        );
                        let state_x = icon_x + cell_w * 1.5;
                        state.renderer.render_text(
                            view,
                            sub.state_label,
                            state_x,
                            sub_y,
                            sub_state_color,
                            card_bg,
                        );
                        let title_x = state_x + (sub.state_label.len() as f32 + 1.0) * cell_w;
                        let sub_display: String = sub
                            .title
                            .chars()
                            .take(max_chars.saturating_sub(10))
                            .collect();
                        state.renderer.render_text(
                            view,
                            &sub_display,
                            title_x,
                            sub_y,
                            sub_title_fg,
                            card_bg,
                        );
                    }

                    sy += entry_h + session_entry_gap;
                }

                // Daemon sessions intentionally not shown in Claudes section.
            }
        }
    }
}
