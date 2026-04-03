use crate::*;
use config::color_or;
use termojinal_render::RoundedRect;

/// Render the command palette as an overlay on top of the terminal.
pub(crate) fn render_command_palette(
    state: &mut AppState,
    view: &wgpu::TextureView,
    phys_w: f32,
    phys_h: f32,
) {
    // If a command execution is active, render its UI instead.
    if state.command_execution.is_some() {
        render_command_execution(state, view, phys_w, phys_h);
        return;
    }

    let cell_size = state.renderer.cell_size();
    let cell_w = cell_size.width;
    let cell_h = cell_size.height;

    let pc = &state.config.palette;
    // 1. Semi-transparent dark overlay covering the entire window.
    let overlay_color = color_or(&pc.overlay_color, [0.0, 0.0, 0.0, 0.5]);
    state
        .renderer
        .submit_separator(view, 0, 0, phys_w as u32, phys_h as u32, overlay_color);

    // 2. Centered floating box.
    let box_w = (phys_w * pc.width_ratio).min(phys_w - 40.0).max(200.0);
    let max_box_h = pc.max_height;
    let max_visible_items = pc.max_visible_items;
    let item_count = match state.command_palette.mode {
        PaletteMode::Command => state.command_palette.filtered.len(),
        PaletteMode::FileFinder => state.command_palette.file_finder.filtered.len(),
    };
    let visible_items = item_count.min(max_visible_items);
    let rows_needed = 1 + visible_items.max(1); // input row + item rows (min 1 for "No matches")
    let box_h = ((rows_needed as f32) * cell_h + cell_h).min(max_box_h); // extra padding
    let box_x = (phys_w - box_w) / 2.0;
    let box_y = (phys_h * 0.2).min(phys_h - box_h - 20.0).max(20.0);

    // Draw box background and border using SDF rounded rectangle.
    let box_bg = color_or(&pc.bg, [0.12, 0.12, 0.16, 0.95]);
    let default_border = color_or(&pc.border_color, [0.3, 0.3, 0.4, 1.0]);
    // Error flash: orange border for 400ms after invalid e/v action.
    let error_flash_active = state
        .command_palette
        .error_flash
        .map(|t| t.elapsed().as_millis() < 400)
        .unwrap_or(false);
    let border_color = if error_flash_active {
        [1.0, 0.58, 0.16, 1.0] // orange
    } else {
        // Clear expired flash.
        if state.command_palette.error_flash.is_some() {
            state.command_palette.error_flash = None;
        }
        default_border
    };
    let corner_radius = pc.corner_radius;
    let border_width = if error_flash_active {
        pc.border_width.max(2.0)
    } else {
        pc.border_width
    };
    let shadow_radius = pc.shadow_radius;
    let shadow_opacity = pc.shadow_opacity;

    let palette_rect = RoundedRect {
        rect: [box_x, box_y, box_w, box_h],
        color: box_bg,
        border_color,
        params: [corner_radius, border_width, shadow_radius, shadow_opacity],
    };
    state.renderer.submit_rounded_rects(view, &[palette_rect]);

    // 3. Input field at the top of the box.
    let input_y = box_y + cell_h * 0.25;
    let input_x = box_x + cell_w;
    let preedit = &state.command_palette.preedit;
    let prompt = match state.command_palette.mode {
        PaletteMode::Command => {
            if preedit.is_empty() {
                format!("> {}", state.command_palette.input)
            } else {
                format!("> {}[{}]", state.command_palette.input, preedit)
            }
        }
        PaletteMode::FileFinder => {
            // Show abbreviated CWD + input.
            let root = &state.command_palette.file_finder.search_root;
            let home = dirs::home_dir()
                .map(|p| p.to_string_lossy().to_string())
                .unwrap_or_default();
            let display_root = if !home.is_empty() && root.starts_with(&home) {
                format!("~{}/", &root[home.len()..])
            } else {
                format!("{}/", root)
            };
            if preedit.is_empty() {
                format!("{}{}", display_root, state.command_palette.input)
            } else {
                format!(
                    "{}{}[{}]",
                    display_root, state.command_palette.input, preedit
                )
            }
        }
    };
    let palette_input_fg = color_or(&pc.input_fg, [0.95, 0.95, 0.95, 1.0]);
    state
        .renderer
        .render_text(view, &prompt, input_x, input_y, palette_input_fg, box_bg);

    // Draw a separator line below the input.
    let sep_y = (input_y + cell_h + cell_h * 0.25) as u32;
    let palette_sep_color = color_or(&pc.separator_color, [0.25, 0.25, 0.3, 1.0]);
    state.renderer.submit_separator(
        view,
        box_x as u32 + 1,
        sep_y,
        box_w as u32 - 2,
        1,
        palette_sep_color,
    );

    // 4. Filtered item list.
    let list_start_y = sep_y as f32 + cell_h * 0.25;
    let cmd_fg = color_or(&pc.command_fg, [0.8, 0.8, 0.82, 1.0]);
    let selected_bg = color_or(&pc.selected_bg, [0.22, 0.22, 0.32, 1.0]);
    let desc_fg = color_or(&pc.description_fg, [0.5, 0.5, 0.55, 1.0]);
    let max_chars = ((box_w - 2.0 * cell_w) / cell_w) as usize;

    match state.command_palette.mode {
        PaletteMode::Command => {
            // --- Command list rendering (existing behavior) ---
            state.command_palette.ensure_visible(max_visible_items);
            let scroll_offset = state.command_palette.scroll_offset;

            for (vi, &cmd_idx) in state
                .command_palette
                .filtered
                .iter()
                .enumerate()
                .skip(scroll_offset)
                .take(max_visible_items)
            {
                let row = vi - scroll_offset;
                let item_y = list_start_y + (row as f32) * cell_h;
                if item_y + cell_h > box_y + box_h {
                    break;
                }

                let is_selected = vi == state.command_palette.selected;
                let bg = if is_selected { selected_bg } else { box_bg };

                if is_selected {
                    let sel_rect = RoundedRect {
                        rect: [box_x + 1.0, item_y, box_w - 2.0, cell_h],
                        color: selected_bg,
                        border_color: [0.0; 4],
                        params: [4.0, 0.0, 0.0, 0.0],
                    };
                    state.renderer.submit_rounded_rects(view, &[sel_rect]);
                }

                let cmd = &state.command_palette.commands[cmd_idx];
                let (badge, badge_fg) = match cmd.kind {
                    CommandKind::Builtin => ("", [0.0; 4]),
                    CommandKind::Plugin => ("[ext] ", [0.7, 0.55, 0.2, 1.0]),
                    CommandKind::PluginVerified => ("[ok] ", [0.4, 0.8, 0.4, 1.0]),
                };
                let badge_w = if badge.is_empty() {
                    0.0
                } else {
                    badge.chars().count() as f32 * cell_w
                };
                if !badge.is_empty() {
                    state
                        .renderer
                        .render_text(view, badge, input_x, item_y, badge_fg, bg);
                }
                let name_display: String = cmd
                    .name
                    .chars()
                    .take(max_chars.saturating_sub(badge.chars().count()))
                    .collect();
                let fg = if is_selected {
                    palette_input_fg
                } else {
                    cmd_fg
                };
                state
                    .renderer
                    .render_text(view, &name_display, input_x + badge_w, item_y, fg, bg);
                let desc_offset = badge_w + name_display.len() as f32 * cell_w + 2.0 * cell_w;
                if desc_offset < box_w - 2.0 * cell_w {
                    let remaining = max_chars.saturating_sub(name_display.len() + 2);
                    let desc_display: String = cmd.description.chars().take(remaining).collect();
                    state.renderer.render_text(
                        view,
                        &desc_display,
                        input_x + desc_offset,
                        item_y,
                        desc_fg,
                        bg,
                    );
                }
            }
            if state.command_palette.filtered.is_empty() {
                let empty_fg = [0.5, 0.5, 0.55, 1.0];
                state.renderer.render_text(
                    view,
                    "No matching commands",
                    input_x,
                    list_start_y,
                    empty_fg,
                    box_bg,
                );
            }
        }
        PaletteMode::FileFinder => {
            // --- File finder list rendering ---
            let ff = &mut state.command_palette.file_finder;
            ff.ensure_visible(max_visible_items);
            let scroll_offset = ff.scroll_offset;
            let dir_icon_fg = [0.55, 0.75, 0.95, 1.0]; // blue-ish for directories

            for (vi, &entry_idx) in ff
                .filtered
                .iter()
                .enumerate()
                .skip(scroll_offset)
                .take(max_visible_items)
            {
                let row = vi - scroll_offset;
                let item_y = list_start_y + (row as f32) * cell_h;
                if item_y + cell_h > box_y + box_h {
                    break;
                }

                let is_selected = vi == ff.selected;
                let bg = if is_selected { selected_bg } else { box_bg };

                if is_selected {
                    let sel_rect = RoundedRect {
                        rect: [box_x + 1.0, item_y, box_w - 2.0, cell_h],
                        color: selected_bg,
                        border_color: [0.0; 4],
                        params: [4.0, 0.0, 0.0, 0.0],
                    };
                    state.renderer.submit_rounded_rects(view, &[sel_rect]);
                }

                let entry = &ff.entries[entry_idx];
                let (icon, icon_fg) = if entry.is_dir {
                    ("\u{F07B} ", dir_icon_fg) //  folder (Nerd Font)
                } else {
                    (file_icon(&entry.name), file_extension_color(&entry.name))
                };
                state
                    .renderer
                    .render_text(view, icon, input_x, item_y, icon_fg, bg);

                let name_display: String = entry
                    .name
                    .chars()
                    .take(max_chars.saturating_sub(3))
                    .collect();
                let name_suffix = if entry.is_dir { "/" } else { "" };
                let display = format!("{}{}", name_display, name_suffix);
                let fg = if is_selected {
                    palette_input_fg
                } else {
                    cmd_fg
                };
                state
                    .renderer
                    .render_text(view, &display, input_x + cell_w * 3.0, item_y, fg, bg);
            }
            if ff.filtered.is_empty() {
                let empty_fg = [0.5, 0.5, 0.55, 1.0];
                state.renderer.render_text(
                    view,
                    "No matching files",
                    input_x,
                    list_start_y,
                    empty_fg,
                    box_bg,
                );
            }
        }
    }
}

