//! Pane rendering, instance building, and viewport management.

use super::types::*;
use super::Renderer;
use crate::color_convert;
use crate::emoji_atlas;

impl Renderer {
    // -----------------------------------------------------------------------
    // Instance building helpers
    // -----------------------------------------------------------------------

    /// Build instance data for a single row of the terminal grid.
    pub(crate) fn build_row_instances(
        &mut self,
        terminal: &termojinal_vt::Terminal,
        row: usize,
        selection: Option<((usize, usize), (usize, usize))>,
        search_matches: Option<&[(usize, usize, usize)]>, // (row, col_start, col_end)
        search_current_idx: Option<usize>,
        link_hover: Option<(usize, usize, usize)>, // (row, col_start, col_end) inclusive
    ) -> Vec<CellInstance> {
        let grid = terminal.grid();
        let cols = grid.cols();
        let scroll_offset = terminal.scroll_offset();

        let (cell_source_scrollback, scrollback_idx, grid_row) = if scroll_offset > 0 {
            if row < scroll_offset {
                (true, scroll_offset - 1 - row, 0)
            } else {
                (false, 0, row - scroll_offset)
            }
        } else {
            (false, 0, row)
        };

        let mut row_instances = Vec::with_capacity(cols);
        for col in 0..cols {
            let cell = if cell_source_scrollback {
                match terminal.scrollback_row(scrollback_idx) {
                    Some(cells) if col < cells.len() => cells[col],
                    _ => termojinal_vt::Cell::default(),
                }
            } else if grid_row < grid.rows() {
                *grid.cell(col, grid_row)
            } else {
                termojinal_vt::Cell::default()
            };

            // Skip continuation cells (width == 0).
            if cell.width == 0 {
                continue;
            }

            let c = if cell.c == '\0' || cell.c == ' ' {
                ' '
            } else {
                cell.c
            };

            // Check if this character is an emoji and get glyph from the
            // appropriate atlas.  For non-emoji characters that fontdue can't
            // render, fall back to the emoji atlas (Core Text) which handles
            // font cascading and can render virtually any Unicode character.
            let (glyph, is_emoji_cell) = if emoji_atlas::is_emoji(c) {
                if let Some(eg) = self.emoji_atlas.get_glyph(c) {
                    (eg, true)
                } else {
                    (self.atlas.get_glyph(c), false)
                }
            } else {
                let mono_glyph = self.atlas.get_glyph(c);
                // If the monospace atlas returned an empty glyph (all-zero region
                // in the atlas) for a non-trivial character, try the emoji atlas
                // as a Core Text fallback.  Also try the emoji atlas for
                // "text emoji" characters (Emoji=Yes, Emoji_Presentation=No,
                // e.g. ⏺ U+23FA, ✔ U+2714) that the mono atlas may lack.
                let try_emoji_fallback = (c > ' '
                    && !c.is_control()
                    && mono_glyph.atlas_w > 0.0
                    && self.atlas.is_glyph_empty(c))
                    || emoji_atlas::is_text_emoji(c);
                if try_emoji_fallback {
                    if let Some(eg) = self.emoji_atlas.get_glyph(c) {
                        (eg, true)
                    } else {
                        (mono_glyph, false)
                    }
                } else {
                    (mono_glyph, false)
                }
            };

            let fg = color_convert::color_to_rgba_themed(cell.fg, true, &self.theme_palette);
            let bg = color_convert::color_to_rgba_themed(cell.bg, false, &self.theme_palette);

            let mut flags = cell.attrs.bits() as u32;

            if is_emoji_cell {
                flags |= FLAG_EMOJI;
            }

            // Mark cursor cell (only when viewing live output).
            if scroll_offset == 0
                && terminal.modes.cursor_visible
                && col == terminal.cursor_col
                && row == terminal.cursor_row
            {
                flags |= FLAG_IS_CURSOR;
            }

            // Mark selected cells.
            if let Some(((sc, sr), (ec, er))) = selection {
                let selected = if row < sr || row > er {
                    false
                } else if row == sr && row == er {
                    col >= sc && col <= ec
                } else if row == sr {
                    col >= sc
                } else if row == er {
                    col <= ec
                } else {
                    true
                };
                if selected {
                    flags |= FLAG_SELECTED;
                }
            }

            // Mark search match cells.
            if let Some(matches) = search_matches {
                for (match_idx, &(m_row, m_col_start, m_col_end)) in matches.iter().enumerate() {
                    if m_row == row && col >= m_col_start && col <= m_col_end {
                        if search_current_idx == Some(match_idx) {
                            flags |= FLAG_SEARCH_CURRENT;
                        } else {
                            flags |= FLAG_SEARCH;
                        }
                        break;
                    }
                }
            }

            // Mark link hover cells with underline.
            if let Some((h_row, h_col_start, h_col_end)) = link_hover {
                if row == h_row && col >= h_col_start && col <= h_col_end {
                    flags |= FLAG_UNDERLINE;
                }
            }

            let width_scale = if cell.width > 1 {
                cell.width as f32
            } else {
                1.0
            };

            // Apply background opacity: default bg gets transparent, colored bg stays opaque.
            // In alternate screen mode (TUI apps like neovim), never make bg transparent
            // — respect the app's background color.
            let mut bg_final = bg;
            if !terminal.modes.alternate_screen && bg == self.theme_palette.bg {
                // Use the configured default_bg (from theme) with opacity applied.
                bg_final = self.default_bg;
                bg_final[3] = self.bg_opacity;
            }

            row_instances.push(CellInstance {
                grid_pos: [col as f32, row as f32],
                atlas_uv: [glyph.atlas_x, glyph.atlas_y, glyph.atlas_w, glyph.atlas_h],
                fg_color: fg,
                bg_color: bg_final,
                flags,
                cell_width_scale: width_scale,
                _pad: [0; 2],
            });
        }
        row_instances
    }

