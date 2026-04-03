use crate::*;
use config::color_or;
use termojinal_layout::PaneId;

/// Render the search bar at the top of the content area (Feature 5).
pub(crate) fn render_search_bar(state: &mut AppState, view: &wgpu::TextureView, phys_w: f32) {
    let search = match &state.search {
        Some(s) => s,
        None => return,
    };
    let cell_size = state.renderer.cell_size();
    let cell_w = cell_size.width;
    let cell_h = cell_size.height;

    let sidebar_w = if state.sidebar_visible {
        state.sidebar_width
    } else {
        0.0
    };
    let tab_h = if tab_bar_visible(state) {
        state.config.tab_bar.height
    } else {
        0.0
    };

    let bar_x = sidebar_w as u32;
    let bar_y = tab_h as u32;
    let bar_w = (phys_w - sidebar_w).max(0.0) as u32;
    let bar_h = cell_h as u32 + 4;

    // Draw search bar background.
    let bar_bg = color_or(&state.config.search.bar_bg, [0.15, 0.15, 0.20, 0.95]);
    state
        .renderer
        .submit_separator(view, bar_x, bar_y, bar_w, bar_h, bar_bg);

    // Bottom border.
    let border_color = color_or(&state.config.search.border_color, [0.25, 0.25, 0.35, 1.0]);
    state.renderer.submit_separator(
        view,
        bar_x,
        bar_y + bar_h - 1,
        bar_w,
        1,
        border_color,
    );

    // Prompt + input text.
    let text_y = bar_y as f32 + 2.0;
    let text_x = sidebar_w + cell_w;
    let prompt = format!("Find: {}", search.query);
    let input_fg = color_or(&state.config.search.input_fg, [0.92, 0.92, 0.95, 1.0]);
    state
        .renderer
        .render_text(view, &prompt, text_x, text_y, input_fg, bar_bg);

    // Match count indicator.
    let count_text = if search.matches.is_empty() {
        "No matches".to_string()
    } else {
        format!("{}/{}", search.current + 1, search.matches.len())
    };
    let count_fg = if search.matches.is_empty() {
        [0.9, 0.4, 0.4, 1.0]
    } else {
        [0.6, 0.8, 0.6, 1.0]
    };
    let count_x = sidebar_w + bar_w as f32 - (count_text.len() as f32 + 1.0) * cell_w;
    state
        .renderer
        .render_text(view, &count_text, count_x, text_y, count_fg, bar_bg);
}

/// Render a minimal 1-line hint bar at the bottom of the focused pane.
///
/// This is a thin reminder that pending Allow Flow requests exist; the
/// full request details live in the sidebar.
pub(crate) fn render_allow_flow_pane_hint(
    state: &mut AppState,
    view: &wgpu::TextureView,
    pane_rects: &[(PaneId, termojinal_layout::Rect)],
    focused_id: PaneId,
) {
    // Find the focused pane's rect.
    let rect = match pane_rects.iter().find(|(id, _)| *id == focused_id) {
        Some((_, r)) => r,
        None => return,
    };

    let cell_size = state.renderer.cell_size();
    let cell_h = cell_size.height;
    let cell_w = cell_size.width;

    // Thin bar: 1 cell row + small vertical padding.
    let bar_pad = 2.0_f32;
    let bar_h = cell_h + bar_pad * 2.0;
    let bar_x = rect.x as u32;
    let bar_y = ((rect.y + rect.h) - bar_h).max(rect.y) as u32;
    let bar_w = rect.w as u32;

    // Colors from config (with sensible fallbacks).
    let ui = &state.config.allow_flow_ui;
    let bar_bg = color_or(&ui.hint_bar_bg, [0.85, 0.47, 0.02, 0.88]);
    let accent = color_or(&ui.hint_bar_accent, [0.96, 0.62, 0.04, 1.0]);
    let hint_fg = color_or(&ui.hint_bar_fg, [0.10, 0.10, 0.14, 1.0]);

    state
        .renderer
        .submit_separator(view, bar_x, bar_y, bar_w, bar_h as u32, bar_bg);

    // Top accent line.
    state
        .renderer
        .submit_separator(view, bar_x, bar_y, bar_w, 1, accent);

    // Text: lightning bolt + short message + key hints.
    let text_x = bar_x as f32 + cell_w;
    let text_y = bar_y as f32 + bar_pad;
    let max_chars = ((bar_w as f32 - 2.0 * cell_w) / cell_w).max(1.0) as usize;
    let msg = "\u{26A1} AI permission needed \u{2014} y/n one \u{00B7} Y/N all \u{00B7} A always";
    let display: String = msg.chars().take(max_chars).collect();
    state
        .renderer
        .render_text(view, &display, text_x, text_y, hint_fg, bar_bg);
}
