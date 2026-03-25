use crate::*;
use config::color_or;
use termojinal_render::RoundedRect;
use winit::window::CursorIcon;

/// Determine the cursor icon for a position within the tab bar.
pub(crate) fn tab_bar_cursor(state: &AppState, mx: f32, _my: f32) -> CursorIcon {
    let sidebar_w = if state.sidebar_visible {
        state.sidebar_width
    } else {
        0.0
    };
    let local_cx = mx - sidebar_w;
    if local_cx < 0.0 {
        return CursorIcon::Default;
    }

    let cell_w = state.renderer.cell_size().width;
    let max_tab_w = state.config.tab_bar.max_width;
    let ws = &state.workspaces[state.active_workspace];
    let mut tab_x: f32 = 0.0;

    for tab in ws.tabs.iter() {
        let tab_w = compute_tab_width(
            &tab.display_title,
            cell_w,
            max_tab_w,
            state.config.tab_bar.min_tab_width,
        );
        if local_cx >= tab_x && local_cx < tab_x + tab_w {
            // Check if over close button (rightmost 1.5 cells)
            let close_start = tab_x + tab_w - 1.5 * cell_w;
            if local_cx >= close_start {
                return CursorIcon::Pointer;
            }
            return CursorIcon::Default;
        }
        tab_x += tab_w;
    }

    // Over new-tab button
    if local_cx >= tab_x && local_cx < tab_x + state.config.tab_bar.new_tab_button_width {
        return CursorIcon::Pointer;
    }

    CursorIcon::Default
}

/// Result of clicking in the tab bar.
pub(crate) enum TabBarClickResult {
    /// Clicked on a tab body — switch to it (and potentially start drag).
    Tab(usize),
    /// Clicked the close button on a tab.
    CloseTab(usize),
    /// Clicked the `+` new-tab button.
    NewTab,
    /// Click didn't hit anything meaningful.
    None,
}

/// Compute the pixel width of a single tab given its display title.
pub(crate) fn compute_tab_width(title: &str, cell_w: f32, max_width: f32, min_width: f32) -> f32 {
    // Text width + left padding (1 cell) + right padding (1 cell) + close button area (1.5 cells).
    let text_width = title.len() as f32 * cell_w + 3.5 * cell_w;
    text_width.clamp(min_width, max_width)
}

/// Handle a click in the tab bar area. Determine which tab was clicked and switch to it.
/// Returns a `TabBarClickResult` describing what was clicked.
pub(crate) fn handle_tab_bar_click(state: &mut AppState) -> TabBarClickResult {
    let cx = state.cursor_pos.0 as f32;
    let sidebar_w = if state.sidebar_visible {
        state.sidebar_width
    } else {
        0.0
    };
    let local_cx = cx - sidebar_w;
    if local_cx < 0.0 {
        return TabBarClickResult::None;
    }
    let cell_w = state.renderer.cell_size().width;
    let max_tab_w = state.config.tab_bar.max_width;

    let ws = active_ws(state);
    let mut tab_x: f32 = 0.0;
    for (i, tab) in ws.tabs.iter().enumerate() {
        let tab_w = compute_tab_width(
            &tab.display_title,
            cell_w,
            max_tab_w,
            state.config.tab_bar.min_tab_width,
        );
        if local_cx >= tab_x && local_cx < tab_x + tab_w {
            // Check if click is on the close button (rightmost 1.5 cells of the tab).
            let close_zone_start = tab_x + tab_w - 1.5 * cell_w;
            if local_cx >= close_zone_start {
                return TabBarClickResult::CloseTab(i);
            }
            // Return the tab index — the caller will decide whether to
            // switch immediately (on release) or start a drag.
            return TabBarClickResult::Tab(i);
        }
        tab_x += tab_w;
    }

    // Check if click is on the `+` new-tab button (after all tabs).
    if local_cx >= tab_x && local_cx < tab_x + state.config.tab_bar.new_tab_button_width {
        return TabBarClickResult::NewTab;
    }

    TabBarClickResult::None
}