/// Render the Quick Launch overlay (fuzzy search for tabs/panes/workspaces).
pub(crate) fn render_quick_launch(state: &mut AppState, view: &wgpu::TextureView, phys_w: f32, phys_h: f32) {
    let cell_size = state.renderer.cell_size();
    let cell_w = cell_size.width;
    let cell_h = cell_size.height;

    let pc = &state.config.palette;

    // 1. Semi-transparent overlay.
    let overlay_color = color_or(&pc.overlay_color, [0.0, 0.0, 0.0, 0.5]);
    state
        .renderer
        .submit_separator(view, 0, 0, phys_w as u32, phys_h as u32, overlay_color);

    // 2. Centered floating box.
    let max_visible_items: usize = 12;
    let box_w = (phys_w * pc.width_ratio).min(phys_w - 40.0).max(200.0);
    let item_count = state.quick_launch.filtered.len();
    let visible_items = item_count.min(max_visible_items);
    let rows_needed = 1 + visible_items.max(1);
    let box_h = ((rows_needed as f32) * cell_h * 1.5 + cell_h).min(pc.max_height);
    let box_x = (phys_w - box_w) / 2.0;
    let box_y = (phys_h * 0.18).min(phys_h - box_h - 20.0).max(20.0);

    let box_bg = color_or(&pc.bg, [0.12, 0.12, 0.16, 0.95]);
    let border_color = [0.35, 0.55, 0.90, 1.0]; // blue accent to distinguish from command palette
    let corner_radius = pc.corner_radius;
    let border_width = pc.border_width;
    let shadow_radius = pc.shadow_radius;
    let shadow_opacity = pc.shadow_opacity;

    let rect = RoundedRect {
        rect: [box_x, box_y, box_w, box_h],
        color: box_bg,
        border_color,
        params: [corner_radius, border_width, shadow_radius, shadow_opacity],
    };
    state.renderer.submit_rounded_rects(view, &[rect]);

    // 3. Input field.
    let input_y = box_y + cell_h * 0.25;
    let input_x = box_x + cell_w;
    let max_chars = ((box_w - cell_w * 2.0) / cell_w).max(1.0) as usize;
    let palette_input_fg = color_or(&pc.input_fg, [0.92, 0.92, 0.95, 1.0]);

    let prompt = format!("\u{F002} {}", state.quick_launch.input); // magnifying glass icon
    let prompt_display: String = prompt.chars().take(max_chars).collect();
    state.renderer.render_text(
        view,
        &prompt_display,
        input_x,
        input_y,
        palette_input_fg,
        box_bg,
    );

    // Separator line.
    let sep_y = input_y + cell_h + 2.0;
    let sep_color = color_or(&pc.separator_color, [0.2, 0.2, 0.3, 1.0]);
    state.renderer.submit_separator(
        view,
        (box_x + cell_w * 0.5) as u32,
        sep_y as u32,
        (box_w - cell_w) as u32,
        1,
        sep_color,
    );

    // 4. Result list.
    let list_start_y = sep_y + 4.0;
    let selected_bg = color_or(&pc.selected_bg, [0.22, 0.22, 0.32, 1.0]);
    let cmd_fg = color_or(&pc.command_fg, [0.8, 0.8, 0.84, 1.0]);
    let desc_fg = color_or(&pc.description_fg, [0.5, 0.5, 0.55, 1.0]);

    let ql = &mut state.quick_launch;
    ql.ensure_visible(max_visible_items);
    let scroll_offset = ql.scroll_offset;
    let row_h = cell_h * 1.5; // extra space for detail line

    for (vi, &entry_idx) in ql
        .filtered
        .iter()
        .enumerate()
        .skip(scroll_offset)
        .take(max_visible_items)
    {
        let row = vi - scroll_offset;
        let item_y = list_start_y + (row as f32) * row_h;
        if item_y + row_h > box_y + box_h {
            break;
        }

        let is_selected = vi == ql.selected;
        let bg = if is_selected { selected_bg } else { box_bg };

        if is_selected {
            let sel_rect = RoundedRect {
                rect: [box_x + 1.0, item_y, box_w - 2.0, row_h],
                color: selected_bg,
                border_color: [0.0; 4],
                params: [4.0, 0.0, 0.0, 0.0],
            };
            state.renderer.submit_rounded_rects(view, &[sel_rect]);
        }

        let entry = &ql.entries[entry_idx];
        // Kind icon.
        let (kind_icon, kind_fg) = match entry.kind {
            QuickLaunchKind::Workspace => ("\u{F0219} ", [0.55, 0.75, 0.95, 1.0]), // workspace icon
            QuickLaunchKind::Tab => ("\u{F03E2} ", [0.75, 0.70, 0.95, 1.0]),       // tab icon
            QuickLaunchKind::Pane => ("\u{F0668} ", [0.65, 0.85, 0.70, 1.0]),      // pane icon
        };
        state
            .renderer
            .render_text(view, kind_icon, input_x, item_y, kind_fg, bg);

        let label_display: String = entry
            .label
            .chars()
            .take(max_chars.saturating_sub(3))
            .collect();
        let fg = if is_selected {
            palette_input_fg
        } else {
            cmd_fg
        };
        state
            .renderer
            .render_text(view, &label_display, input_x + cell_w * 3.0, item_y, fg, bg);

        // Detail line (smaller, dimmed).
        let detail_display: String = entry
            .detail
            .chars()
            .take(max_chars.saturating_sub(4))
            .collect();
        state.renderer.render_text(
            view,
            &detail_display,
            input_x + cell_w * 3.0,
            item_y + cell_h * 0.85,
            desc_fg,
            bg,
        );
    }

    if ql.filtered.is_empty() {
        let empty_fg = [0.5, 0.5, 0.55, 1.0];
        state.renderer.render_text(
            view,
            "No matching items",
            input_x,
            list_start_y,
            empty_fg,
            box_bg,
        );
    }

    // Hint at bottom.
    let hint = "\u{21B5} Jump  \u{2191}\u{2193} Navigate  esc Close";
    let hint_y = box_y + box_h - cell_h * 0.8;
    let hint_fg = [0.4, 0.4, 0.5, 0.7];
    state
        .renderer
        .render_text(view, hint, input_x, hint_y, hint_fg, box_bg);
}