    /// Build scrollbar instances. Returns the instances to append after all rows.
    pub(crate) fn build_scrollbar_instances(
        &mut self,
        terminal: &termojinal_vt::Terminal,
    ) -> Vec<CellInstance> {
        let grid = terminal.grid();
        let cols = grid.cols();
        let rows = grid.rows();
        let scroll_offset = terminal.scroll_offset();
        let scrollback_len = terminal.scrollback_len();

        if scrollback_len == 0 {
            return Vec::new();
        }

        let space_glyph = self.atlas.get_glyph(' ');

        let total_lines = scrollback_len + rows;
        let thumb_height_f = (rows as f64 / total_lines as f64 * rows as f64).max(1.0);
        let thumb_top_f =
            (scrollback_len - scroll_offset) as f64 / total_lines as f64 * rows as f64;

        let thumb_top = thumb_top_f.floor() as usize;
        let thumb_bottom = ((thumb_top_f + thumb_height_f).ceil() as usize).min(rows);

        // Fixed-pixel scrollbar width converted to grid units.
        let cell_w = self.atlas.cell_size.width;
        let scrollbar_grid_w = (self.scrollbar_width_px / cell_w).max(0.05);
        // Place scrollbar at the right edge of the last column (inside clip area).
        let scrollbar_x = (cols as f32) - scrollbar_grid_w;

        let mut instances = Vec::with_capacity(rows);
        for r in 0..rows {
            let is_thumb = r >= thumb_top && r < thumb_bottom;
            let bg = if is_thumb {
                [1.0_f32, 1.0, 1.0, self.scrollbar_thumb_opacity]
            } else {
                [1.0_f32, 1.0, 1.0, self.scrollbar_track_opacity]
            };
            instances.push(CellInstance {
                grid_pos: [scrollbar_x, r as f32],
                atlas_uv: [
                    space_glyph.atlas_x,
                    space_glyph.atlas_y,
                    space_glyph.atlas_w,
                    space_glyph.atlas_h,
                ],
                fg_color: bg,
                bg_color: bg,
                flags: 0,
                cell_width_scale: 1.0,
                _pad: [0; 2],
            });
        }
        instances
    }