/// Render the iTerm2-inspired tab bar.
pub(crate) fn render_tab_bar(state: &mut AppState, view: &wgpu::TextureView, phys_w: f32) {
    // --- Color palette from config ---
    let tc = &state.config.tab_bar;
    let tab_bar_bg = color_or(&tc.bg, [0.10, 0.10, 0.12, 1.0]);
    let active_tab_bg = color_or(&tc.active_tab_bg, [0.18, 0.18, 0.22, 1.0]);
    let active_fg = color_or(&tc.active_tab_fg, [0.95, 0.95, 0.97, 1.0]);
    let inactive_fg = color_or(&tc.inactive_tab_fg, [0.55, 0.55, 0.60, 1.0]);
    let accent_color = color_or(&tc.accent_color, [0.30, 0.55, 1.0, 1.0]);
    let separator_color = color_or(&tc.separator_color, [0.22, 0.22, 0.25, 1.0]);
    let close_fg = color_or(&tc.close_button_fg, [0.50, 0.50, 0.55, 1.0]);
    let new_tab_fg = color_or(&tc.new_button_fg, [0.50, 0.50, 0.55, 1.0]);

    let cell_w = state.renderer.cell_size().width;
    let cell_h = state.renderer.cell_size().height;
    let max_tab_w = state.config.tab_bar.max_width;
    let sidebar_w = if state.sidebar_visible {
        state.sidebar_width
    } else {
        0.0
    };
    let bar_x = sidebar_w as u32;
    let bar_w = (phys_w - sidebar_w).max(0.0) as u32;
    let bar_h_f = state.config.tab_bar.height;
    let bar_h = bar_h_f as u32;
    let accent_h = state.config.tab_bar.accent_height;
    let tab_pad_x = state.config.tab_bar.padding_x;
    let tab_pad_y = state.config.tab_bar.padding_y;

    // Draw tab bar background.
    state
        .renderer
        .submit_separator(view, bar_x, 0, bar_w, bar_h, tab_bar_bg);

    // Draw bottom border if enabled.
    if state.config.tab_bar.bottom_border {
        let border_color = color_or(
            &state.config.tab_bar.bottom_border_color,
            [0.16, 0.16, 0.20, 1.0],
        );
        state.renderer.submit_separator(
            view,
            bar_x,
            bar_h.saturating_sub(1),
            bar_w,
            1,
            border_color,
        );
    }

    // Draw each tab in the current workspace.
    let ws_idx = state.active_workspace;
    let ws = &state.workspaces[ws_idx];
    let active_tab_idx = ws.active_tab;
    let num_tabs = ws.tabs.len();
    let mut tab_x: f32 = sidebar_w;
    let text_y = (bar_h_f - cell_h) / 2.0;

    for (i, tab) in ws.tabs.iter().enumerate() {
        let tab_w = compute_tab_width(
            &tab.display_title,
            cell_w,
            max_tab_w,
            state.config.tab_bar.min_tab_width,
        );
        let is_active = i == active_tab_idx;

        // Draw tab background (active tab is brighter).
        if is_active {
            state.renderer.submit_separator(
                view,
                tab_x as u32,
                0,
                tab_w as u32,
                bar_h,
                active_tab_bg,
            );

            // Draw accent-colored bottom border (2px) for active tab.
            state.renderer.submit_separator(
                view,
                tab_x as u32,
                bar_h - accent_h,
                tab_w as u32,
                accent_h,
                accent_color,
            );
        }

        // Draw tab title text.
        let fg = if is_active { active_fg } else { inactive_fg };
        let bg = if is_active { active_tab_bg } else { tab_bar_bg };

        // Indicator dot for active tab (procedural circle to avoid CJK-width squishing).
        let dot_offset = if is_active {
            let dot_d = cell_h * 0.40;
            let dot_r = dot_d / 2.0;
            let dot_cx = tab_x + tab_pad_x + cell_w * 0.5;
            let dot_cy = text_y.max(0.0) + cell_h * 0.5;
            state.renderer.submit_rounded_rects(
                view,
                &[RoundedRect {
                    rect: [dot_cx - dot_r, dot_cy - dot_r, dot_d, dot_d],
                    color: accent_color,
                    border_color: [0.0; 4],
                    params: [dot_r, 0.0, 0.0, 0.0],
                }],
            );
            cell_w * 1.5 // dot + gap
        } else {
            0.0
        };

        // Title text.
        let text_x = tab_x + tab_pad_x + dot_offset;
        // Truncate title if it won't fit (leave room for close button).
        let avail_chars = ((tab_w - 3.5 * cell_w - dot_offset) / cell_w).max(1.0) as usize;
        let display: String = if tab.display_title.len() > avail_chars {
            let mut s: String = tab
                .display_title
                .chars()
                .take(avail_chars.saturating_sub(1))
                .collect();
            s.push('\u{2026}'); // ellipsis
            s
        } else {
            tab.display_title.clone()
        };
        state
            .renderer
            .render_text(view, &display, text_x, text_y.max(0.0), fg, bg);

        // Close button: show `\u{00d7}` (always for active tab, area is always clickable).
        if is_active || num_tabs > 1 {
            let close_x = tab_x + tab_w - tab_pad_x - cell_w;
            let close_char = "\u{00D7}"; // ×
            state.renderer.render_text(
                view,
                close_char,
                close_x,
                text_y.max(0.0),
                if is_active { close_fg } else { inactive_fg },
                bg,
            );
        }

        tab_x += tab_w;

        // Draw vertical separator between tabs (1px).
        if i < num_tabs - 1 {
            let sep_margin = tab_pad_y as u32;
            state.renderer.submit_separator(
                view,
                tab_x as u32,
                sep_margin,
                1,
                bar_h.saturating_sub(sep_margin * 2).max(1),
                separator_color,
            );
        }
    }

    // Draw `+` new-tab button after all tabs.
    let plus_x = tab_x + (state.config.tab_bar.new_tab_button_width - cell_w) / 2.0;
    state
        .renderer
        .render_text(view, "+", plus_x, text_y.max(0.0), new_tab_fg, tab_bar_bg);
}
