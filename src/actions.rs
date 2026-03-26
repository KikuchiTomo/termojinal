//! Action dispatch.

use crate::*;
use termojinal_ipc::keybinding::Action;
use termojinal_layout::{Direction, SplitDirection};
use winit::event_loop::ActiveEventLoop;

pub(crate) fn dispatch_action(
    state: &mut AppState,
    action: &Action,
    proxy: &EventLoopProxy<UserEvent>,
    buffers: &Arc<Mutex<HashMap<PaneId, VecDeque<Vec<u8>>>>>,
    event_loop: &ActiveEventLoop,
) -> bool {
    // Dismiss close confirmation dialog when any other action is dispatched.
    if state.pending_close_confirm.is_some() && !matches!(action, Action::CloseTab) {
        state.pending_close_confirm = None;
    }

    let focused_id = active_tab(state).layout.focused();
    match action {
        Action::SplitRight => {
            let cwd = resolve_new_pane_cwd(state);
            let next_id = state.next_pane_id;
            let tab = active_tab_mut(state);
            tab.layout.set_next_id(next_id);
            let (new_layout, new_id) = tab.layout.split(focused_id, SplitDirection::Horizontal);
            tab.layout = new_layout;
            state.next_pane_id = new_id + 1;
            let pane_rects = active_pane_rects(state);
            let new_rect = pane_rects
                .iter()
                .find(|(id, _)| *id == new_id)
                .map(|(_, r)| *r);
            if let Some(rect) = new_rect {
                let (cols, rows) = state.renderer.grid_size_raw(rect.w as u32, rect.h as u32);
                match spawn_pane(
                    new_id,
                    cols.max(1),
                    rows.max(1),
                    proxy,
                    buffers,
                    cwd,
                    Some(&state.config.time_travel),
                    state.renderer.cjk_width,
                ) {
                    Ok(pane) => {
                        active_tab_mut(state).panes.insert(new_id, pane);
                    }
                    Err(e) => {
                        log::error!("failed to spawn pane: {e}");
                    }
                }
            }
            resize_all_panes(state);
            state.window.request_redraw();
            true
        }
        Action::SplitDown => {
            let cwd = resolve_new_pane_cwd(state);
            let next_id = state.next_pane_id;
            let tab = active_tab_mut(state);
            tab.layout.set_next_id(next_id);
            let (new_layout, new_id) = tab.layout.split(focused_id, SplitDirection::Vertical);
            tab.layout = new_layout;
            state.next_pane_id = new_id + 1;
            let pane_rects = active_pane_rects(state);
            let new_rect = pane_rects
                .iter()
                .find(|(id, _)| *id == new_id)
                .map(|(_, r)| *r);
            if let Some(rect) = new_rect {
                let (cols, rows) = state.renderer.grid_size_raw(rect.w as u32, rect.h as u32);
                match spawn_pane(
                    new_id,
                    cols.max(1),
                    rows.max(1),
                    proxy,
                    buffers,
                    cwd,
                    Some(&state.config.time_travel),
                    state.renderer.cjk_width,
                ) {
                    Ok(pane) => {
                        active_tab_mut(state).panes.insert(new_id, pane);
                    }
                    Err(e) => {
                        log::error!("failed to spawn pane: {e}");
                    }
                }
            }
            resize_all_panes(state);
            state.window.request_redraw();
            true
        }
        Action::ZoomPane => {
            let tab = active_tab_mut(state);
            tab.layout = tab.layout.toggle_zoom();
            resize_all_panes(state);
            state.window.request_redraw();
            true
        }
        Action::NextPane => {
            clear_focused_preedit(state);
            let tab = active_tab_mut(state);
            tab.layout = tab.layout.navigate(Direction::Next);
            let fmt = state.config.tab_bar.format.clone();
            let fb_cwd = state.pane_git_cache.cwd.clone();
            let ws = active_ws_mut(state);
            let ti = ws.active_tab;
            update_tab_title(&mut ws.tabs[ti], &fmt, ti + 1, &fb_cwd);
            update_window_title(state);
            update_tree_root_for_focused_pane(state);
            state.window.request_redraw();
            true
        }
        Action::PrevPane => {
            clear_focused_preedit(state);
            let tab = active_tab_mut(state);
            tab.layout = tab.layout.navigate(Direction::Prev);
            let fmt = state.config.tab_bar.format.clone();
            let fb_cwd = state.pane_git_cache.cwd.clone();
            let ws = active_ws_mut(state);
            let ti = ws.active_tab;
            update_tab_title(&mut ws.tabs[ti], &fmt, ti + 1, &fb_cwd);
            update_window_title(state);
            update_tree_root_for_focused_pane(state);
            state.window.request_redraw();
            true
        }
        Action::NewTab => {
            // Create a new tab in the current workspace.
            let cwd = resolve_new_pane_cwd(state);
            let new_id = state.next_pane_id;
            state.next_pane_id += 1;
            let layout = LayoutTree::new(new_id);
            let size = state.window.inner_size();
            let phys_w = size.width as f32;
            let phys_h = size.height as f32;
            // When we add a new tab, the workspace will have >1 tabs, so tab bar will appear.
            let sidebar_w = if state.sidebar_visible {
                state.sidebar_width
            } else {
                0.0
            };
            let cw = (phys_w - sidebar_w).max(1.0);
            let ch = (phys_h - state.config.tab_bar.height).max(1.0);
            let (cols, rows) = state.renderer.grid_size_raw(cw as u32, ch as u32);
            match spawn_pane(
                new_id,
                cols.max(1),
                rows.max(1),
                proxy,
                buffers,
                cwd,
                Some(&state.config.time_travel),
                state.renderer.cjk_width,
            ) {
                Ok(pane) => {
                    let mut panes = HashMap::new();
                    panes.insert(new_id, pane);
                    let fmt = state.config.tab_bar.format.clone();
                    let ws = active_ws_mut(state);
                    let tab_num = ws.tabs.len() + 1;
                    let tab = Tab {
                        layout,
                        panes,
                        name: format!("Tab {tab_num}"),
                        display_title: format_tab_title(&fmt, "", "", tab_num),
                    };
                    ws.tabs.push(tab);
                    ws.active_tab = ws.tabs.len() - 1;
                    resize_all_panes(state);
                    update_window_title(state);
                    state.window.request_redraw();
                }
                Err(e) => {
                    log::error!("failed to spawn pane for new tab: {e}");
                }
            }
            true
        }
        Action::CloseTab => {
            // Check if there's a running child process before closing.
            let pane_info = {
                let tab = active_tab(state);
                let focused_id = tab.layout.focused();
                tab.panes
                    .get(&focused_id)
                    .map(|p| (focused_id, p.shell_pid))
            };
            if let Some((pane_id, pid)) = pane_info {
                if let Some(proc_name) = detect_foreground_child(pid) {
                    // Show confirmation dialog instead of closing immediately.
                    state.pending_close_confirm = Some((proc_name, pane_id));
                    state.window.request_redraw();
                    return true;
                }
            }
            close_focused_pane(state, buffers, event_loop);
            true
        }
        Action::NewWorkspace => {
            // Create a new workspace with one tab and one pane.
            let cwd = resolve_new_pane_cwd(state);
            let new_id = state.next_pane_id;
            state.next_pane_id += 1;
            let layout = LayoutTree::new(new_id);
            let size = state.window.inner_size();
            let phys_w = size.width as f32;
            let phys_h = size.height as f32;
            let sidebar_w = if state.sidebar_visible {
                state.sidebar_width
            } else {
                0.0
            };
            let cw = (phys_w - sidebar_w).max(1.0);
            let ch = phys_h.max(1.0); // Single tab, no tab bar
            let (cols, rows) = state.renderer.grid_size_raw(cw as u32, ch as u32);
            match spawn_pane(
                new_id,
                cols.max(1),
                rows.max(1),
                proxy,
                buffers,
                cwd,
                Some(&state.config.time_travel),
                state.renderer.cjk_width,
            ) {
                Ok(pane) => {
                    let mut panes = HashMap::new();
                    panes.insert(new_id, pane);
                    let ws_num = state.workspaces.len() + 1;
                    let fmt = state.config.tab_bar.format.clone();
                    let tab = Tab {
                        layout,
                        panes,
                        name: "Tab 1".to_string(),
                        display_title: format_tab_title(&fmt, "", "", 1),
                    };
                    let ws = Workspace {
                        tabs: vec![tab],
                        active_tab: 0,
                        name: format!("Workspace {ws_num}"),
                    };
                    state.workspaces.push(ws);
                    state.agent_infos.push(AgentSessionInfo::default());
                    let mut new_tree = DirectoryTreeState::new();
                    if state.config.directory_tree.enabled {
                        new_tree.visible = true;
                    }
                    state.dir_trees.push(new_tree);
                    state.active_workspace = state.workspaces.len() - 1;
                    resize_all_panes(state);
                    update_window_title(state);
                    state.window.request_redraw();
                }
                Err(e) => {
                    log::error!("failed to spawn pane for new workspace: {e}");
                }
            }
            true
        }
        Action::NextTab => {
            clear_focused_preedit(state);
            let ws = active_ws_mut(state);
            if ws.active_tab + 1 < ws.tabs.len() {
                ws.active_tab += 1;
                resize_all_panes(state);
                update_window_title(state);
                state.window.request_redraw();
            }
            true
        }
        Action::PrevTab => {
            clear_focused_preedit(state);
            let ws = active_ws_mut(state);
            if ws.active_tab > 0 {
                ws.active_tab -= 1;
                resize_all_panes(state);
                update_window_title(state);
                state.window.request_redraw();
            }
            true
        }
        Action::Workspace(n) => {
            clear_focused_preedit(state);
            let idx = (*n as usize).saturating_sub(1);
            if idx < state.workspaces.len() {
                state.active_workspace = idx;
                if idx < state.workspace_infos.len() {
                    state.workspace_infos[idx].has_unread = false;
                }
                resize_all_panes(state);
                update_window_title(state);
                state.window.request_redraw();
            }
            true
        }
        Action::CommandPalette => {
            // If a command execution is active, cancel it on toggle off.
            if state.command_palette.visible {
                if let Some(ref mut exec) = state.command_execution {
                    exec.runner.cancel();
                }
                state.command_execution = None;
            }
            state.command_palette.toggle();
            // Initialize file finder with focused pane's CWD.
            if state.command_palette.visible {
                let cwd = {
                    let tab = active_tab(state);
                    let pane = tab.panes.get(&focused_id);
                    pane.and_then(|p| {
                        // Prefer OSC 7 CWD (shell-reported, always up-to-date).
                        let osc_cwd = &p.terminal.osc.cwd;
                        if !osc_cwd.is_empty() {
                            return Some(osc_cwd.clone());
                        }
                        // Fallback: inspect the child process's CWD via lsof.
                        let pty_pid = p.shell_pid;
                        if pty_pid > 0 {
                            if let Some(cwd) = get_child_cwd(pty_pid) {
                                return Some(cwd);
                            }
                        }
                        None
                    })
                    .unwrap_or_else(|| {
                        std::env::current_dir()
                            .map(|p| p.to_string_lossy().to_string())
                            .unwrap_or_else(|_| "/".to_string())
                    })
                };
                state.command_palette.init_file_finder(&cwd);
            }
            state.window.request_redraw();
            true
        }
        Action::Copy => {
            if let Some(pane) = active_tab_mut(state).panes.get_mut(&focused_id) {
                if let Some(ref sel) = pane.selection {
                    let text = sel.text(&pane.terminal);
                    if !text.is_empty() {
                        // Build RTF with colors and formatting preserved.
                        let cell_rows = sel.cells(&pane.terminal);
                        let palette = &state.renderer.theme_palette;
                        let rtf = cells_to_rtf(&cell_rows, palette);
                        copy_to_clipboard_with_rtf(&text, &rtf);
                        // Keep selection visible after copy.
                        state.window.request_redraw();
                        return true;
                    }
                }
            }
            // No selection — send Ctrl+C to the focused pane.
            if let Some(pane) = active_tab(state).panes.get(&focused_id) {
                let _ = daemon_pty_write(&pane.session_id, &[0x03]);
            }
            true
        }
        Action::Paste => {
            if let Some(pane) = active_tab(state).panes.get(&focused_id) {
                if let Ok(mut cb) = arboard::Clipboard::new() {
                    if let Ok(text) = cb.get_text() {
                        if pane.terminal.modes.bracketed_paste {
                            let _ = daemon_pty_write(&pane.session_id, b"\x1b[200~");
                            let _ = daemon_pty_write(&pane.session_id, text.as_bytes());
                            let _ = daemon_pty_write(&pane.session_id, b"\x1b[201~");
                        } else {
                            let _ = daemon_pty_write(&pane.session_id, text.as_bytes());
                        }
                    }
                }
            }
            true
        }
        Action::SelectAll => {
            if let Some(pane) = active_tab_mut(state).panes.get_mut(&focused_id) {
                let cols = pane.terminal.cols();
                let rows = pane.terminal.rows();
                let sb_len = pane.terminal.scrollback_len();
                // Select from the very top of scrollback to the bottom-right of the visible grid.
                // Scroll to the top so the start of selection is visible.
                pane.terminal.set_scroll_offset(sb_len);
                pane.selection = Some(Selection {
                    start: GridPos { col: 0, row: 0 },
                    end: GridPos {
                        col: cols.saturating_sub(1),
                        row: rows.saturating_sub(1),
                    },
                    active: false,
                    scroll_offset_at_start: sb_len,
                    scroll_offset_at_end: 0,
                });
                state.window.request_redraw();
            }
            true
        }
        Action::ClearScreen => {
            let focused_id = active_tab(state).layout.focused();
            if let Some(pane) = active_tab_mut(state).panes.get(&focused_id) {
                let _ = daemon_pty_write(&pane.session_id, b"\x1b[2J\x1b[H");
            }
            true
        }
        Action::ClearScrollback => {
            let focused_id = active_tab(state).layout.focused();
            if let Some(pane) = active_tab_mut(state).panes.get_mut(&focused_id) {
                pane.terminal.clear_all();
            }
            state.window.request_redraw();
            true
        }
        Action::Quit => {
            // Signal all background threads to stop.
            state.shutdown.store(true, std::sync::atomic::Ordering::SeqCst);
            state.status_collector.shutdown.store(true, std::sync::atomic::Ordering::SeqCst);
            state.workspace_refresher.shutdown.store(true, std::sync::atomic::Ordering::SeqCst);
            // Detach PTY handles so daemon-tracked sessions survive the GUI exit.
            detach_all_ptys(state);
            event_loop.exit();
            true
        }
        Action::ToggleSidebar => {
            state.sidebar_visible = !state.sidebar_visible;
            resize_all_panes(state);
            state.window.request_redraw();
            true
        }
        Action::ToggleDirectoryTree => {
            let wi = state.active_workspace;
            while state.dir_trees.len() <= wi {
                state.dir_trees.push(DirectoryTreeState::new());
            }
            let now_visible = !state.dir_trees[wi].visible;
            state.dir_trees[wi].visible = now_visible;
            if now_visible {
                // Ensure sidebar is visible when showing tree.
                if !state.sidebar_visible {
                    state.sidebar_visible = true;
                    resize_all_panes(state);
                }
                // Load tree if root changed or not yet loaded.
                let cwd = {
                    let ws = &state.workspaces[wi];
                    let tab = &ws.tabs[ws.active_tab];
                    let fid = tab.layout.focused();
                    let pane = tab.panes.get(&fid);
                    let osc_cwd = pane.map(|p| p.terminal.osc.cwd.clone()).unwrap_or_default();
                    if !osc_cwd.is_empty() {
                        osc_cwd
                    } else {
                        // lsof fallback for CWD detection.
                        let pty_cwd = pane.and_then(|p| {
                            let pid = p.shell_pid;
                            if pid > 0 {
                                get_child_cwd(pid)
                            } else {
                                None
                            }
                        });
                        pty_cwd.unwrap_or_else(|| {
                            std::env::current_dir()
                                .map(|p| p.to_string_lossy().to_string())
                                .unwrap_or_else(|_| std::env::var("HOME").unwrap_or_default())
                        })
                    }
                };
                let root = resolve_tree_root(&cwd, &state.config.directory_tree.root_mode);
                if state.dir_trees[wi].entries.is_empty() || state.dir_trees[wi].root_path != root {
                    load_tree_root(&mut state.dir_trees[wi], &root);
                }
                state.dir_trees[wi].focused = true;
            } else {
                state.dir_trees[wi].focused = false;
            }
            state.window.request_redraw();
            true
        }
        Action::NextWorkspace => {
            clear_focused_preedit(state);
            if state.active_workspace + 1 < state.workspaces.len() {
                state.active_workspace += 1;
                resize_all_panes(state);
                update_window_title(state);
                update_tree_root_for_focused_pane(state);
                state.window.request_redraw();
            }
            true
        }
        Action::PrevWorkspace => {
            clear_focused_preedit(state);
            if state.active_workspace > 0 {
                state.active_workspace -= 1;
                resize_all_panes(state);
                update_window_title(state);
                update_tree_root_for_focused_pane(state);
                state.window.request_redraw();
            }
            true
        }
        Action::FontIncrease => {
            let new_size =
                (state.font_size + state.config.font.size_step).min(state.config.font.max_size);
            if let Err(e) = state.renderer.set_font_size(new_size) {
                log::error!("failed to increase font size: {e}");
            } else {
                state.font_size = new_size;
                resize_all_panes(state);
                state.window.request_redraw();
            }
            true
        }
        Action::FontDecrease => {
            let new_size = (state.font_size - state.config.font.size_step).max(6.0);
            if let Err(e) = state.renderer.set_font_size(new_size) {
                log::error!("failed to decrease font size: {e}");
            } else {
                state.font_size = new_size;
                resize_all_panes(state);
                state.window.request_redraw();
            }
            true
        }
        Action::Search => {
            if state.search.is_some() {
                state.search = None;
            } else {
                state.search = Some(SearchState::new());
            }
            state.window.request_redraw();
            true
        }
        Action::Passthrough => {
            // Force key through to PTY, skip binding.
            false
        }
        Action::None => true,
        Action::AllowFlowPanel => {
            // Open sidebar if closed, then jump to the first workspace
            // with pending Allow Flow requests.
            if !state.sidebar_visible {
                state.sidebar_visible = true;
            }
            if let Some(ws_idx) = state.allow_flow.first_workspace_with_pending() {
                if state.active_workspace != ws_idx {
                    state.active_workspace = ws_idx;
                    resize_all_panes(state);
                    update_window_title(state);
                }
            }
            state.window.request_redraw();
            true
        }
        Action::QuickLaunch => {
            state.quick_launch.toggle();
            if state.quick_launch.visible {
                state
                    .quick_launch
                    .rebuild_entries(&state.workspaces, &state.workspace_infos);
            }
            state.window.request_redraw();
            true
        }
        Action::Command(name) => {
            if let Some(cmd) = state
                .external_commands
                .iter()
                .find(|c| c.meta.name == *name)
            {
                match CommandExecution::new(cmd) {
                    Ok(exec) => {
                        state.command_execution = Some(exec);
                        state.command_palette.visible = true;
                        state.window.request_redraw();
                    }
                    Err(e) => log::error!("failed to start command '{}': {}", name, e),
                }
            } else {
                log::warn!("unknown command: {}", name);
            }
            true
        }
        Action::ToggleQuickTerminal => {
            toggle_quick_terminal(state);
            true
        }
        Action::About => {
            state.about_visible = !state.about_visible;
            state.about_scroll = 0;
            state.window.request_redraw();
            true
        }
        Action::ClaudesDashboard => {
            state.claudes_dashboard.toggle();
            if state.claudes_dashboard.visible {
                refresh_claudes_dashboard(state);
            }
            state.window.request_redraw();
            true
        }
        Action::PrevCommand => {
            if state.config.time_travel.command_navigation {
                let focused_id = active_tab(state).layout.focused();
                if let Some(pane) = active_tab_mut(state).panes.get_mut(&focused_id) {
                    pane.terminal.jump_to_prev_command();
                }
                state.window.request_redraw();
            }
            true
        }
        Action::NextCommand => {
            if state.config.time_travel.command_navigation {
                let focused_id = active_tab(state).layout.focused();
                if let Some(pane) = active_tab_mut(state).panes.get_mut(&focused_id) {
                    pane.terminal.jump_to_next_command();
                }
                state.window.request_redraw();
            }
            true
        }
        Action::FirstCommand => {
            if state.config.time_travel.command_navigation {
                let focused_id = active_tab(state).layout.focused();
                if let Some(pane) = active_tab_mut(state).panes.get_mut(&focused_id) {
                    if let Some(first) = pane.terminal.command_history().front() {
                        let id = first.id;
                        pane.terminal.jump_to_command(id);
                    }
                }
                state.window.request_redraw();
            }
            true
        }
        Action::LastCommand => {
            if state.config.time_travel.command_navigation {
                let focused_id = active_tab(state).layout.focused();
                if let Some(pane) = active_tab_mut(state).panes.get_mut(&focused_id) {
                    pane.terminal.set_scroll_offset(0);
                }
                state.window.request_redraw();
            }
            true
        }
        Action::CommandTimeline => {
            if state.config.time_travel.timeline_ui {
                state.timeline_visible = !state.timeline_visible;
                if state.timeline_visible {
                    state.timeline_input.clear();
                    state.timeline_selected = 0;
                    state.timeline_scroll_offset = 0;
                    // S4: Remember which pane the timeline was opened for
                    state.timeline_pane_id = Some(active_tab(state).layout.focused());
                } else {
                    state.timeline_pane_id = None;
                }
                state.window.request_redraw();
            }
            true
        }
        Action::CreateSnapshot => {
            if state.config.time_travel.snapshots {
                // Snapshot creation will be handled in the snapshot module
                log::debug!("snapshot creation requested (time_travel.snapshots)");
            }
            true
        }
        Action::ExtractPaneToTab => {
            let tab = active_tab(state);
            let focused_id = tab.layout.focused();
            // Only extract if the tab has more than one pane.
            if tab.layout.pane_count() <= 1 {
                return true;
            }
            // Extract the focused pane from the current tab's layout.
            let extract_result = tab.layout.extract_pane(focused_id);
            if let Some((remaining_layout, extracted_layout)) = extract_result {
                // Update the current tab's layout (pane removed).
                let tab = active_tab_mut(state);
                // Remove the pane from the current tab's pane map.
                let pane = tab.panes.remove(&focused_id);
                tab.layout = remaining_layout;

                // Create a new tab with the extracted pane.
                if let Some(pane) = pane {
                    let mut panes = HashMap::new();
                    panes.insert(focused_id, pane);
                    let fmt = state.config.tab_bar.format.clone();
                    let ws = active_ws_mut(state);
                    let tab_num = ws.tabs.len() + 1;
                    let new_tab = Tab {
                        layout: extracted_layout,
                        panes,
                        name: format!("Tab {tab_num}"),
                        display_title: format_tab_title(&fmt, "", "", tab_num),
                    };
                    ws.tabs.push(new_tab);
                    ws.active_tab = ws.tabs.len() - 1;
                }
                resize_all_panes(state);
                update_window_title(state);
                state.window.request_redraw();
            }
            true
        }
        Action::UnreadJump | Action::OpenSettings => {
            log::debug!("unhandled action: {:?}", action);
            true
        }
    }
}