    /// Compute scrollbar geometry in pixel coordinates relative to the pane.
    ///
    /// Returns `None` if there is no scrollback content.
    pub fn scrollbar_geometry(
        &self,
        terminal: &termojinal_vt::Terminal,
    ) -> Option<ScrollbarGeometry> {
        let grid = terminal.grid();
        let cols = grid.cols();
        let rows = grid.rows();
        let scroll_offset = terminal.scroll_offset();
        let scrollback_len = terminal.scrollback_len();

        if scrollback_len == 0 {
            return None;
        }

        let cell_w = self.atlas.cell_size.width;
        let cell_h = self.atlas.cell_size.height;

        let scrollbar_grid_w = (self.scrollbar_width_px / cell_w).max(0.05);
        let track_x_px = (cols as f32 - scrollbar_grid_w) * cell_w;
        let track_width_px = self.scrollbar_width_px;
        let total_height_px = rows as f32 * cell_h;

        let total_lines = scrollback_len + rows;
        let thumb_height_f = (rows as f64 / total_lines as f64 * rows as f64).max(1.0);
        let thumb_top_f =
            (scrollback_len - scroll_offset) as f64 / total_lines as f64 * rows as f64;

        let thumb_top_px = thumb_top_f as f32 * cell_h;
        let thumb_bottom_px =
            ((thumb_top_f + thumb_height_f) as f32 * cell_h).min(total_height_px);

        Some(ScrollbarGeometry {
            track_x: track_x_px,
            track_width: track_width_px,
            thumb_top: thumb_top_px,
            thumb_bottom: thumb_bottom_px,
            total_height: total_height_px,
            rows,
            scrollback_len,
        })
    }

    /// Set which pane is being rendered (selects the per-pane cache).
    pub(crate) fn set_active_pane(&mut self, pane_key: PaneKey) {
        self.current_pane_key = pane_key;
    }

    /// Build or incrementally update instance data. Returns the total instance count.
    ///
    /// When the grid reports dirty rows and the grid dimensions/scroll offset
    /// haven't changed, only the dirty rows are rebuilt and patched into the
    /// GPU buffer, avoiding a full re-upload.
    pub(crate) fn update_instances(
        &mut self,
        terminal: &termojinal_vt::Terminal,
        selection: Option<((usize, usize), (usize, usize))>,
        search_matches: Option<&[(usize, usize, usize)]>,
        search_current_idx: Option<usize>,
        link_hover: Option<(usize, usize, usize)>,
    ) -> usize {
        let key = self.current_pane_key;
        self.pane_caches.entry(key).or_default();

        let grid = terminal.grid();
        let cols = grid.cols();
        let rows = grid.rows();
        let scroll_offset = terminal.scroll_offset();

        let cache = &self.pane_caches[&key];
        let dims_match = cache.grid_dims == (cols, rows);
        let scroll_match = cache.scroll_offset == scroll_offset;
        let selection_match = cache.selection == selection;
        let search_match = cache.search_matches.as_deref() == search_matches
            && cache.search_current_idx == search_current_idx;
        let link_hover_match = cache.link_hover == link_hover;

        let can_incremental = dims_match
            && scroll_match
            && selection_match
            && search_match
            && link_hover_match
            && !cache.instances.is_empty()
            && grid.any_dirty();

        if can_incremental {
            let instance_stride = std::mem::size_of::<CellInstance>();

            for row in 0..rows {
                let row_dirty = if scroll_offset == 0 {
                    grid.is_row_dirty(row)
                } else if row >= scroll_offset {
                    grid.is_row_dirty(row - scroll_offset)
                } else {
                    false
                };

                let is_cursor_row = scroll_offset == 0 && row == terminal.cursor_row;

                if !row_dirty && !is_cursor_row {
                    continue;
                }

                let new_row_instances = self.build_row_instances(
                    terminal,
                    row,
                    selection,
                    search_matches,
                    search_current_idx,
                    link_hover,
                );

                let cache = &self.pane_caches[&key];
                // Guard against stale row_instance_counts after resize
                if row >= cache.row_instance_counts.len() {
                    return self.full_rebuild(
                        terminal,
                        selection,
                        search_matches,
                        search_current_idx,
                        link_hover,
                    );
                }
                let row_start_instance: usize = cache.row_instance_counts[..row].iter().sum();
                let old_count = cache.row_instance_counts[row];

                if new_row_instances.len() == old_count {
                    let Some(cache) = self.pane_caches.get_mut(&key) else {
                        return self.full_rebuild(
                            terminal,
                            selection,
                            search_matches,
                            search_current_idx,
                            link_hover,
                        );
                    };
                    // Guard against instance buffer out-of-bounds
                    if row_start_instance + new_row_instances.len() > cache.instances.len() {
                        return self.full_rebuild(
                            terminal,
                            selection,
                            search_matches,
                            search_current_idx,
                            link_hover,
                        );
                    }
                    for (i, inst) in new_row_instances.iter().enumerate() {
                        cache.instances[row_start_instance + i] = *inst;
                    }
                    let byte_offset = (row_start_instance * instance_stride) as u64;
                    self.queue.write_buffer(
                        &self.instance_buffer,
                        byte_offset,
                        bytemuck::cast_slice(&new_row_instances),
                    );
                } else {
                    return self.full_rebuild(
                        terminal,
                        selection,
                        search_matches,
                        search_current_idx,
                        link_hover,
                    );
                }
            }

            grid.clear_dirty();
            return self.pane_caches[&key].instance_count;
        }

        // Check if nothing changed at all.
        let cache = &self.pane_caches[&key];
        if !grid.any_dirty()
            && dims_match
            && scroll_match
            && selection_match
            && search_match
            && link_hover_match
            && !cache.instances.is_empty()
        {
            return cache.instance_count;
        }

        self.full_rebuild(terminal, selection, search_matches, search_current_idx, link_hover)
    }

