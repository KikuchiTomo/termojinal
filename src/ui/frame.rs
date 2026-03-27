use crate::*;
use config::color_or;
use termojinal_render::RoundedRect;

/// Render all panes with tab bar and sidebar.
pub(crate) fn render_frame(state: &mut AppState) -> Result<(), termojinal_render::RenderError> {
    // Poll the background brew update check result.
    if state.update_checker.available_version.is_none() {
        if let Ok(guard) = state.update_check_result.lock() {
            if let Some(ref ver) = *guard {
                let v = ver.clone();
                drop(guard);
                log::info!(
                    "update available via Homebrew: v{v} (current: {})",
                    env!("CARGO_PKG_VERSION")
                );
                state.update_checker.available_version = Some(v.clone());
                // Send a desktop notification about the update.
                let title = "Termojinal Update Available";
                let body = format!(
                    "v{} is available (current: v{}). Run: brew upgrade termojinal",
                    v,
                    env!("CARGO_PKG_VERSION")
                );
                notification::send_notification(title, &body, false);
            }
        }
    }

    let size = state.window.inner_size();
    let phys_w = size.width as f32;
    let phys_h = size.height as f32;
    let pane_rects = active_pane_rects(state);
    let focused_id = active_tab(state).layout.focused();
    let has_tab_bar = tab_bar_visible(state);

    // Always use the multi-pane path since we may have the tab bar/sidebar occupying space.
    let output = state.renderer.get_surface_texture()?;
    let view = output
        .texture
        .create_view(&wgpu::TextureViewDescriptor::default());

    // Clear entire surface.
    state.renderer.clear_surface(&view);

    // Fill content area with terminal background (prevents transparent padding).
    // Apply bg_opacity so transparency still works.
    {
        let mut term_bg = color_or(&state.config.theme.background, [0.067, 0.067, 0.09, 1.0]);
        term_bg[3] = state.config.window.opacity;
        let sidebar_w = if state.sidebar_visible {
            state.sidebar_width
        } else {
            0.0
        };
        let tab_h = if has_tab_bar {
            state.config.tab_bar.height
        } else {
            0.0
        };
        let status_h = effective_status_bar_height(state);
        let content_x = sidebar_w as u32;
        let content_y = tab_h as u32;
        let content_w = (phys_w - sidebar_w).max(0.0) as u32;
        let content_h = (phys_h - tab_h - status_h).max(0.0) as u32;
        state
            .renderer
            .submit_separator(&view, content_x, content_y, content_w, content_h, term_bg);
    }

    // Render sidebar if visible (suppressed in Quick Terminal when configured).
    let sidebar_shown = state.sidebar_visible
        && !(state.quick_terminal.visible && !state.config.quick_terminal.show_sidebar);
    if sidebar_shown {
        render_sidebar(state, &view, phys_h);
    }

    // Render tab bar if visible (always_show or >1 tabs).
    if has_tab_bar {
        render_tab_bar(state, &view, phys_w);
    }

    // Render each pane.
    let ws_idx = state.active_workspace;
    let tab_idx = state.workspaces[ws_idx].active_tab;
    for (pid, rect) in &pane_rects {
        if let Some(pane) = state.workspaces[ws_idx].tabs[tab_idx].panes.get(pid) {
            let sel_bounds = sel_bounds_for(pane);
            let preedit = if *pid == focused_id {
                pane.preedit.as_deref()
            } else {
                None
            };
            // Pass search matches only for the focused pane.
            let (s_matches, s_current_idx) = if *pid == focused_id {
                if let Some(ref search) = state.search {
                    if !search.matches.is_empty() {
                        (Some(search.matches.as_slice()), Some(search.current))
                    } else {
                        (None, None)
                    }
                } else {
                    (None, None)
                }
            } else {
                (None, None)
            };
            let viewport = (
                rect.x as u32,
                rect.y as u32,
                (rect.w as u32).max(1),
                (rect.h as u32).max(1),
            );
            state.renderer.render_pane(
                &pane.terminal,
                sel_bounds,
                viewport,
                *pid,
                preedit,
                &view,
                s_matches,
                s_current_idx,
            )?;
        }
    }

    // Draw separators between panes.
    let sep_color = color_or(&state.config.pane.separator_color, [0.3, 0.3, 0.3, 1.0]);
    let sep = state.config.pane.separator_width;
    for i in 0..pane_rects.len() {
        for j in (i + 1)..pane_rects.len() {
            let (_, r1) = &pane_rects[i];
            let (_, r2) = &pane_rects[j];

            let r1_right = (r1.x + r1.w) as u32;
            let r2_left = r2.x as u32;
            if r1_right.abs_diff(r2_left) <= 1 {
                let y0 = r1.y.max(r2.y) as u32;
                let y1 = (r1.y + r1.h).min(r2.y + r2.h) as u32;
                if y1 > y0 {
                    state.renderer.submit_separator(
                        &view,
                        r1_right.saturating_sub(sep / 2),
                        y0,
                        sep,
                        y1 - y0,
                        sep_color,
                    );
                }
            }

            let r1_bottom = (r1.y + r1.h) as u32;
            let r2_top = r2.y as u32;
            if r1_bottom.abs_diff(r2_top) <= 1 {
                let x0 = r1.x.max(r2.x) as u32;
                let x1 = (r1.x + r1.w).min(r2.x + r2.w) as u32;
                if x1 > x0 {
                    state.renderer.submit_separator(
                        &view,
                        x0,
                        r1_bottom.saturating_sub(sep / 2),
                        x1 - x0,
                        sep,
                        sep_color,
                    );
                }
            }
        }
    }

    // Focus border on the focused pane (only when multiple panes).
    if pane_rects.len() > 1 {
        let focus_color = color_or(&state.config.pane.focus_border_color, [0.2, 0.6, 1.0, 0.8]);
        let b = state.config.pane.focus_border_width;
        if let Some((_, r)) = pane_rects.iter().find(|(id, _)| *id == focused_id) {
            let (x, y, w, h) = (r.x as u32, r.y as u32, r.w as u32, r.h as u32);
            state
                .renderer
                .submit_separator(&view, x, y, w, b, focus_color);
            if h > b {
                state
                    .renderer
                    .submit_separator(&view, x, y + h - b, w, b, focus_color);
            }
            state
                .renderer
                .submit_separator(&view, x, y, b, h, focus_color);
            if w > b {
                state
                    .renderer
                    .submit_separator(&view, x + w - b, y, b, h, focus_color);
            }
        }
    }

    // Draw drop zone preview when a tab is being dragged into a pane.
    if let Some(ref drag) = state.tab_pane_drag {
        if let Some((_, rect)) = pane_rects.iter().find(|(id, _)| *id == drag.target_pane) {
            // Compute the preview rectangle based on the drop zone.
            let (zx, zy, zw, zh) = match drag.zone {
                DropZone::Top => (rect.x, rect.y, rect.w, rect.h * 0.5),
                DropZone::Bottom => (rect.x, rect.y + rect.h * 0.5, rect.w, rect.h * 0.5),
                DropZone::Left => (rect.x, rect.y, rect.w * 0.5, rect.h),
                DropZone::Right => (rect.x + rect.w * 0.5, rect.y, rect.w * 0.5, rect.h),
            };
            let inset = 4.0_f32;
            state.renderer.submit_rounded_rects(
                &view,
                &[RoundedRect {
                    rect: [
                        zx + inset,
                        zy + inset,
                        (zw - inset * 2.0).max(1.0),
                        (zh - inset * 2.0).max(1.0),
                    ],
                    color: [0.2, 0.5, 1.0, 0.18],
                    border_color: [0.2, 0.5, 1.0, 0.4],
                    params: [8.0, 2.0, 0.0, 0.0], // corner_radius, border_width, no shadow
                }],
            );
        }
    }

    // Update IME cursor position for the focused pane.
    if let Some((_, rect)) = pane_rects.iter().find(|(id, _)| *id == focused_id) {
        if let Some(fp) = state.workspaces[ws_idx].tabs[tab_idx]
            .panes
            .get(&focused_id)
        {
            let cell_size = state.renderer.cell_size();
            let x = rect.x + (fp.terminal.cursor_col as f32 * cell_size.width);
            let y = rect.y + (fp.terminal.cursor_row as f32 * cell_size.height);
            state.window.set_ime_cursor_area(
                winit::dpi::PhysicalPosition::new(x as f64, y as f64),
                winit::dpi::PhysicalSize::new(cell_size.width as f64, cell_size.height as f64),
            );
        }
    }

    // Render status bar at the bottom if enabled.
    if state.config.status_bar.enabled {
        render_status_bar(state, &view, phys_w, phys_h);
    }

    // Render search bar if visible (Feature 5).
    if state.search.is_some() {
        render_search_bar(state, &view, phys_w);
    }

    // Render Allow Flow overlay at the bottom of the focused pane.
    // Render Allow Flow pane hint bar (thin 1-line bar at the bottom of the
    // focused pane) when there are pending requests for ANY workspace.
    // This ensures cross-workspace approval works — the user sees the hint
    // bar even when the pending request belongs to a different workspace.
    if state.allow_flow.pane_hint_visible
        && state.allow_flow.first_workspace_with_pending().is_some()
    {
        render_allow_flow_pane_hint(state, &view, &pane_rects, focused_id);
    }

    // Render command palette overlay if visible.
    if state.command_palette.visible {
        render_command_palette(state, &view, phys_w, phys_h);
    }

    // Render Quick Launch overlay if visible.
    if state.quick_launch.visible {
        render_quick_launch(state, &view, phys_w, phys_h);
    }

    // Render Session Picker overlay if visible.
    if state.session_picker.visible {
        render_session_picker(state, &view, phys_w, phys_h);
    }

    // Render command timeline overlay if visible.
    if state.timeline_visible {
        render_command_timeline(state, &view, phys_w, phys_h);
    }

    // Render Claudes Dashboard overlay if visible.
    if state.claudes_dashboard.visible {
        // Auto-refresh data every second.
        if state.claudes_dashboard.last_refresh.elapsed() >= std::time::Duration::from_secs(1) {
            refresh_claudes_dashboard(state);
        }
        render_claudes_dashboard(state, &view, phys_w, phys_h);
        state.needs_animation_frame = true;
    }

    // Render About overlay if visible.
    if state.about_visible {
        render_about_overlay(state, &view, phys_w, phys_h);
    }

    // Render close confirmation dialog if pending.
    if let Some((ref proc_name, _)) = state.pending_close_confirm.clone() {
        render_close_confirm_dialog(state, &view, phys_w, phys_h, proc_name);
    }

    output.present();
    Ok(())
}