/// Render the command execution UI as an overlay (replaces normal palette content).
pub(crate) fn render_command_execution(
    state: &mut AppState,
    view: &wgpu::TextureView,
    phys_w: f32,
    phys_h: f32,
) {
    let cell_size = state.renderer.cell_size();
    let cell_w = cell_size.width;
    let cell_h = cell_size.height;

    let pc = &state.config.palette;

    // 1. Semi-transparent dark overlay covering the entire window.
    let overlay_color = color_or(&pc.overlay_color, [0.0, 0.0, 0.0, 0.5]);
    state
        .renderer
        .submit_separator(view, 0, 0, phys_w as u32, phys_h as u32, overlay_color);

    // Borrow the execution state to compute box dimensions.
    let Some(exec) = state.command_execution.as_ref() else {
        return;
    };
    let max_visible = pc.max_visible_items;

    // Determine how many rows the content needs.
    let content_rows = match &exec.ui_state {
        CommandUIState::Loading => 2,
        CommandUIState::Fuzzy { .. } | CommandUIState::Multi { .. } => {
            let visible = exec.filtered_items.len().min(max_visible);
            2 + visible // prompt/input + separator + items
        }
        CommandUIState::Confirm { .. } => 3,
        CommandUIState::Text { .. } => 3,
        CommandUIState::Info => 2,
        CommandUIState::Done(_) => 3,
        CommandUIState::Error(_) => 3,
    };

    // 2. Centered floating box.
    let box_w = (phys_w * pc.width_ratio).min(phys_w - 40.0).max(200.0);
    let max_box_h = pc.max_height;
    let box_h = ((content_rows as f32) * cell_h + cell_h).min(max_box_h);
    let box_x = (phys_w - box_w) / 2.0;
    let box_y = (phys_h * 0.2).min(phys_h - box_h - 20.0).max(20.0);

    // Draw box background and border using SDF rounded rectangle.
    let box_bg = color_or(&pc.bg, [0.12, 0.12, 0.16, 0.95]);
    let border_color = color_or(&pc.border_color, [0.3, 0.3, 0.4, 1.0]);
    let corner_radius = pc.corner_radius;
    let border_width = pc.border_width;
    let shadow_radius = pc.shadow_radius;
    let shadow_opacity = pc.shadow_opacity;

    let exec_rect = RoundedRect {
        rect: [box_x, box_y, box_w, box_h],
        color: box_bg,
        border_color,
        params: [corner_radius, border_width, shadow_radius, shadow_opacity],
    };
    state.renderer.submit_rounded_rects(view, &[exec_rect]);

    let input_x = box_x + cell_w;
    let input_y = box_y + cell_h * 0.25;
    let palette_input_fg = color_or(&pc.input_fg, [0.95, 0.95, 0.95, 1.0]);
    let cmd_fg = color_or(&pc.command_fg, [0.8, 0.8, 0.82, 1.0]);
    let selected_bg = color_or(&pc.selected_bg, [0.22, 0.22, 0.32, 1.0]);
    let desc_fg = color_or(&pc.description_fg, [0.5, 0.5, 0.55, 1.0]);
    let palette_sep_color = color_or(&pc.separator_color, [0.25, 0.25, 0.3, 1.0]);
    let max_chars = ((box_w - 2.0 * cell_w) / cell_w) as usize;

    // Re-borrow exec (the earlier borrow was dropped before renderer calls).
    let Some(exec) = state.command_execution.as_ref() else {
        return;
    };

    match &exec.ui_state {
        CommandUIState::Loading => {
            let msg = format!("Running {}...", exec.command_name);
            let display: String = msg.chars().take(max_chars).collect();
            state
                .renderer
                .render_text(view, &display, input_x, input_y, palette_input_fg, box_bg);
        }

        CommandUIState::Fuzzy { prompt } | CommandUIState::Multi { prompt } => {
            let is_multi = matches!(exec.ui_state, CommandUIState::Multi { .. });
            let prompt_str = prompt.clone();
            let input_str = exec.input.clone();
            let filtered: Vec<usize> = exec.filtered_items.clone();
            let selected_idx = exec.selected;
            let selected_set_snapshot: std::collections::HashSet<usize> = exec.selected_set.clone();
            let items_snapshot: Vec<_> = exec
                .items
                .iter()
                .map(|item| {
                    (
                        item.label.clone(),
                        item.value.clone(),
                        item.description.clone(),
                    )
                })
                .collect();

            // Prompt and input at the top.
            let prompt_display = format!("{}: {}", prompt_str, input_str);
            let display: String = prompt_display.chars().take(max_chars).collect();
            state
                .renderer
                .render_text(view, &display, input_x, input_y, palette_input_fg, box_bg);

            // Separator below prompt.
            let sep_y = (input_y + cell_h + cell_h * 0.25) as u32;
            state.renderer.submit_separator(
                view,
                box_x as u32 + 1,
                sep_y,
                box_w as u32 - 2,
                1,
                palette_sep_color,
            );

            // Item list.
            let list_start_y = sep_y as f32 + cell_h * 0.25;
            for (i, &item_idx) in filtered.iter().enumerate().take(max_visible) {
                let item_y = list_start_y + (i as f32) * cell_h;
                if item_y + cell_h > box_y + box_h {
                    break;
                }

                let is_selected = i == selected_idx;
                let bg = if is_selected { selected_bg } else { box_bg };

                if is_selected {
                    let sel_rect = RoundedRect {
                        rect: [box_x + 1.0, item_y, box_w - 2.0, cell_h],
                        color: selected_bg,
                        border_color: [0.0; 4],
                        params: [4.0, 0.0, 0.0, 0.0],
                    };
                    state.renderer.submit_rounded_rects(view, &[sel_rect]);
                }

                let (ref label_opt, ref value, ref desc_opt) = items_snapshot[item_idx];
                let label = label_opt.as_deref().unwrap_or(value.as_str());

                // Multi-select: show check mark for toggled items.
                let prefix = if is_multi {
                    if selected_set_snapshot.contains(&item_idx) {
                        "[x] "
                    } else {
                        "[ ] "
                    }
                } else {
                    ""
                };

                let name_display: String = format!("{}{}", prefix, label)
                    .chars()
                    .take(max_chars)
                    .collect();
                let fg = if is_selected {
                    palette_input_fg
                } else {
                    cmd_fg
                };
                state
                    .renderer
                    .render_text(view, &name_display, input_x, item_y, fg, bg);

                // Description after the name.
                if let Some(ref desc) = desc_opt {
                    let desc_offset = name_display.len() as f32 * cell_w + 2.0 * cell_w;
                    if desc_offset < box_w - 2.0 * cell_w {
                        let remaining = max_chars.saturating_sub(name_display.len() + 2);
                        let desc_display: String = desc.chars().take(remaining).collect();
                        state.renderer.render_text(
                            view,
                            &desc_display,
                            input_x + desc_offset,
                            item_y,
                            desc_fg,
                            bg,
                        );
                    }
                }
            }

            // Show "No matches" if filtered list is empty.
            if filtered.is_empty() {
                let no_match = "No matching items";
                let empty_fg = [0.5, 0.5, 0.55, 1.0];
                let list_start_y = (input_y + cell_h + cell_h * 0.25) as f32 + cell_h * 0.25;
                state
                    .renderer
                    .render_text(view, no_match, input_x, list_start_y, empty_fg, box_bg);
            }
        }

        CommandUIState::Confirm { message, default } => {
            let message_str = message.clone();
            let default_val = *default;
            let display: String = message_str.chars().take(max_chars).collect();
            state
                .renderer
                .render_text(view, &display, input_x, input_y, palette_input_fg, box_bg);

            let sep_y = (input_y + cell_h + cell_h * 0.25) as u32;
            state.renderer.submit_separator(
                view,
                box_x as u32 + 1,
                sep_y,
                box_w as u32 - 2,
                1,
                palette_sep_color,
            );

            let hint = if default_val {
                "Press Enter for Yes, or N for No"
            } else {
                "Press Y for Yes, or Enter for No"
            };
            let hint_y = sep_y as f32 + cell_h * 0.25;
            state
                .renderer
                .render_text(view, hint, input_x, hint_y, desc_fg, box_bg);
        }

        CommandUIState::Text { label, placeholder } => {
            let label_str = label.clone();
            let placeholder_str = placeholder.clone();
            let input_str = exec.input.clone();
            let label_display: String = format!("{}:", label_str).chars().take(max_chars).collect();
            state.renderer.render_text(
                view,
                &label_display,
                input_x,
                input_y,
                palette_input_fg,
                box_bg,
            );

            let sep_y = (input_y + cell_h + cell_h * 0.25) as u32;
            state.renderer.submit_separator(
                view,
                box_x as u32 + 1,
                sep_y,
                box_w as u32 - 2,
                1,
                palette_sep_color,
            );

            let text_y = sep_y as f32 + cell_h * 0.25;
            if input_str.is_empty() && !placeholder_str.is_empty() {
                let ph_display: String = placeholder_str.chars().take(max_chars).collect();
                state
                    .renderer
                    .render_text(view, &ph_display, input_x, text_y, desc_fg, box_bg);
            } else {
                let input_display: String = input_str.chars().take(max_chars).collect();
                state.renderer.render_text(
                    view,
                    &input_display,
                    input_x,
                    text_y,
                    palette_input_fg,
                    box_bg,
                );
            }
        }

        CommandUIState::Info => {
            let info_msg = exec.info_message.clone();
            let spinner_chars = ["|", "/", "-", "\\"];
            let tick = (std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_millis() / 200)
                .unwrap_or(0)
                % 4) as usize;
            let spinner = spinner_chars[tick];
            let msg = format!("{} {}", spinner, info_msg);
            let display: String = msg.chars().take(max_chars).collect();
            state
                .renderer
                .render_text(view, &display, input_x, input_y, palette_input_fg, box_bg);
        }

        CommandUIState::Done(notify) => {
            let msg = if let Some(ref text) = notify {
                format!("Done: {}", text)
            } else {
                let name = exec.command_name.clone();
                format!("Command '{}' completed", name)
            };
            let display: String = msg.chars().take(max_chars).collect();
            let done_fg = [0.55, 0.82, 0.33, 1.0]; // green
            state
                .renderer
                .render_text(view, &display, input_x, input_y, done_fg, box_bg);

            let hint = "Press any key to dismiss";
            let hint_y = input_y + cell_h;
            state
                .renderer
                .render_text(view, hint, input_x, hint_y, desc_fg, box_bg);
        }

        CommandUIState::Error(message) => {
            let msg = format!("Error: {}", message);
            let display: String = msg.chars().take(max_chars).collect();
            let error_fg = [1.0, 0.42, 0.42, 1.0]; // red
            state
                .renderer
                .render_text(view, &display, input_x, input_y, error_fg, box_bg);

            let hint = "Press Esc to dismiss";
            let hint_y = input_y + cell_h;
            state
                .renderer
                .render_text(view, hint, input_x, hint_y, desc_fg, box_bg);
        }
    }
}