    /// Perform a full rebuild of all instance data for the current pane.
    pub(crate) fn full_rebuild(
        &mut self,
        terminal: &termojinal_vt::Terminal,
        selection: Option<((usize, usize), (usize, usize))>,
        search_matches: Option<&[(usize, usize, usize)]>,
        search_current_idx: Option<usize>,
        link_hover: Option<(usize, usize, usize)>,
    ) -> usize {
        let grid = terminal.grid();
        let cols = grid.cols();
        let rows = grid.rows();
        let scroll_offset = terminal.scroll_offset();
        let total_cells = cols * rows;

        let mut instances = Vec::with_capacity(total_cells);
        let mut row_counts = Vec::with_capacity(rows);

        for row in 0..rows {
            let row_instances = self.build_row_instances(
                terminal,
                row,
                selection,
                search_matches,
                search_current_idx,
                link_hover,
            );
            row_counts.push(row_instances.len());
            instances.extend_from_slice(&row_instances);
        }

        let scrollbar_instances = self.build_scrollbar_instances(terminal);
        instances.extend_from_slice(&scrollbar_instances);

        if instances.len() > self.instance_capacity {
            self.instance_capacity = instances.len().next_power_of_two();
            self.instance_buffer = self.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("cell instances"),
                size: (self.instance_capacity * std::mem::size_of::<CellInstance>()) as u64,
                usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
        }

        if !instances.is_empty() {
            self.queue
                .write_buffer(&self.instance_buffer, 0, bytemuck::cast_slice(&instances));
        }

        let count = instances.len();

        let key = self.current_pane_key;
        let cache = self.pane_caches.entry(key).or_default();
        cache.instances = instances;
        cache.instance_count = count;
        cache.scroll_offset = scroll_offset;
        cache.grid_dims = (cols, rows);
        cache.selection = selection;
        cache.row_instance_counts = row_counts;
        cache.search_matches = search_matches.map(|m| m.to_vec());
        cache.search_current_idx = search_current_idx;
        cache.link_hover = link_hover;

        grid.clear_dirty();
        count
    }

    // -----------------------------------------------------------------------
    // Uniform helpers
    // -----------------------------------------------------------------------

    /// Compute uniforms for full-surface rendering.
    pub(crate) fn compute_uniforms_full(
        &self,
        terminal: &termojinal_vt::Terminal,
    ) -> Uniforms {
        let cell_w = self.atlas.cell_size.width;
        let cell_h = self.atlas.cell_size.height;
        let surface_w = self.surface_config.width as f32;
        let surface_h = self.surface_config.height as f32;

        let cell_ndc_w = (cell_w / surface_w) * 2.0;
        let cell_ndc_h = (cell_h / surface_h) * 2.0;

        let grid_offset_x = -1.0 + (cell_w / surface_w) * 2.0;
        let grid_offset_y = 1.0 - (cell_h * 0.5 / surface_h) * 2.0;

        self.build_uniforms(
            terminal,
            cell_ndc_w,
            cell_ndc_h,
            grid_offset_x,
            grid_offset_y,
        )
    }

