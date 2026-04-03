use crate::*;
use config::color_or;
use termojinal_claude::monitor::SessionState;
use termojinal_render::color_convert::color_to_rgba;
use termojinal_render::RoundedRect;

// ---------------------------------------------------------------------------
// Claudes Dashboard — data refresh and rendering
// ---------------------------------------------------------------------------

pub(crate) fn refresh_claudes_dashboard(state: &mut AppState) {
    let sessions = state.claude_monitor.get_sessions();
    let mut entries = Vec::new();

    for session in &sessions {
        // Read JSONL stats (model, tokens, cost, tool usage).
        let stats =
            termojinal_claude::monitor::read_session_jsonl_stats(&session.session_id, &session.cwd);

        // Compute total context used (input + output + cache_read).
        let context_used = stats.input_tokens + stats.output_tokens + stats.cache_read_tokens;

        // Find workspace name.
        let workspace_name = if session.workspace_idx < state.workspaces.len() {
            state.workspaces[session.workspace_idx].name.clone()
        } else {
            format!("workspace:{}", session.workspace_idx)
        };

        entries.push(DashboardEntry {
            pane_id: session.pane_id,
            workspace_idx: session.workspace_idx,
            session_id: session.session_id.clone(),
            title: session.title.clone(),
            state: session.state.clone(),
            model: stats.model,
            context_used,
            context_max: stats.context_max,
            tokens_used: stats.input_tokens + stats.output_tokens,
            cost_estimate: stats.cost_estimate,
            cwd: session.cwd.clone(),
            workspace_name,
            subagents: session.subagents.clone(),
            tool_usage: stats.tool_usage,
            started_at: session.started_at,
        });
    }

    // Preserve selection if possible.
    let old_selected = state
        .claudes_dashboard
        .entries
        .get(state.claudes_dashboard.selected_idx)
        .map(|e| e.session_id.clone());
    state.claudes_dashboard.entries = entries;
    if let Some(old_id) = old_selected {
        if let Some(pos) = state
            .claudes_dashboard
            .entries
            .iter()
            .position(|e| e.session_id == old_id)
        {
            state.claudes_dashboard.selected_idx = pos;
        }
    }
    if state.claudes_dashboard.selected_idx >= state.claudes_dashboard.entries.len() {
        state.claudes_dashboard.selected_idx =
            state.claudes_dashboard.entries.len().saturating_sub(1);
    }
    state.claudes_dashboard.last_refresh = std::time::Instant::now();
}