/// Load the "About Termojinal" text including version, copyright, and third-party licenses.
pub(crate) fn load_about_text() -> String {
    let version = env!("CARGO_PKG_VERSION");
    let mut text = format!("Termojinal v{version}\n");
    text.push_str("GPU-accelerated terminal emulator\n");
    text.push_str("\n");
    text.push_str("Copyright (c) 2025-2026 Tomoo Kikuchi\n");
    text.push_str("All rights reserved.\n");
    text.push_str("\n");
    text.push_str("Licensed under the MIT License.\n");
    text.push_str("https://opensource.org/licenses/MIT\n");
    text.push_str("\n");

    // Try to load THIRD_PARTY_LICENSES.md from the executable's directory
    // or from known locations.
    let exe_dir = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|d| d.to_path_buf()));

    let search_paths = [
        exe_dir
            .as_ref()
            .map(|d| d.join("../Resources/THIRD_PARTY_LICENSES.md")),
        exe_dir
            .as_ref()
            .map(|d| d.join("../../THIRD_PARTY_LICENSES.md")),
        Some(std::path::PathBuf::from("THIRD_PARTY_LICENSES.md")),
    ];

    for path in search_paths.iter().flatten() {
        if let Ok(content) = std::fs::read_to_string(path) {
            text.push_str(&content);
            break;
        }
    }

    text
}