    /// Compute uniforms for viewport rendering.
    pub(crate) fn compute_uniforms_viewport(
        &self,
        terminal: &termojinal_vt::Terminal,
        vp_x: u32,
        vp_y: u32,
        vp_w: u32,
        vp_h: u32,
    ) -> Uniforms {
        let cell_w = self.atlas.cell_size.width;
        let cell_h = self.atlas.cell_size.height;
        let surface_w = self.surface_config.width as f32;
        let surface_h = self.surface_config.height as f32;

        // Cell size in NDC is still relative to the full surface (since NDC maps
        // to the full surface, and the scissor rect clips).
        let cell_ndc_w = (cell_w / surface_w) * 2.0;
        let cell_ndc_h = (cell_h / surface_h) * 2.0;

        // Grid offset: position the grid at the viewport origin.
        // No padding in viewport mode — each pane fills its viewport entirely.
        let vp_ndc_x = (vp_x as f32 / surface_w) * 2.0 - 1.0;
        let vp_ndc_y = 1.0 - (vp_y as f32 / surface_h) * 2.0;

        let grid_offset_x = vp_ndc_x;
        let grid_offset_y = vp_ndc_y;

        // Adjust cell_size based on viewport dimensions for grid_size calculations
        // (but the actual NDC cell size stays the same since NDC covers the full surface).
        // The viewport width/height affects how many cells fit, but the shader
        // cell_size must remain relative to the surface for correct positioning.
        let _ = (vp_w, vp_h); // Used by scissor rect, not needed in uniforms here.

        self.build_uniforms(
            terminal,
            cell_ndc_w,
            cell_ndc_h,
            grid_offset_x,
            grid_offset_y,
        )
    }

    /// Build a Uniforms struct with the given transform parameters.
    pub(crate) fn build_uniforms(
        &self,
        terminal: &termojinal_vt::Terminal,
        cell_ndc_w: f32,
        cell_ndc_h: f32,
        grid_offset_x: f32,
        grid_offset_y: f32,
    ) -> Uniforms {
        let cursor_shape = match terminal.cursor_shape {
            termojinal_vt::CursorShape::Block | termojinal_vt::CursorShape::BlinkingBlock => 0.0,
            termojinal_vt::CursorShape::Underline
            | termojinal_vt::CursorShape::BlinkingUnderline => 1.0,
            termojinal_vt::CursorShape::Bar | termojinal_vt::CursorShape::BlinkingBar => 2.0,
        };

        Uniforms {
            cell_size: [cell_ndc_w, cell_ndc_h],
            grid_offset: [grid_offset_x, grid_offset_y],
            atlas_size: [self.atlas.width as f32, self.atlas.height as f32],
            emoji_atlas_size: [
                self.emoji_atlas.width as f32,
                self.emoji_atlas.height as f32,
            ],
            cursor_pos: [
                terminal.cursor_col as f32,
                terminal.cursor_row as f32,
                0.85,
                0.85,
            ],
            cursor_extra: [
                0.85,
                cursor_shape,
                if self.cursor_blink_on { 1.0 } else { 0.0 },
                0.0,
            ],
        }
    }

    // -----------------------------------------------------------------------
    // Public rendering API
    // -----------------------------------------------------------------------

    /// Render the terminal grid to the surface (full-surface, backward-compatible).
    ///
    /// `selection` is an optional pair of `((start_col, start_row), (end_col, end_row))`
    /// in normalized (reading) order. Cells within the selection will have their
    /// fg/bg swapped via `FLAG_SELECTED`.
    pub fn render(
        &mut self,
        terminal: &termojinal_vt::Terminal,
        selection: Option<((usize, usize), (usize, usize))>,
        preedit: Option<&str>,
    ) -> Result<(), RenderError> {
        self.set_active_pane(0); // Single-pane always uses key 0.

        // Sync image textures if the image store has been modified.
        if terminal.image_store.has_placements() {
            self.image_renderer
                .sync_images(&self.device, &self.queue, &terminal.image_store);
        }

        let (output, mut encoder) = self.begin_frame()?;
        let view = output
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());

        self.render_viewport(terminal, selection, None, &mut encoder, &view)?;