/// Render the Claudes Dashboard overlay.
pub(crate) fn render_claudes_dashboard(
    state: &mut AppState,
    view: &wgpu::TextureView,
    phys_w: f32,
    phys_h: f32,
) {
    let cell_size = state.renderer.cell_size();
    let cell_w = cell_size.width;
    let cell_h = cell_size.height;
    let pc = &state.config.palette;

    // 1. Semi-transparent dark overlay covering entire window.
    let overlay_color = color_or(&pc.overlay_color, [0.0, 0.0, 0.0, 0.55]);
    state
        .renderer
        .submit_separator(view, 0, 0, phys_w as u32, phys_h as u32, overlay_color);

    // 2. Centered floating box — 75% width, 80% height.
    let box_w = (phys_w * 0.75).min(phys_w - 40.0).max(400.0);
    let box_h = (phys_h * 0.80).min(phys_h - 40.0).max(200.0);
    let box_x = (phys_w - box_w) / 2.0;
    let box_y = (phys_h - box_h) / 2.0;

    let box_bg = color_or(&pc.bg, [0.10, 0.10, 0.14, 0.95]);
    let border_color = color_or(&pc.border_color, [0.3, 0.3, 0.4, 1.0]);
    let corner_radius = pc.corner_radius;
    let border_width = pc.border_width;
    let shadow_radius = pc.shadow_radius;
    let shadow_opacity = pc.shadow_opacity;

    let dashboard_rect = RoundedRect {
        rect: [box_x, box_y, box_w, box_h],
        color: box_bg,
        border_color,
        params: [corner_radius, border_width, shadow_radius, shadow_opacity],
    };
    state.renderer.submit_rounded_rects(view, &[dashboard_rect]);

    // Footer area.
    let footer_h = cell_h * 1.5;
    let content_h = box_h - footer_h;

    // Header.
    let header_y = box_y + cell_h * 0.25;
    let entry_count = state.claudes_dashboard.entries.len();
    let header_text = format!("  Claudes ({})", entry_count);
    let header_fg = [0.95, 0.95, 0.95, 1.0];
    state.renderer.render_text(
        view,
        &header_text,
        box_x + cell_w * 0.5,
        header_y,
        header_fg,
        box_bg,
    );

    // Header separator.
    let header_sep_y = (header_y + cell_h + cell_h * 0.15) as u32;
    let sep_color = color_or(&pc.separator_color, [0.25, 0.25, 0.3, 1.0]);
    state.renderer.submit_separator(
        view,
        box_x as u32 + 1,
        header_sep_y,
        box_w as u32 - 2,
        1,
        sep_color,
    );

    // Split: left pane (40%) and right pane (60%).
    let left_w = (box_w * 0.40).max(100.0);
    let right_w = box_w - left_w;
    let left_x = box_x;
    let right_x = box_x + left_w;
    let list_start_y = header_sep_y as f32 + cell_h * 0.25;
    let list_area_h = content_h - (list_start_y - box_y);

    // Vertical separator between left and right panes.
    state.renderer.submit_separator(
        view,
        right_x as u32,
        header_sep_y,
        1,
        (box_y + content_h - header_sep_y as f32) as u32,
        sep_color,
    );

    // Right pane: "Detail" header.
    let detail_header = "  Detail";
    state.renderer.render_text(
        view,
        detail_header,
        right_x + cell_w * 0.5,
        header_y,
        header_fg,
        box_bg,
    );

    // Color definitions.
    let text_fg = [0.75, 0.75, 0.78, 1.0];
    let dim_fg = [0.5, 0.5, 0.55, 1.0];
    let selected_bg = color_or(&pc.selected_bg, [0.22, 0.22, 0.32, 1.0]);
    let green = [0.35, 0.82, 0.35, 1.0];
    let yellow = [0.90, 0.82, 0.20, 1.0];
    let red = [1.0, 0.42, 0.42, 1.0];
    let orange = [1.0, 0.60, 0.15, 1.0];
    let accent = [0.40, 0.70, 1.0, 1.0];

    // Pulse animation for Running indicator: oscillate alpha between 0.4 and 1.0.
    let pulse_t = state.app_start_time.elapsed().as_secs_f32();
    let pulse_alpha = 0.7 + 0.3 * (pulse_t * 2.5).sin();
    let green_pulse = [green[0], green[1], green[2], pulse_alpha];

    // --- Left pane: session list ---
    let entry_h = cell_h * 3.5; // 3 lines + padding per entry
    let max_visible = ((list_area_h) / entry_h) as usize;
    state.claudes_dashboard.ensure_visible(max_visible.max(1));
    let scroll_offset = state.claudes_dashboard.scroll_offset;
    let left_max_chars = ((left_w - 2.0 * cell_w) / cell_w) as usize;

    if state.claudes_dashboard.entries.is_empty() {
        let empty_msg = "No Claude Code sessions detected";
        state.renderer.render_text(
            view,
            empty_msg,
            left_x + cell_w,
            list_start_y + cell_h,
            dim_fg,
            box_bg,
        );
    }

    for (vi, entry) in state
        .claudes_dashboard
        .entries
        .iter()
        .enumerate()
        .skip(scroll_offset)
        .take(max_visible.max(1))
    {
        let row = vi - scroll_offset;
        let item_y = list_start_y + (row as f32) * entry_h;
        if item_y + entry_h > box_y + content_h {
            break;
        }

        let is_selected = vi == state.claudes_dashboard.selected_idx;
        let bg = if is_selected { selected_bg } else { box_bg };

        if is_selected {
            let sel_rect = RoundedRect {
                rect: [left_x + 2.0, item_y, left_w - 4.0, entry_h - cell_h * 0.25],
                color: selected_bg,
                border_color: [0.0; 4],
                params: [4.0, 0.0, 0.0, 0.0],
            };
            state.renderer.submit_rounded_rects(view, &[sel_rect]);
        }

        // Line 1: state icon + title
        let state_icon = match entry.state {
            SessionState::Running => "\u{25CF} ", // filled circle
            SessionState::Idle => "\u{25CB} ",    // open circle
            SessionState::Done => "\u{2714} ",    // checkmark
            SessionState::WaitingForPermission => "\u{23F3} ", // hourglass
        };
        let state_color = match entry.state {
            SessionState::Running => green_pulse,
            SessionState::Idle => yellow,
            SessionState::Done => dim_fg,
            SessionState::WaitingForPermission => orange,
        };
        let title_display: String = if is_selected {
            format!("\u{25B6} {}{}", state_icon, entry.title)
        } else {
            format!("  {}{}", state_icon, entry.title)
        };
        let title_trunc: String = title_display.chars().take(left_max_chars).collect();
        let title_fg = if is_selected { header_fg } else { text_fg };
        state.renderer.render_text(
            view,
            &title_trunc,
            left_x + cell_w * 0.5,
            item_y + cell_h * 0.15,
            title_fg,
            bg,
        );
        // Color the state icon separately.
        let icon_str: String = if is_selected {
            format!("\u{25B6} {}", state_icon)
        } else {
            format!("  {}", state_icon)
        };
        state.renderer.render_text(
            view,
            &icon_str,
            left_x + cell_w * 0.5,
            item_y + cell_h * 0.15,
            state_color,
            bg,
        );

        // Line 2: model | context bar N%
        let model_short = termojinal_claude::monitor::model_short_name(&entry.model);
        let pct = if entry.context_max > 0 {
            ((entry.context_used as f64 / entry.context_max as f64) * 100.0).min(100.0)
        } else {
            0.0
        };
        let bar_len = 8usize;
        let filled = ((pct / 100.0) * bar_len as f64) as usize;
        let bar: String = format!(
            "{}{}",
            "\u{2588}".repeat(filled.min(bar_len)),
            "\u{2591}".repeat(bar_len.saturating_sub(filled)),
        );
        let context_str = format!(
            "  {} \u{2502} {}/{}k {} {:.0}%",
            model_short,
            format_token_count(entry.context_used),
            entry.context_max / 1000,
            bar,
            pct,
        );
        let bar_color = if pct < 70.0 {
            green
        } else if pct < 90.0 {
            yellow
        } else {
            red
        };
        let context_trunc: String = context_str.chars().take(left_max_chars).collect();
        state.renderer.render_text(
            view,
            &context_trunc,
            left_x + cell_w * 0.5,
            item_y + cell_h * 1.15,
            bar_color,
            bg,
        );

        // Line 3: subagent count | cwd
        let agent_count = entry.subagents.len();
        let cwd_abbr = abbreviate_home(&entry.cwd);
        let line3 = format!("  \u{2299}{} agents \u{2502} {}", agent_count, cwd_abbr);
        let line3_trunc: String = line3.chars().take(left_max_chars).collect();
        state.renderer.render_text(
            view,
            &line3_trunc,
            left_x + cell_w * 0.5,
            item_y + cell_h * 2.15,
            dim_fg,
            bg,
        );
    }

    // --- Right pane: detail view ---
    if let Some(entry) = state
        .claudes_dashboard
        .entries
        .get(state.claudes_dashboard.selected_idx)
    {
        let detail_x = right_x + cell_w * 1.5;
        let detail_max_chars = ((right_w - 3.0 * cell_w) / cell_w) as usize;
        let mut dy = list_start_y;
        let line_h = cell_h * 1.2;

        // Task title.
        let title_trunc: String = entry.title.chars().take(detail_max_chars).collect();
        state
            .renderer
            .render_text(view, &title_trunc, detail_x, dy, header_fg, box_bg);
        dy += line_h * 1.2;

        // Model.
        let model_line = format!("Model:    {}", entry.model);
        state
            .renderer
            .render_text(view, &model_line, detail_x, dy, text_fg, box_bg);
        dy += line_h;

        // Context bar.
        let pct = if entry.context_max > 0 {
            ((entry.context_used as f64 / entry.context_max as f64) * 100.0).min(100.0)
        } else {
            0.0
        };
        let bar_len = 10usize;
        let filled = ((pct / 100.0) * bar_len as f64) as usize;
        let bar: String = format!(
            "{}{}",
            "\u{2588}".repeat(filled.min(bar_len)),
            "\u{2591}".repeat(bar_len.saturating_sub(filled)),
        );
        let ctx_line = format!(
            "Context:  {} {}/{}k",
            bar,
            format_token_count(entry.context_used),
            entry.context_max / 1000,
        );
        let bar_color = if pct < 70.0 {
            green
        } else if pct < 90.0 {
            yellow
        } else {
            red
        };
        state
            .renderer
            .render_text(view, &ctx_line, detail_x, dy, bar_color, box_bg);
        dy += line_h;

        // Tokens.
        let tokens_line = format!("Tokens:   ~{}k used", entry.tokens_used / 1000);
        state
            .renderer
            .render_text(view, &tokens_line, detail_x, dy, text_fg, box_bg);
        dy += line_h;

        // Cost.
        let cost_line = format!("Cost:     ~${:.2}", entry.cost_estimate);
        state
            .renderer
            .render_text(view, &cost_line, detail_x, dy, text_fg, box_bg);
        dy += line_h;

        // CWD.
        let cwd_abbr = abbreviate_home(&entry.cwd);
        let cwd_line = format!("CWD:      {}", cwd_abbr);
        let cwd_trunc: String = cwd_line.chars().take(detail_max_chars).collect();
        state
            .renderer
            .render_text(view, &cwd_trunc, detail_x, dy, text_fg, box_bg);
        dy += line_h;

        // Workspace / pane info.
        let pane_line = format!("Pane:     {}  pane#{}", entry.workspace_name, entry.pane_id);
        let pane_trunc: String = pane_line.chars().take(detail_max_chars).collect();
        state
            .renderer
            .render_text(view, &pane_trunc, detail_x, dy, text_fg, box_bg);
        dy += line_h;

        // Started time.
        if entry.started_at > 0 {
            let now_ms = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_millis() as u64)
                .unwrap_or(0);
            let elapsed_secs = now_ms.saturating_sub(entry.started_at) / 1000;
            let elapsed_str = if elapsed_secs < 60 {
                format!("{} sec ago", elapsed_secs)
            } else if elapsed_secs < 3600 {
                format!("{} min ago", elapsed_secs / 60)
            } else {
                format!(
                    "{} hr {} min ago",
                    elapsed_secs / 3600,
                    (elapsed_secs % 3600) / 60
                )
            };
            let started_line = format!("Started:  {}", elapsed_str);
            state
                .renderer
                .render_text(view, &started_line, detail_x, dy, text_fg, box_bg);
        }
        dy += line_h * 1.3;

        // Subagents section.
        if !entry.subagents.is_empty() {
            let sub_header = format!("Subagents ({}):", entry.subagents.len());
            state
                .renderer
                .render_text(view, &sub_header, detail_x, dy, accent, box_bg);
            dy += line_h;

            for (i, agent) in entry.subagents.iter().enumerate() {
                let is_last = i == entry.subagents.len() - 1;
                let connector = if is_last {
                    "\u{2514}\u{2500} "
                } else {
                    "\u{251C}\u{2500} "
                };
                let state_dot = match agent.state {
                    SessionState::Running => "\u{25CF} ",
                    _ => "\u{2714} ",
                };
                let agent_state_color = match agent.state {
                    SessionState::Running => green_pulse,
                    _ => dim_fg,
                };
                let agent_desc: String = agent.description.chars().take(30).collect();
                let agent_type_display = if agent.agent_type.is_empty() {
                    "Agent".to_string()
                } else {
                    agent.agent_type.clone()
                };
                let agent_line = format!(
                    "{}{}{:<12} \"{}\"",
                    connector, state_dot, agent_type_display, agent_desc
                );
                let agent_trunc: String = agent_line.chars().take(detail_max_chars).collect();
                state
                    .renderer
                    .render_text(view, &agent_trunc, detail_x, dy, text_fg, box_bg);
                // Color the state dot.
                let dot_offset = connector.chars().count() as f32 * cell_w;
                state.renderer.render_text(
                    view,
                    state_dot,
                    detail_x + dot_offset,
                    dy,
                    agent_state_color,
                    box_bg,
                );
                dy += line_h;
            }
        }
        dy += cell_h * 0.3;

        // Tool usage horizontal bar chart.
        if !entry.tool_usage.is_empty() && dy + line_h < box_y + content_h {
            state
                .renderer
                .render_text(view, "Tools Used:", detail_x, dy, accent, box_bg);
            dy += line_h;

            // Sort tools by usage count descending.
            let mut tools: Vec<(&String, &u32)> = entry.tool_usage.iter().collect();
            tools.sort_by(|a, b| b.1.cmp(a.1));
            let max_count = tools.first().map(|(_, &c)| c).unwrap_or(1).max(1);

            let tool_colors = [
                [0.45, 0.75, 1.0, 1.0],  // blue
                [0.35, 0.85, 0.55, 1.0], // green
                [0.95, 0.75, 0.30, 1.0], // orange
                [0.85, 0.50, 0.95, 1.0], // purple
                [0.95, 0.55, 0.55, 1.0], // red
                [0.50, 0.90, 0.85, 1.0], // cyan
            ];

            let bar_max_width = 14usize; // max bar chars
            for (ti, (tool_name, &count)) in tools.iter().enumerate() {
                if dy + line_h > box_y + content_h {
                    break;
                }
                let bar_filled =
                    ((count as f64 / max_count as f64) * bar_max_width as f64) as usize;
                let bar: String = "\u{2588}".repeat(bar_filled.max(1));
                let tool_display: String =
                    format!("{:<6}", tool_name.chars().take(6).collect::<String>());
                let tool_line = format!("{} {} {}", tool_display, bar, count);
                let tool_trunc: String = tool_line.chars().take(detail_max_chars).collect();
                let color_idx = ti % tool_colors.len();
                state.renderer.render_text(
                    view,
                    &tool_trunc,
                    detail_x,
                    dy,
                    tool_colors[color_idx],
                    box_bg,
                );
                dy += line_h;
            }
        }
        dy += cell_h * 0.3;

        // Mini preview: render a few lines from the target pane's terminal.
        if dy + cell_h * 5.0 < box_y + content_h {
            let preview_label = "\u{250C}\u{2500} Mini Preview \u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2510}";
            state
                .renderer
                .render_text(view, preview_label, detail_x, dy, dim_fg, box_bg);
            dy += line_h;

            // Try to access the pane's terminal grid.
            let preview_lines = 5usize;
            let preview_cols = detail_max_chars.min(30);
            let mut found = false;
            for ws in &state.workspaces {
                if found {
                    break;
                }
                for tab in &ws.tabs {
                    if let Some(pane) = tab.panes.get(&entry.pane_id) {
                        found = true;
                        let grid = pane.terminal.grid();
                        let rows = grid.rows();
                        // Show last N rows (near cursor).
                        let start_row = rows.saturating_sub(preview_lines);
                        for r in start_row..rows {
                            if dy + cell_h > box_y + content_h {
                                break;
                            }
                            // Render the border character in dim color.
                            state.renderer.render_text(
                                view,
                                "\u{2502} ",
                                detail_x,
                                dy,
                                dim_fg,
                                box_bg,
                            );
                            // Render each cell with its actual terminal color.
                            let col_start_x = detail_x + cell_w * 2.0;
                            let mut cx = col_start_x;
                            for c in 0..preview_cols.min(grid.cols()) {
                                let cell = grid.cell(c, r);
                                if cell.c == '\0' || cell.c == ' ' {
                                    cx += cell_w;
                                    continue;
                                }
                                let fg = color_to_rgba(cell.fg, true);
                                let ch_str = cell.c.to_string();
                                state.renderer.render_text(
                                    view, &ch_str, cx, dy, fg, box_bg,
                                );
                                cx += cell_w;
                            }
                            dy += cell_h;
                        }
                        break;
                    }
                }
            }
            if !found {
                state.renderer.render_text(
                    view,
                    "\u{2502} (pane not accessible)",
                    detail_x,
                    dy,
                    dim_fg,
                    box_bg,
                );
                dy += cell_h;
            }
            let preview_bottom = "\u{2514}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2518}";
            state
                .renderer
                .render_text(view, preview_bottom, detail_x, dy, dim_fg, box_bg);
        }
    } else {
        // No sessions -- show placeholder in right pane.
        let detail_x = right_x + cell_w * 2.0;
        let detail_y = list_start_y + cell_h * 2.0;
        state.renderer.render_text(
            view,
            "No session selected",
            detail_x,
            detail_y,
            dim_fg,
            box_bg,
        );
    }

    // --- Footer ---
    let footer_y = box_y + box_h - cell_h * 1.25;
    state.renderer.submit_separator(
        view,
        box_x as u32 + 1,
        (footer_y - cell_h * 0.25) as u32,
        box_w as u32 - 2,
        1,
        sep_color,
    );
    let running_count = state
        .claudes_dashboard
        .entries
        .iter()
        .filter(|e| matches!(e.state, SessionState::Running | SessionState::Idle))
        .count();
    let total_count = state.claudes_dashboard.entries.len();
    let footer_left = format!(
        " {} sessions, {} running  \u{2502}  \u{2191}\u{2193}/j,k/C-n,C-p: move  Enter: jump  Esc: close",
        total_count,
        running_count,
    );
    state
        .renderer
        .render_text(view, &footer_left, box_x + cell_w, footer_y, dim_fg, box_bg);
}

/// Format token count for display (e.g. 12345 -> "12.3k", 1234 -> "1.2k").
pub(crate) fn format_token_count(tokens: u64) -> String {
    if tokens >= 1_000_000 {
        format!("{:.1}M", tokens as f64 / 1_000_000.0)
    } else if tokens >= 1_000 {
        format!("{:.1}k", tokens as f64 / 1_000.0)
    } else {
        format!("{}", tokens)
    }
}