/// Render the command timeline overlay (Cmd+Shift+T).
///
/// Shows a searchable list of all commands executed in the focused pane,
/// with timestamps, exit codes, and durations.
pub(crate) fn render_command_timeline(
    state: &mut AppState,
    view: &wgpu::TextureView,
    phys_w: f32,
    phys_h: f32,
) {
    let cell_size = state.renderer.cell_size();
    let cell_w = cell_size.width;
    let cell_h = cell_size.height;

    // W3: Extract only the lightweight data needed for rendering (no full clone).
    #[allow(dead_code)]
    struct TimelineEntry {
        command_text: String,
        timestamp: chrono::DateTime<chrono::Utc>,
        exit_code: Option<i32>,
        duration_ms: Option<u64>,
        id: u64,
    }
    let focused_id = active_tab(state).layout.focused();
    let filter = state.timeline_input.to_lowercase();
    let entries: Vec<TimelineEntry> = active_tab(state)
        .panes
        .get(&focused_id)
        .map(|p| {
            p.terminal
                .command_history()
                .iter()
                .enumerate()
                .rev()
                .filter(|(_, cmd)| {
                    filter.is_empty() || cmd.command_text.to_lowercase().contains(&filter)
                })
                .map(|(_, cmd)| TimelineEntry {
                    command_text: cmd.command_text.clone(),
                    timestamp: cmd.timestamp,
                    exit_code: cmd.exit_code,
                    duration_ms: cmd.duration_ms,
                    id: cmd.id,
                })
                .collect()
        })
        .unwrap_or_default();

    // 1. Semi-transparent overlay.
    let overlay_color = [0.0, 0.0, 0.0, 0.5];
    state
        .renderer
        .submit_separator(view, 0, 0, phys_w as u32, phys_h as u32, overlay_color);

    // 2. Centered floating box.
    let box_w = (phys_w * 0.6).min(phys_w - 40.0).max(300.0);
    let max_visible = 15usize;
    let visible_items = entries.len().min(max_visible);
    let rows_needed = 1 + visible_items.max(1);
    let box_h = ((rows_needed as f32) * cell_h + cell_h * 1.5).min(phys_h - 40.0);
    let box_x = (phys_w - box_w) / 2.0;
    let box_y = (phys_h * 0.15).min(phys_h - box_h - 20.0).max(20.0);

    let box_bg = [0.10, 0.10, 0.14, 0.95];
    let border_color = [0.3, 0.3, 0.4, 1.0];

    let timeline_rect = RoundedRect {
        rect: [box_x, box_y, box_w, box_h],
        color: box_bg,
        border_color,
        params: [8.0, 1.0, 12.0, 0.3],
    };
    state.renderer.submit_rounded_rects(view, &[timeline_rect]);

    // 3. Input field.
    let input_y = box_y + cell_h * 0.25;
    let input_x = box_x + cell_w;
    let prompt = format!("> {}", state.timeline_input);
    let input_fg = [0.95, 0.95, 0.95, 1.0];
    state
        .renderer
        .render_text(view, &prompt, input_x, input_y, input_fg, box_bg);

    // Separator.
    let sep_y = (input_y + cell_h + cell_h * 0.25) as u32;
    let sep_color = [0.25, 0.25, 0.3, 1.0];
    state.renderer.submit_separator(
        view,
        box_x as u32 + 1,
        sep_y,
        box_w as u32 - 2,
        1,
        sep_color,
    );

    // 4. Command list.
    let list_start_y = sep_y as f32 + cell_h * 0.25;
    let cmd_fg = [0.8, 0.8, 0.82, 1.0];
    let selected_bg = [0.22, 0.22, 0.32, 1.0];
    let time_fg = [0.5, 0.5, 0.55, 1.0];
    let success_fg = [0.65, 0.89, 0.63, 1.0]; // green
    let fail_fg = [0.95, 0.55, 0.66, 1.0]; // red

    // Clamp selected.
    if !entries.is_empty() {
        state.timeline_selected = state.timeline_selected.min(entries.len() - 1);
    }
    // Ensure visible.
    if state.timeline_selected < state.timeline_scroll_offset {
        state.timeline_scroll_offset = state.timeline_selected;
    } else if state.timeline_selected >= state.timeline_scroll_offset + max_visible {
        state.timeline_scroll_offset = state.timeline_selected + 1 - max_visible;
    }

    let max_chars = ((box_w - 2.0 * cell_w) / cell_w) as usize;

    if entries.is_empty() {
        let msg = if filter.is_empty() {
            "No commands recorded (OSC 133 required)"
        } else {
            "No matching commands"
        };
        let msg_y = list_start_y + cell_h * 0.5;
        state
            .renderer
            .render_text(view, msg, input_x, msg_y, time_fg, box_bg);
    } else {
        for (vi, entry) in entries
            .iter()
            .enumerate()
            .skip(state.timeline_scroll_offset)
            .take(max_visible)
        {
            let row = vi - state.timeline_scroll_offset;
            let item_y = list_start_y + (row as f32) * cell_h;
            if item_y + cell_h > box_y + box_h {
                break;
            }

            let is_selected = vi == state.timeline_selected;
            let bg = if is_selected { selected_bg } else { box_bg };

            if is_selected {
                let sel_rect = RoundedRect {
                    rect: [box_x + 1.0, item_y, box_w - 2.0, cell_h],
                    color: selected_bg,
                    border_color: [0.0; 4],
                    params: [4.0, 0.0, 0.0, 0.0],
                };
                state.renderer.submit_rounded_rects(view, &[sel_rect]);
            }

            let cmd = entry;

            // Format: "HH:MM:SS  command_text  [exit_code] [duration]"
            let time_str = cmd.timestamp.format("%H:%M:%S").to_string();
            let exit_indicator = match cmd.exit_code {
                Some(0) => "OK".to_string(),
                Some(code) => format!("E{code}"),
                None => "..".to_string(),
            };
            let duration_str = cmd
                .duration_ms
                .map(|ms| {
                    if ms < 1000 {
                        format!("{ms}ms")
                    } else if ms < 60_000 {
                        format!("{:.1}s", ms as f64 / 1000.0)
                    } else {
                        format!("{:.0}m", ms as f64 / 60_000.0)
                    }
                })
                .unwrap_or_default();

            // Time
            let time_x = box_x + cell_w * 0.5;
            state
                .renderer
                .render_text(view, &time_str, time_x, item_y, time_fg, bg);

            // Command text (truncated)
            let cmd_x = time_x + cell_w * 10.0;
            let max_cmd_chars = max_chars.saturating_sub(18);
            let cmd_display: String = cmd.command_text.chars().take(max_cmd_chars).collect();
            state
                .renderer
                .render_text(view, &cmd_display, cmd_x, item_y, cmd_fg, bg);

            // Exit code indicator (right-aligned)
            let exit_fg = match cmd.exit_code {
                Some(0) => success_fg,
                Some(_) => fail_fg,
                None => time_fg,
            };
            let exit_x = box_x + box_w - cell_w * 8.0;
            state
                .renderer
                .render_text(view, &exit_indicator, exit_x, item_y, exit_fg, bg);

            // Duration (right-aligned)
            if !duration_str.is_empty() {
                let dur_x = box_x + box_w - cell_w * 4.0;
                state
                    .renderer
                    .render_text(view, &duration_str, dur_x, item_y, time_fg, bg);
            }
        }
    }

    // 5. Footer with keyboard shortcuts.
    let footer_y = box_y + box_h - cell_h * 0.75;
    let footer_text = "Up/Down navigate  Enter jump  Esc close  Cmd+R rerun";
    state
        .renderer
        .render_text(view, footer_text, input_x, footer_y, time_fg, box_bg);
}