        // Submit the main frame encoder first.
        self.queue.submit(std::iter::once(encoder.finish()));

        // Render preedit overlay if present (separate submit before present).
        if let Some(text) = preedit {
            self.render_preedit_overlay(terminal, text, None, &view);
        }

        output.present();
        Ok(())
    }

    /// Get the surface texture for multi-pane rendering.
    pub fn get_surface_texture(&mut self) -> Result<wgpu::SurfaceTexture, RenderError> {
        Ok(self.surface.get_current_texture()?)
    }

    /// Begin a frame: get the surface texture and encoder.
    pub fn begin_frame(
        &mut self,
    ) -> Result<(wgpu::SurfaceTexture, wgpu::CommandEncoder), RenderError> {
        let output = self.surface.get_current_texture()?;
        let encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("render encoder"),
            });
        Ok((output, encoder))
    }

    /// End a frame: submit the command encoder and present the surface texture.
    pub fn end_frame(&mut self, output: wgpu::SurfaceTexture, encoder: wgpu::CommandEncoder) {
        self.queue.submit(std::iter::once(encoder.finish()));
        output.present();
    }

    /// Clear the surface to the default background color. Submits immediately.
    pub fn clear_surface(&mut self, view: &wgpu::TextureView) {
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("clear encoder"),
            });
        {
            let _pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("clear pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color {
                            r: 0.0,
                            g: 0.0,
                            b: 0.0,
                            a: 0.0,
                        }),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
            });
        }
        self.queue.submit(std::iter::once(encoder.finish()));
    }

    /// Render a single pane into a viewport and submit immediately.
    /// Each pane gets its own encoder+submit cycle so buffer writes don't clobber.
    pub fn render_pane(
        &mut self,
        terminal: &termojinal_vt::Terminal,
        selection: Option<((usize, usize), (usize, usize))>,
        viewport: (u32, u32, u32, u32),
        pane_key: PaneKey,
        preedit: Option<&str>,
        view: &wgpu::TextureView,
        search_matches: Option<&[(usize, usize, usize)]>,
        search_current_idx: Option<usize>,
        link_hover: Option<(usize, usize, usize)>,
    ) -> Result<(), RenderError> {
        let (vp_x, vp_y, vp_w, vp_h) = viewport;

        // Sync image textures if the image store has been modified.
        if terminal.image_store.has_placements() {
            self.image_renderer
                .sync_images(&self.device, &self.queue, &terminal.image_store);
        }

        // Select the per-pane cache so dirty optimization works across panes.
        self.set_active_pane(pane_key);
        let instance_count =
            self.update_instances(terminal, selection, search_matches, search_current_idx, link_hover);

        // Re-upload atlas if needed.
        let current_glyph_count = self.atlas.glyph_count();
        if current_glyph_count != self.atlas_texture_version {
            self.reupload_atlas();
            self.atlas_texture_version = current_glyph_count;
        }

        // Re-upload emoji atlas if needed.
        let current_emoji_count = self.emoji_atlas.glyph_count();
        if current_emoji_count != self.emoji_texture_version {
            self.reupload_emoji_atlas();
            self.emoji_texture_version = current_emoji_count;
        }

        // Compute uniforms for this viewport.
        let uniforms = self.compute_uniforms_viewport(terminal, vp_x, vp_y, vp_w, vp_h);
        self.queue
            .write_buffer(&self.uniform_buffer, 0, bytemuck::bytes_of(&uniforms));

        // Create encoder and render pass.
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("pane encoder"),
            });
        {
            let mut render_pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("pane render pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Load,
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
            });
            render_pass.set_scissor_rect(vp_x, vp_y, vp_w, vp_h);
            render_pass.set_pipeline(&self.render_pipeline);
            render_pass.set_bind_group(0, &self.bind_group, &[]);
            render_pass.set_vertex_buffer(0, self.instance_buffer.slice(..));
            render_pass.draw(0..6, 0..instance_count as u32);
        }

        // Render inline images on top of cells.
        if terminal.image_store.has_placements() {
            let cell_w = self.atlas.cell_size.width;
            let cell_h = self.atlas.cell_size.height;
            let surface_w = self.surface_config.width as f32;
            let surface_h = self.surface_config.height as f32;
            let grid_offset_px = (vp_x as f32, vp_y as f32);

            self.image_renderer.render_placements(
                &self.queue,
                &mut encoder,
                view,
                terminal.image_store.placements(),
                cell_w,
                cell_h,
                surface_w,
                surface_h,
                grid_offset_px,
                Some(viewport),
                terminal.scroll_offset(),
            );
        }

        // Submit immediately so this pane's data is flushed before the next pane writes.
        self.queue.submit(std::iter::once(encoder.finish()));

        // Render preedit overlay if present.
        if let Some(text) = preedit {
            let (vp_x, vp_y, vp_w, vp_h) = viewport;
            self.render_preedit_overlay(terminal, text, Some((vp_x, vp_y, vp_w, vp_h)), view);
        }

        Ok(())
    }

    /// Render a terminal grid into a specific viewport region of the surface.
    ///
    /// `viewport` is in physical pixels: `(x, y, width, height)`.
    /// `selection` is optional selection bounds.
    /// If `viewport` is `None`, render to the full surface (current behavior).
    pub fn render_viewport(
        &mut self,
        terminal: &termojinal_vt::Terminal,
        selection: Option<((usize, usize), (usize, usize))>,
        viewport: Option<(u32, u32, u32, u32)>,
        encoder: &mut wgpu::CommandEncoder,
        view: &wgpu::TextureView,
    ) -> Result<(), RenderError> {
        // Build/update instance data (with dirty-row optimization).
        let instance_count = self.update_instances(terminal, selection, None, None, None);

        // Re-upload atlas texture if new glyphs were rasterized.
        let current_glyph_count = self.atlas.glyph_count();
        if current_glyph_count != self.atlas_texture_version {
            self.reupload_atlas();
            self.atlas_texture_version = current_glyph_count;
        }

        // Re-upload emoji atlas if needed.
        let current_emoji_count = self.emoji_atlas.glyph_count();
        if current_emoji_count != self.emoji_texture_version {
            self.reupload_emoji_atlas();
            self.emoji_texture_version = current_emoji_count;
        }

        // Compute uniforms based on viewport.
        let uniforms = match viewport {
            Some((vp_x, vp_y, vp_w, vp_h)) => {
                self.compute_uniforms_viewport(terminal, vp_x, vp_y, vp_w, vp_h)
            }
            None => self.compute_uniforms_full(terminal),
        };

        self.queue
            .write_buffer(&self.uniform_buffer, 0, bytemuck::bytes_of(&uniforms));

        // Encode render pass.
        {
            let mut render_pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("cell render pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: if viewport.is_some() {
                            // When rendering into a viewport, don't clear the
                            // entire surface — just load existing content.
                            wgpu::LoadOp::Load
                        } else {
                            wgpu::LoadOp::Clear(wgpu::Color {
                                r: 0.0,
                                g: 0.0,
                                b: 0.0,
                                a: 0.0,
                            })
                        },
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
            });

            // Apply scissor rect if viewport is specified.
            if let Some((vp_x, vp_y, vp_w, vp_h)) = viewport {
                render_pass.set_scissor_rect(vp_x, vp_y, vp_w, vp_h);
            }

            render_pass.set_pipeline(&self.render_pipeline);
            render_pass.set_bind_group(0, &self.bind_group, &[]);
            render_pass.set_vertex_buffer(0, self.instance_buffer.slice(..));
            render_pass.draw(0..6, 0..instance_count as u32);
        }

        // Render inline images on top of cells if any placements exist.
        if terminal.image_store.has_placements() {
            let cell_w = self.atlas.cell_size.width;
            let cell_h = self.atlas.cell_size.height;
            let surface_w = self.surface_config.width as f32;
            let surface_h = self.surface_config.height as f32;

            // Grid offset in pixels (matches the uniform computation).
            let grid_offset_px = match viewport {
                Some((vp_x, vp_y, _vp_w, _vp_h)) => (vp_x as f32, vp_y as f32),
                None => (cell_w, cell_h * 0.5),
            };

            self.image_renderer.render_placements(
                &self.queue,
                encoder,
                view,
                terminal.image_store.placements(),
                cell_w,
                cell_h,
                surface_w,
                surface_h,
                grid_offset_px,
                viewport,
                terminal.scroll_offset(),
            );
        }

        Ok(())
    }
}