/// Render a close confirmation dialog overlay.
pub(crate) fn render_close_confirm_dialog(
    state: &mut AppState,
    view: &wgpu::TextureView,
    phys_w: f32,
    phys_h: f32,
    proc_name: &str,
) {
    let cell_w = state.renderer.cell_size().width;
    let cell_h = state.renderer.cell_size().height;

    // Dark overlay behind the dialog.
    let overlay_color = [0.0, 0.0, 0.0, 0.55];
    state
        .renderer
        .submit_separator(view, 0, 0, phys_w as u32, phys_h as u32, overlay_color);

    // Dialog box dimensions.
    let dialog_w = (cell_w * 38.0).min(phys_w * 0.8);
    let dialog_h = cell_h * 5.0;
    let dialog_x = (phys_w - dialog_w) / 2.0;
    let dialog_y = (phys_h - dialog_h) / 2.0;

    // Dialog background.
    let bg = [0.12, 0.12, 0.16, 0.97];
    let border_color = [0.35, 0.35, 0.50, 0.9];
    state.renderer.submit_rounded_rects(
        view,
        &[RoundedRect {
            rect: [dialog_x, dialog_y, dialog_w, dialog_h],
            color: bg,
            border_color,
            params: [8.0, 1.0, 0.0, 0.0],
        }],
    );

    let text_x = dialog_x + cell_w * 1.5;
    let text_fg = [0.92, 0.92, 0.96, 1.0];
    let hint_fg = [0.60, 0.60, 0.68, 1.0];
    let warn_fg = [0.95, 0.75, 0.30, 1.0];

    // Warning icon + message.
    let line1_y = dialog_y + cell_h * 1.0;
    let msg = format!("\"{}\" is running in this pane.", proc_name);
    state
        .renderer
        .render_text(view, &msg, text_x, line1_y, warn_fg, bg);

    // Question.
    let line2_y = line1_y + cell_h * 1.2;
    state
        .renderer
        .render_text(view, "Close anyway?", text_x, line2_y, text_fg, bg);

    // Key hints.
    let line3_y = line2_y + cell_h * 1.4;
    state.renderer.render_text(
        view,
        "Y = Close    N / Esc = Cancel",
        text_x,
        line3_y,
        hint_fg,
        bg,
    );
}

/// Render the kill-and-close confirmation dialog (Cmd+Shift+W).
pub(crate) fn render_kill_confirm_dialog(
    state: &mut AppState,
    view: &wgpu::TextureView,
    phys_w: f32,
    phys_h: f32,
) {
    let cell_w = state.renderer.cell_size().width;
    let cell_h = state.renderer.cell_size().height;

    // Dark overlay behind the dialog.
    let overlay_color = [0.0, 0.0, 0.0, 0.55];
    state
        .renderer
        .submit_separator(view, 0, 0, phys_w as u32, phys_h as u32, overlay_color);

    // Dialog box dimensions.
    let dialog_w = (cell_w * 42.0).min(phys_w * 0.8);
    let dialog_h = cell_h * 5.0;
    let dialog_x = (phys_w - dialog_w) / 2.0;
    let dialog_y = (phys_h - dialog_h) / 2.0;

    // Dialog background.
    let bg = [0.12, 0.12, 0.16, 0.97];
    let border_color = [0.35, 0.35, 0.50, 0.9];
    state.renderer.submit_rounded_rects(
        view,
        &[RoundedRect {
            rect: [dialog_x, dialog_y, dialog_w, dialog_h],
            color: bg,
            border_color,
            params: [8.0, 1.0, 0.0, 0.0],
        }],
    );

    let text_x = dialog_x + cell_w * 1.5;
    let hint_fg = [0.60, 0.60, 0.68, 1.0];
    let warn_fg = [0.95, 0.75, 0.30, 1.0];

    // Warning message.
    let line1_y = dialog_y + cell_h * 1.0;
    state
        .renderer
        .render_text(view, "Kill daemon session for this pane?", text_x, line1_y, warn_fg, bg);

    // Key hints.
    let line2_y = line1_y + cell_h * 1.4;
    state.renderer.render_text(
        view,
        "Y = Kill & Close  N = Close only  Esc = Cancel",
        text_x,
        line2_y,
        hint_fg,
        bg,
    );
}

/// Render the pane↔tab move confirmation dialog.
pub(crate) fn render_pane_tab_confirm_dialog(
    state: &mut AppState,
    view: &wgpu::TextureView,
    phys_w: f32,
    phys_h: f32,
    confirm: &PaneTabConfirm,
) {
    let cell_w = state.renderer.cell_size().width;
    let cell_h = state.renderer.cell_size().height;

    // Dark overlay behind the dialog.
    let overlay_color = [0.0, 0.0, 0.0, 0.55];
    state
        .renderer
        .submit_separator(view, 0, 0, phys_w as u32, phys_h as u32, overlay_color);

    // Dialog box dimensions.
    let dialog_w = (cell_w * 42.0).min(phys_w * 0.8);
    let dialog_h = cell_h * 5.0;
    let dialog_x = (phys_w - dialog_w) / 2.0;
    let dialog_y = (phys_h - dialog_h) / 2.0;

    // Dialog background.
    let bg = [0.12, 0.12, 0.16, 0.97];
    let border_color = [0.35, 0.35, 0.50, 0.9];
    state.renderer.submit_rounded_rects(
        view,
        &[RoundedRect {
            rect: [dialog_x, dialog_y, dialog_w, dialog_h],
            color: bg,
            border_color,
            params: [8.0, 1.0, 0.0, 0.0],
        }],
    );

    let text_x = dialog_x + cell_w * 1.5;
    let text_fg = [0.92, 0.92, 0.96, 1.0];
    let hint_fg = [0.60, 0.60, 0.68, 1.0];
    let warn_fg = [0.95, 0.75, 0.30, 1.0];

    // Description of the action.
    let line1_y = dialog_y + cell_h * 1.0;
    let msg = match confirm {
        PaneTabConfirm::PaneToTab { .. } => "Move this pane to a new tab?",
        PaneTabConfirm::TabToPane { .. } => "Merge this tab into the current pane?",
    };
    state
        .renderer
        .render_text(view, msg, text_x, line1_y, warn_fg, bg);

    // Question.
    let line2_y = line1_y + cell_h * 1.2;
    state
        .renderer
        .render_text(view, "Continue?", text_x, line2_y, text_fg, bg);

    // Key hints.
    let line3_y = line2_y + cell_h * 1.4;
    state.renderer.render_text(
        view,
        "Y = OK    N / Esc = Cancel",
        text_x,
        line3_y,
        hint_fg,
        bg,
    );
}

/// Render the "About Termojinal" overlay on top of the terminal.
pub(crate) fn render_about_overlay(state: &mut AppState, view: &wgpu::TextureView, phys_w: f32, phys_h: f32) {
    let cell_size = state.renderer.cell_size();
    let cell_w = cell_size.width;
    let cell_h = cell_size.height;

    let pc = &state.config.palette;

    // 1. Semi-transparent dark overlay covering the entire window.
    let overlay_color = color_or(&pc.overlay_color, [0.0, 0.0, 0.0, 0.5]);
    state
        .renderer
        .submit_separator(view, 0, 0, phys_w as u32, phys_h as u32, overlay_color);

    // 2. Centered floating box (use most of the window).
    let box_w = (phys_w * 0.7).min(phys_w - 40.0).max(200.0);
    let box_h = (phys_h * 0.7).min(phys_h - 40.0).max(100.0);
    let box_x = (phys_w - box_w) / 2.0;
    let box_y = (phys_h - box_h) / 2.0;

    // Draw box background and border.
    let box_bg = color_or(&pc.bg, [0.12, 0.12, 0.16, 0.95]);
    let border_color = color_or(&pc.border_color, [0.3, 0.3, 0.4, 1.0]);
    let corner_radius = pc.corner_radius;
    let border_width = pc.border_width;
    let shadow_radius = pc.shadow_radius;
    let shadow_opacity = pc.shadow_opacity;

    let about_rect = RoundedRect {
        rect: [box_x, box_y, box_w, box_h],
        color: box_bg,
        border_color,
        params: [corner_radius, border_width, shadow_radius, shadow_opacity],
    };
    state.renderer.submit_rounded_rects(view, &[about_rect]);

    // 3. Load and render the about text.
    let about_text = load_about_text();
    let lines: Vec<&str> = about_text.lines().collect();
    let max_chars = ((box_w - 2.0 * cell_w) / cell_w) as usize;
    let content_x = box_x + cell_w;

    // Reserve space for the footer hint line.
    let footer_h = cell_h * 1.5;
    let content_area_h = box_h - cell_h * 0.5 - footer_h;
    let max_visible_lines = (content_area_h / cell_h) as usize;

    // Clamp scroll offset.
    let max_scroll = lines.len().saturating_sub(max_visible_lines);
    if state.about_scroll > max_scroll {
        state.about_scroll = max_scroll;
    }
    let scroll = state.about_scroll;

    // Title/header styling.
    let title_fg = [0.95, 0.95, 0.95, 1.0];
    let text_fg = [0.75, 0.75, 0.78, 1.0];
    let hint_fg = [0.5, 0.5, 0.55, 1.0];

    for (i, line) in lines
        .iter()
        .enumerate()
        .skip(scroll)
        .take(max_visible_lines)
    {
        let row = i - scroll;
        let y = box_y + cell_h * 0.25 + (row as f32) * cell_h;
        let display: String = line.chars().take(max_chars).collect();

        // Use brighter color for the first few header lines.
        let fg = if i < 4 { title_fg } else { text_fg };
        state
            .renderer
            .render_text(view, &display, content_x, y, fg, box_bg);
    }

    // 4. Footer: "Press any key to close" hint.
    let footer_y = box_y + box_h - cell_h * 1.25;

    // Separator above footer.
    let sep_color = color_or(&pc.separator_color, [0.25, 0.25, 0.3, 1.0]);
    state.renderer.submit_separator(
        view,
        box_x as u32 + 1,
        (footer_y - cell_h * 0.25) as u32,
        box_w as u32 - 2,
        1,
        sep_color,
    );

    let scroll_hint = if max_scroll > 0 {
        "Arrow keys to scroll, any other key to close"
    } else {
        "Press any key to close"
    };
    state
        .renderer
        .render_text(view, scroll_hint, content_x, footer_y, hint_fg, box_bg);
}

/// Render the Session Picker overlay — a command-palette-style floating panel
/// that lets the user choose between attaching an existing unattached session
/// or creating a new one.
pub(crate) fn render_session_picker(
    state: &mut AppState,
    view: &wgpu::TextureView,
    phys_w: f32,
    phys_h: f32,
) {
    let cell_size = state.renderer.cell_size();
    let cell_w = cell_size.width;
    let cell_h = cell_size.height;

    let pc = &state.config.palette;

    // 1. Semi-transparent overlay.
    let overlay_color = color_or(&pc.overlay_color, [0.0, 0.0, 0.0, 0.55]);
    state
        .renderer
        .submit_separator(view, 0, 0, phys_w as u32, phys_h as u32, overlay_color);

    // 2. Centered floating box — match QuickLaunch layout exactly.
    let max_visible_items: usize = 10;
    let box_w = (phys_w * pc.width_ratio).min(phys_w - 40.0).max(200.0);
    let item_count = state.session_picker.filtered.len();
    let visible_items = item_count.min(max_visible_items);
    let rows_needed = 1 + visible_items.max(1);
    let row_h = cell_h * 2.2; // label + detail line + gap
    let box_h = ((rows_needed as f32) * row_h + cell_h * 2.0).min(pc.max_height);
    let box_x = (phys_w - box_w) / 2.0;
    let box_y = (phys_h * 0.18).min(phys_h - box_h - 20.0).max(20.0);

    let box_bg = color_or(&pc.bg, [0.12, 0.12, 0.16, 0.95]);
    let border_color = [0.25, 0.78, 0.72, 1.0]; // teal accent
    let corner_radius = pc.corner_radius;
    let border_width = pc.border_width;
    let shadow_radius = pc.shadow_radius;
    let shadow_opacity = pc.shadow_opacity;

    let rect = RoundedRect {
        rect: [box_x, box_y, box_w, box_h],
        color: box_bg,
        border_color,
        params: [corner_radius, border_width, shadow_radius, shadow_opacity],
    };
    state.renderer.submit_rounded_rects(view, &[rect]);

    // 3. Input field (same as QuickLaunch).
    let input_y = box_y + cell_h * 0.25;
    let input_x = box_x + cell_w;
    let max_chars = ((box_w - cell_w * 2.0) / cell_w).max(1.0) as usize;
    let palette_input_fg = color_or(&pc.input_fg, [0.92, 0.92, 0.95, 1.0]);

    let prompt = format!("\u{F0668} {}", state.session_picker.input);
    let prompt_display: String = prompt.chars().take(max_chars).collect();
    state
        .renderer
        .render_text(view, &prompt_display, input_x, input_y, palette_input_fg, box_bg);

    // Separator line.
    let sep_y = input_y + cell_h + 2.0;
    let sep_color = color_or(&pc.separator_color, [0.2, 0.2, 0.3, 1.0]);
    state.renderer.submit_separator(
        view,
        (box_x + cell_w * 0.5) as u32,
        sep_y as u32,
        (box_w - cell_w) as u32,
        1,
        sep_color,
    );

    // 4. Result list.
    let list_start_y = sep_y + 4.0;
    let selected_bg = color_or(&pc.selected_bg, [0.22, 0.22, 0.32, 1.0]);
    let cmd_fg = color_or(&pc.command_fg, [0.8, 0.8, 0.84, 1.0]);
    let desc_fg = color_or(&pc.description_fg, [0.5, 0.5, 0.55, 1.0]);

    let sp = &mut state.session_picker;
    sp.ensure_visible(max_visible_items);
    let scroll_offset = sp.scroll_offset;

    for (vi, &entry_idx) in sp
        .filtered
        .iter()
        .enumerate()
        .skip(scroll_offset)
        .take(max_visible_items)
    {
        let row = vi - scroll_offset;
        let item_y = list_start_y + (row as f32) * row_h;
        if item_y + row_h > box_y + box_h {
            break;
        }

        let is_selected = vi == sp.selected;
        let bg = if is_selected { selected_bg } else { box_bg };

        if is_selected {
            let sel_rect = RoundedRect {
                rect: [box_x + 1.0, item_y, box_w - 2.0, row_h],
                color: selected_bg,
                border_color: [0.0; 4],
                params: [4.0, 0.0, 0.0, 0.0],
            };
            state.renderer.submit_rounded_rects(view, &[sel_rect]);
        }

        let entry = &sp.entries[entry_idx];

        // Icon.
        let (icon, icon_fg) = if entry.session_id.is_none() {
            ("\u{F0415} ", [0.4, 0.9, 0.5, 1.0]) // plus icon, green
        } else {
            ("\u{F0489} ", [0.25, 0.78, 0.72, 1.0]) // terminal icon, teal
        };
        state
            .renderer
            .render_text(view, icon, input_x, item_y, icon_fg, bg);

        // Label.
        let label_display: String = entry
            .label
            .chars()
            .take(max_chars.saturating_sub(3))
            .collect();
        let fg = if is_selected { palette_input_fg } else { cmd_fg };
        state
            .renderer
            .render_text(view, &label_display, input_x + cell_w * 3.0, item_y, fg, bg);

        // Detail line (dimmed).
        let detail_display: String = entry
            .detail
            .chars()
            .take(max_chars.saturating_sub(4))
            .collect();
        state.renderer.render_text(
            view,
            &detail_display,
            input_x + cell_w * 3.0,
            item_y + cell_h * 0.85,
            desc_fg,
            bg,
        );
    }

    if sp.filtered.is_empty() {
        let empty_fg = [0.5, 0.5, 0.55, 1.0];
        state.renderer.render_text(
            view,
            "No matching sessions",
            input_x,
            list_start_y,
            empty_fg,
            box_bg,
        );
    }

    // Separator above hint.
    let hint_sep_y = box_y + box_h - cell_h * 1.8;
    state.renderer.submit_separator(
        view,
        (box_x + cell_w * 0.5) as u32,
        hint_sep_y as u32,
        (box_w - cell_w) as u32,
        1,
        sep_color,
    );

    // Hint at bottom — keep inside the box with padding from the border.
    let hint = "\u{21B5} Select  \u{2191}\u{2193} Navigate  esc Cancel";
    let hint_y = box_y + box_h - cell_h * 1.4;
    let hint_fg = [0.4, 0.4, 0.5, 0.7];
    state
        .renderer
        .render_text(view, hint, input_x, hint_y, hint_fg, box_bg);
}
