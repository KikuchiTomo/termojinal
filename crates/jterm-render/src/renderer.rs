//! wgpu-based GPU terminal renderer.
//!
//! Sets up the wgpu render pipeline and renders terminal cells as textured quads
//! using instanced rendering for efficiency.

use std::sync::Arc;

use crate::atlas::{Atlas, CellSize, FontConfig};
use crate::color_convert;

/// Per-cell instance data sent to the GPU.
///
/// Each cell is one instance; the vertex shader generates a quad from 6 vertices.
#[repr(C)]
#[derive(Clone, Copy, Debug, bytemuck::Pod, bytemuck::Zeroable)]
struct CellInstance {
    /// Grid position (column, row).
    grid_pos: [f32; 2],
    /// Atlas UV region: (x, y, w, h) in texels.
    atlas_uv: [f32; 4],
    /// Foreground color RGBA.
    fg_color: [f32; 4],
    /// Background color RGBA.
    bg_color: [f32; 4],
    /// Attribute flags (matches jterm_vt::Attrs bits).
    flags: u32,
    /// Padding to align to 16 bytes.
    _pad: [u32; 3],
}

/// Uniform data for the shader.
#[repr(C)]
#[derive(Clone, Copy, Debug, bytemuck::Pod, bytemuck::Zeroable)]
struct Uniforms {
    /// Cell size in NDC: (width, height).
    cell_size: [f32; 2],
    /// Grid offset in NDC (top-left corner).
    grid_offset: [f32; 2],
    /// Atlas texture size: (width, height).
    atlas_size: [f32; 2],
    /// Padding.
    _pad0: [f32; 2],
    /// cursor_pos: (col, row, cursor_color_r, cursor_color_g)
    cursor_pos: [f32; 4],
    /// cursor_extra: (cursor_color_b, cursor_shape, blink_on, _pad)
    cursor_extra: [f32; 4],
}

/// Flag indicating this cell is the cursor cell.
const FLAG_IS_CURSOR: u32 = 0x10000;

/// Flag indicating this cell is selected (for selection highlighting).
const FLAG_SELECTED: u32 = 0x20000;

/// Errors from the renderer.
#[derive(Debug, thiserror::Error)]
pub enum RenderError {
    #[error("failed to get adapter")]
    AdapterNotFound,

    #[error("failed to request device: {0}")]
    DeviceRequest(#[from] wgpu::RequestDeviceError),

    #[error("surface error: {0}")]
    Surface(#[from] wgpu::SurfaceError),

    #[error("atlas error: {0}")]
    Atlas(#[from] crate::atlas::AtlasError),

    #[error("create surface error: {0}")]
    CreateSurface(#[from] wgpu::CreateSurfaceError),
}

/// The GPU renderer for the terminal.
pub struct Renderer {
    device: wgpu::Device,
    queue: wgpu::Queue,
    surface: wgpu::Surface<'static>,
    surface_config: wgpu::SurfaceConfiguration,
    render_pipeline: wgpu::RenderPipeline,
    bind_group_layout: wgpu::BindGroupLayout,
    bind_group: wgpu::BindGroup,
    uniform_buffer: wgpu::Buffer,
    instance_buffer: wgpu::Buffer,
    instance_capacity: usize,
    atlas: Atlas,
    atlas_texture: wgpu::Texture,
    atlas_texture_version: usize,
    /// Whether the cursor blink is in the "on" state.
    pub cursor_blink_on: bool,

    // --- Dirty rendering state ---
    /// Cached instance data from the last full rebuild.
    cached_instances: Vec<CellInstance>,
    /// Number of instances last uploaded to the GPU (including scrollbar, etc.).
    cached_instance_count: usize,
    /// The scroll offset that was active when the cache was built.
    cached_scroll_offset: usize,
    /// The grid dimensions (cols, rows) when the cache was built.
    cached_grid_dims: (usize, usize),
    /// The selection bounds when the cache was built.
    cached_selection: Option<((usize, usize), (usize, usize))>,
    /// The number of instances per row (varies because continuation cells are skipped).
    /// Length = grid rows. Each entry is the count of CellInstance for that row.
    cached_row_instance_counts: Vec<usize>,
}

impl Renderer {
    /// Create a new renderer for the given window.
    ///
    /// Takes `Arc<Window>` because the wgpu surface requires `'static` lifetime.
    pub async fn new(
        window: Arc<winit::window::Window>,
        font_config: &FontConfig,
    ) -> Result<Self, RenderError> {
        // Create wgpu instance.
        let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor {
            backends: wgpu::Backends::METAL,
            ..Default::default()
        });

        // Create surface. Arc<Window> is 'static so the surface can own it.
        let surface = instance.create_surface(window.clone())?;

        // Request adapter.
        let adapter = instance
            .request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::LowPower,
                compatible_surface: Some(&surface),
                force_fallback_adapter: false,
            })
            .await
            .ok_or(RenderError::AdapterNotFound)?;

        // Request device.
        let (device, queue) = adapter
            .request_device(
                &wgpu::DeviceDescriptor {
                    label: Some("jterm device"),
                    ..Default::default()
                },
                None,
            )
            .await?;

        // Configure surface.
        let size = window.inner_size();
        let caps = surface.get_capabilities(&adapter);
        let surface_format = caps.formats.iter().find(|f| !f.is_srgb()).copied()
            .unwrap_or(caps.formats[0]);

        let surface_config = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format: surface_format,
            width: size.width.max(1),
            height: size.height.max(1),
            present_mode: wgpu::PresentMode::Fifo,
            desired_maximum_frame_latency: 2,
            alpha_mode: wgpu::CompositeAlphaMode::Auto,
            view_formats: vec![],
        };
        surface.configure(&device, &surface_config);

        // Build font atlas.
        let atlas = Atlas::new(font_config)?;

        // Create atlas texture.
        let atlas_texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("atlas"),
            size: wgpu::Extent3d {
                width: atlas.width,
                height: atlas.height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::R8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });

        // Upload atlas data.
        queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &atlas_texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            &atlas.data,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(atlas.width),
                rows_per_image: Some(atlas.height),
            },
            wgpu::Extent3d {
                width: atlas.width,
                height: atlas.height,
                depth_or_array_layers: 1,
            },
        );

        let atlas_view = atlas_texture.create_view(&wgpu::TextureViewDescriptor::default());

        let atlas_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("atlas sampler"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });

        // Uniform buffer.
        let uniform_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("uniforms"),
            size: std::mem::size_of::<Uniforms>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        // Instance buffer — start with space for 80x24 cells.
        let initial_capacity = 80 * 24;
        let instance_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("cell instances"),
            size: (initial_capacity * std::mem::size_of::<CellInstance>()) as u64,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        // Bind group layout.
        let bind_group_layout =
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("bind group layout"),
                entries: &[
                    wgpu::BindGroupLayoutEntry {
                        binding: 0,
                        visibility: wgpu::ShaderStages::VERTEX | wgpu::ShaderStages::FRAGMENT,
                        ty: wgpu::BindingType::Buffer {
                            ty: wgpu::BufferBindingType::Uniform,
                            has_dynamic_offset: false,
                            min_binding_size: None,
                        },
                        count: None,
                    },
                    wgpu::BindGroupLayoutEntry {
                        binding: 1,
                        visibility: wgpu::ShaderStages::FRAGMENT,
                        ty: wgpu::BindingType::Texture {
                            sample_type: wgpu::TextureSampleType::Float { filterable: true },
                            view_dimension: wgpu::TextureViewDimension::D2,
                            multisampled: false,
                        },
                        count: None,
                    },
                    wgpu::BindGroupLayoutEntry {
                        binding: 2,
                        visibility: wgpu::ShaderStages::FRAGMENT,
                        ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                        count: None,
                    },
                ],
            });

        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("bind group"),
            layout: &bind_group_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: uniform_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(&atlas_view),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::Sampler(&atlas_sampler),
                },
            ],
        });

        // Shader module.
        let shader_source = include_str!("shader.wgsl");
        let shader_module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("cell shader"),
            source: wgpu::ShaderSource::Wgsl(shader_source.into()),
        });

        // Pipeline layout.
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("pipeline layout"),
            bind_group_layouts: &[&bind_group_layout],
            push_constant_ranges: &[],
        });

        // Instance buffer layout.
        let instance_layout = wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<CellInstance>() as u64,
            step_mode: wgpu::VertexStepMode::Instance,
            attributes: &[
                // grid_pos: vec2<f32> at location(0)
                wgpu::VertexAttribute {
                    format: wgpu::VertexFormat::Float32x2,
                    offset: 0,
                    shader_location: 0,
                },
                // atlas_uv: vec4<f32> at location(1)
                wgpu::VertexAttribute {
                    format: wgpu::VertexFormat::Float32x4,
                    offset: 8,
                    shader_location: 1,
                },
                // fg_color: vec4<f32> at location(2)
                wgpu::VertexAttribute {
                    format: wgpu::VertexFormat::Float32x4,
                    offset: 24,
                    shader_location: 2,
                },
                // bg_color: vec4<f32> at location(3)
                wgpu::VertexAttribute {
                    format: wgpu::VertexFormat::Float32x4,
                    offset: 40,
                    shader_location: 3,
                },
                // flags: u32 at location(4)
                wgpu::VertexAttribute {
                    format: wgpu::VertexFormat::Uint32,
                    offset: 56,
                    shader_location: 4,
                },
            ],
        };

        // Render pipeline.
        let render_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("cell pipeline"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader_module,
                entry_point: Some("vs_main"),
                compilation_options: Default::default(),
                buffers: &[instance_layout],
            },
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                strip_index_format: None,
                front_face: wgpu::FrontFace::Ccw,
                cull_mode: None,
                unclipped_depth: false,
                polygon_mode: wgpu::PolygonMode::Fill,
                conservative: false,
            },
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            fragment: Some(wgpu::FragmentState {
                module: &shader_module,
                entry_point: Some("fs_main"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: surface_format,
                    blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            multiview: None,
            cache: None,
        });

        let atlas_glyph_count = atlas.glyph_count();

        Ok(Self {
            device,
            queue,
            surface,
            surface_config,
            render_pipeline,
            bind_group_layout,
            bind_group,
            uniform_buffer,
            instance_buffer,
            instance_capacity: initial_capacity,
            atlas,
            atlas_texture,
            atlas_texture_version: atlas_glyph_count,
            cursor_blink_on: true,
            cached_instances: Vec::new(),
            cached_instance_count: 0,
            cached_scroll_offset: usize::MAX, // Force first rebuild
            cached_grid_dims: (0, 0),
            cached_selection: None,
            cached_row_instance_counts: Vec::new(),
        })
    }

    // -----------------------------------------------------------------------
    // Instance building helpers
    // -----------------------------------------------------------------------

    /// Build instance data for a single row of the terminal grid.
    fn build_row_instances(
        &mut self,
        terminal: &jterm_vt::Terminal,
        row: usize,
        selection: Option<((usize, usize), (usize, usize))>,
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
                    _ => jterm_vt::Cell::default(),
                }
            } else {
                *grid.cell(col, grid_row)
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

            // Get glyph info (rasterize on demand if needed).
            let glyph = self.atlas.get_glyph(c);

            let fg = color_convert::color_to_rgba(cell.fg, true);
            let bg = color_convert::color_to_rgba(cell.bg, false);

            let mut flags = cell.attrs.bits() as u32;

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

            row_instances.push(CellInstance {
                grid_pos: [col as f32, row as f32],
                atlas_uv: [glyph.atlas_x, glyph.atlas_y, glyph.atlas_w, glyph.atlas_h],
                fg_color: fg,
                bg_color: bg,
                flags,
                _pad: [0; 3],
            });
        }
        row_instances
    }

    /// Build scrollbar instances. Returns the instances to append after all rows.
    fn build_scrollbar_instances(
        &mut self,
        terminal: &jterm_vt::Terminal,
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
        let thumb_height_f =
            (rows as f64 / total_lines as f64 * rows as f64).max(1.0);
        let thumb_top_f =
            (scrollback_len - scroll_offset) as f64 / total_lines as f64 * rows as f64;

        let thumb_top = thumb_top_f.floor() as usize;
        let thumb_bottom =
            ((thumb_top_f + thumb_height_f).ceil() as usize).min(rows);

        let scrollbar_x = cols as f32 + 0.8;

        let mut instances = Vec::with_capacity(rows);
        for r in 0..rows {
            let is_thumb = r >= thumb_top && r < thumb_bottom;
            let bg = if is_thumb {
                [1.0_f32, 1.0, 1.0, 0.5]
            } else {
                [1.0_f32, 1.0, 1.0, 0.1]
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
                _pad: [0; 3],
            });
        }
        instances
    }

    /// Build or incrementally update instance data. Returns the total instance count.
    ///
    /// When the grid reports dirty rows and the grid dimensions/scroll offset
    /// haven't changed, only the dirty rows are rebuilt and patched into the
    /// GPU buffer, avoiding a full re-upload.
    fn update_instances(
        &mut self,
        terminal: &jterm_vt::Terminal,
        selection: Option<((usize, usize), (usize, usize))>,
    ) -> usize {
        let grid = terminal.grid();
        let cols = grid.cols();
        let rows = grid.rows();
        let scroll_offset = terminal.scroll_offset();

        let dims_match = self.cached_grid_dims == (cols, rows);
        let scroll_match = self.cached_scroll_offset == scroll_offset;
        let selection_match = self.cached_selection == selection;

        // Determine if we can do an incremental update:
        // - Grid dimensions must match the cache
        // - Scroll offset must match
        // - Selection must match (selection changes can affect any row's flags)
        // - The grid must report some dirty rows (otherwise nothing to do)
        let can_incremental = dims_match
            && scroll_match
            && selection_match
            && !self.cached_instances.is_empty()
            && grid.any_dirty();

        if can_incremental {
            // Incremental: only rebuild dirty rows and patch into the buffer.
            let instance_stride = std::mem::size_of::<CellInstance>();

            for row in 0..rows {
                // For rows sourced from scrollback, we always rebuild if dirty
                // since scrollback content could have changed.
                let is_grid_row = scroll_offset == 0 || row >= scroll_offset;
                let row_dirty = if is_grid_row && scroll_offset == 0 {
                    grid.is_row_dirty(row)
                } else {
                    // Scrollback rows or shifted rows: always rebuild for safety.
                    // (In practice, if scroll_match is true, scrollback rows
                    // haven't changed, but the grid rows that map to grid_row
                    // after scroll offset may still be dirty.)
                    if scroll_offset > 0 && row >= scroll_offset {
                        let grid_row = row - scroll_offset;
                        grid.is_row_dirty(grid_row)
                    } else {
                        false
                    }
                };

                // Also check if this is the cursor row (cursor may have moved).
                let is_cursor_row = scroll_offset == 0 && row == terminal.cursor_row;

                if !row_dirty && !is_cursor_row {
                    continue;
                }

                let new_row_instances = self.build_row_instances(terminal, row, selection);

                // Calculate the byte offset into the cached instances for this row.
                let row_start_instance: usize = self.cached_row_instance_counts[..row]
                    .iter()
                    .sum();
                let old_count = self.cached_row_instance_counts[row];

                if new_row_instances.len() == old_count {
                    // Same number of instances: patch in place.
                    for (i, inst) in new_row_instances.iter().enumerate() {
                        self.cached_instances[row_start_instance + i] = *inst;
                    }
                    let byte_offset = (row_start_instance * instance_stride) as u64;
                    self.queue.write_buffer(
                        &self.instance_buffer,
                        byte_offset,
                        bytemuck::cast_slice(&new_row_instances),
                    );
                } else {
                    // Row instance count changed (e.g., wide chars appeared/disappeared).
                    // Fall back to full rebuild.
                    return self.full_rebuild(terminal, selection);
                }
            }

            // Also rebuild scrollbar if present (it depends on scroll_offset which
            // hasn't changed here, so it's the same — skip).

            grid.clear_dirty();
            return self.cached_instance_count;
        }

        // Full rebuild needed.
        if !grid.any_dirty()
            && dims_match
            && scroll_match
            && selection_match
            && !self.cached_instances.is_empty()
        {
            // Nothing changed at all — skip rebuild entirely.
            return self.cached_instance_count;
        }

        self.full_rebuild(terminal, selection)
    }

    /// Perform a full rebuild of all instance data.
    fn full_rebuild(
        &mut self,
        terminal: &jterm_vt::Terminal,
        selection: Option<((usize, usize), (usize, usize))>,
    ) -> usize {
        let grid = terminal.grid();
        let cols = grid.cols();
        let rows = grid.rows();
        let scroll_offset = terminal.scroll_offset();
        let total_cells = cols * rows;

        let mut instances = Vec::with_capacity(total_cells);
        let mut row_counts = Vec::with_capacity(rows);

        for row in 0..rows {
            let row_instances = self.build_row_instances(terminal, row, selection);
            row_counts.push(row_instances.len());
            instances.extend_from_slice(&row_instances);
        }

        // Add scrollbar instances.
        let scrollbar_instances = self.build_scrollbar_instances(terminal);
        instances.extend_from_slice(&scrollbar_instances);

        // Ensure instance buffer is large enough.
        if instances.len() > self.instance_capacity {
            self.instance_capacity = instances.len().next_power_of_two();
            self.instance_buffer = self.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("cell instances"),
                size: (self.instance_capacity * std::mem::size_of::<CellInstance>()) as u64,
                usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
        }

        // Upload all instance data.
        if !instances.is_empty() {
            self.queue.write_buffer(
                &self.instance_buffer,
                0,
                bytemuck::cast_slice(&instances),
            );
        }

        let count = instances.len();

        // Update cache.
        self.cached_instances = instances;
        self.cached_instance_count = count;
        self.cached_scroll_offset = scroll_offset;
        self.cached_grid_dims = (cols, rows);
        self.cached_selection = selection;
        self.cached_row_instance_counts = row_counts;

        grid.clear_dirty();
        count
    }

    // -----------------------------------------------------------------------
    // Uniform helpers
    // -----------------------------------------------------------------------

    /// Compute uniforms for full-surface rendering.
    fn compute_uniforms_full(&self, terminal: &jterm_vt::Terminal) -> Uniforms {
        let cell_w = self.atlas.cell_size.width;
        let cell_h = self.atlas.cell_size.height;
        let surface_w = self.surface_config.width as f32;
        let surface_h = self.surface_config.height as f32;

        let cell_ndc_w = (cell_w / surface_w) * 2.0;
        let cell_ndc_h = (cell_h / surface_h) * 2.0;

        let grid_offset_x = -1.0 + (cell_w / surface_w) * 2.0;
        let grid_offset_y = 1.0 - (cell_h * 0.5 / surface_h) * 2.0;

        self.build_uniforms(terminal, cell_ndc_w, cell_ndc_h, grid_offset_x, grid_offset_y)
    }

    /// Compute uniforms for viewport rendering.
    fn compute_uniforms_viewport(
        &self,
        terminal: &jterm_vt::Terminal,
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

        self.build_uniforms(terminal, cell_ndc_w, cell_ndc_h, grid_offset_x, grid_offset_y)
    }

    /// Build a Uniforms struct with the given transform parameters.
    fn build_uniforms(
        &self,
        terminal: &jterm_vt::Terminal,
        cell_ndc_w: f32,
        cell_ndc_h: f32,
        grid_offset_x: f32,
        grid_offset_y: f32,
    ) -> Uniforms {
        let cursor_shape = match terminal.cursor_shape {
            jterm_vt::CursorShape::Block | jterm_vt::CursorShape::BlinkingBlock => 0.0,
            jterm_vt::CursorShape::Underline | jterm_vt::CursorShape::BlinkingUnderline => 1.0,
            jterm_vt::CursorShape::Bar | jterm_vt::CursorShape::BlinkingBar => 2.0,
        };

        Uniforms {
            cell_size: [cell_ndc_w, cell_ndc_h],
            grid_offset: [grid_offset_x, grid_offset_y],
            atlas_size: [self.atlas.width as f32, self.atlas.height as f32],
            _pad0: [0.0; 2],
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
        terminal: &jterm_vt::Terminal,
        selection: Option<((usize, usize), (usize, usize))>,
    ) -> Result<(), RenderError> {
        let (output, mut encoder) = self.begin_frame()?;
        let view = output
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());

        self.render_viewport(terminal, selection, None, &mut encoder, &view)?;

        self.end_frame(output, encoder);
        Ok(())
    }

    /// Begin a frame: get the surface texture and create a command encoder.
    pub fn begin_frame(&mut self) -> Result<(wgpu::SurfaceTexture, wgpu::CommandEncoder), RenderError> {
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

    /// Render a terminal grid into a specific viewport region of the surface.
    ///
    /// `viewport` is in physical pixels: `(x, y, width, height)`.
    /// `selection` is optional selection bounds.
    /// If `viewport` is `None`, render to the full surface (current behavior).
    pub fn render_viewport(
        &mut self,
        terminal: &jterm_vt::Terminal,
        selection: Option<((usize, usize), (usize, usize))>,
        viewport: Option<(u32, u32, u32, u32)>,
        encoder: &mut wgpu::CommandEncoder,
        view: &wgpu::TextureView,
    ) -> Result<(), RenderError> {
        // Build/update instance data (with dirty-row optimization).
        let instance_count = self.update_instances(terminal, selection);

        // Re-upload atlas texture if new glyphs were rasterized.
        let current_glyph_count = self.atlas.glyph_count();
        if current_glyph_count != self.atlas_texture_version {
            self.reupload_atlas();
            self.atlas_texture_version = current_glyph_count;
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
                                r: color_convert::DEFAULT_BG[0] as f64,
                                g: color_convert::DEFAULT_BG[1] as f64,
                                b: color_convert::DEFAULT_BG[2] as f64,
                                a: 1.0,
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

        Ok(())
    }

    /// Draw a 1px separator line at the given position.
    ///
    /// The separator is rendered as one or more background-only quads
    /// positioned at the given pixel coordinates.
    pub fn draw_separator(
        &mut self,
        encoder: &mut wgpu::CommandEncoder,
        view: &wgpu::TextureView,
        x: u32,
        y: u32,
        width: u32,
        height: u32,
        color: [f32; 4],
    ) {
        let cell_w = self.atlas.cell_size.width;
        let cell_h = self.atlas.cell_size.height;
        let surface_w = self.surface_config.width as f32;
        let surface_h = self.surface_config.height as f32;

        // How many cell-sized quads do we need to cover the separator?
        let cells_needed_x = ((width as f32) / cell_w).ceil() as usize;
        let cells_needed_y = ((height as f32) / cell_h).ceil() as usize;
        let num_quads = cells_needed_x.max(1) * cells_needed_y.max(1);

        let space_glyph = self.atlas.get_glyph(' ');

        // Build separator instances. We position them using NDC coordinates
        // directly by computing grid_pos such that grid_pos * cell_size + offset
        // places them at the right pixel location.
        //
        // We'll set up a custom uniform for this pass where grid_offset is
        // at the separator origin.
        let sep_ndc_x = (x as f32 / surface_w) * 2.0 - 1.0;
        let sep_ndc_y = 1.0 - (y as f32 / surface_h) * 2.0;

        let cell_ndc_w = (cell_w / surface_w) * 2.0;
        let cell_ndc_h = (cell_h / surface_h) * 2.0;

        let mut instances = Vec::with_capacity(num_quads);
        for iy in 0..cells_needed_y.max(1) {
            for ix in 0..cells_needed_x.max(1) {
                instances.push(CellInstance {
                    grid_pos: [ix as f32, iy as f32],
                    atlas_uv: [
                        space_glyph.atlas_x,
                        space_glyph.atlas_y,
                        space_glyph.atlas_w,
                        space_glyph.atlas_h,
                    ],
                    fg_color: color,
                    bg_color: color,
                    flags: 0,
                    _pad: [0; 3],
                });
            }
        }

        // We need to temporarily set uniforms for the separator.
        let sep_uniforms = Uniforms {
            cell_size: [cell_ndc_w, cell_ndc_h],
            grid_offset: [sep_ndc_x, sep_ndc_y],
            atlas_size: [self.atlas.width as f32, self.atlas.height as f32],
            _pad0: [0.0; 2],
            cursor_pos: [0.0; 4],
            cursor_extra: [0.0; 4],
        };

        self.queue
            .write_buffer(&self.uniform_buffer, 0, bytemuck::bytes_of(&sep_uniforms));

        // Ensure instance buffer is large enough for the separator quads.
        // (We temporarily use the instance buffer; in production, a separate
        // buffer would be cleaner, but this works for the separator use case.)
        if instances.len() > self.instance_capacity {
            self.instance_capacity = instances.len().next_power_of_two();
            self.instance_buffer = self.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("cell instances"),
                size: (self.instance_capacity * std::mem::size_of::<CellInstance>()) as u64,
                usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
        }

        self.queue.write_buffer(
            &self.instance_buffer,
            0,
            bytemuck::cast_slice(&instances),
        );

        {
            let mut render_pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("separator render pass"),
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

            // Clip to the separator region.
            render_pass.set_scissor_rect(x, y, width, height);
            render_pass.set_pipeline(&self.render_pipeline);
            render_pass.set_bind_group(0, &self.bind_group, &[]);
            render_pass.set_vertex_buffer(0, self.instance_buffer.slice(..));
            render_pass.draw(0..6, 0..instances.len() as u32);
        }

        // Invalidate the cached instances since we overwrote the instance buffer
        // with separator data.
        self.cached_instance_count = 0;
        self.cached_instances.clear();
    }

    /// Handle a window resize.
    pub fn resize(&mut self, width: u32, height: u32) {
        if width == 0 || height == 0 {
            return;
        }
        self.surface_config.width = width;
        self.surface_config.height = height;
        self.surface.configure(&self.device, &self.surface_config);
        // Invalidate cache on resize since surface dimensions changed.
        self.cached_instance_count = 0;
        self.cached_instances.clear();
    }

    /// Get the cell size in pixels.
    pub fn cell_size(&self) -> CellSize {
        self.atlas.cell_size
    }

    /// Calculate grid dimensions with padding (for single-pane / full-surface).
    pub fn grid_size(&self, width: u32, height: u32) -> (u16, u16) {
        let cw = self.atlas.cell_size.width;
        let ch = self.atlas.cell_size.height;
        let usable_w = (width as f32) - 2.0 * cw;
        let usable_h = (height as f32) - ch;
        let cols = (usable_w / cw).floor().max(1.0) as u16;
        let rows = (usable_h / ch).floor().max(1.0) as u16;
        (cols, rows)
    }

    /// Calculate grid dimensions without padding (for multi-pane viewports).
    pub fn grid_size_raw(&self, width: u32, height: u32) -> (u16, u16) {
        let cw = self.atlas.cell_size.width;
        let ch = self.atlas.cell_size.height;
        let cols = (width as f32 / cw).floor().max(1.0) as u16;
        let rows = (height as f32 / ch).floor().max(1.0) as u16;
        (cols, rows)
    }

    /// Re-upload the atlas texture to the GPU (e.g., after new glyphs are rasterized).
    fn reupload_atlas(&mut self) {
        // Check if atlas size changed (it may have grown).
        let needs_recreate = self.atlas.width != self.atlas_texture.width()
            || self.atlas.height != self.atlas_texture.height();

        if needs_recreate {
            self.atlas_texture = self.device.create_texture(&wgpu::TextureDescriptor {
                label: Some("atlas"),
                size: wgpu::Extent3d {
                    width: self.atlas.width,
                    height: self.atlas.height,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::R8Unorm,
                usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
                view_formats: &[],
            });

            // Recreate bind group with new texture view.
            let atlas_view = self
                .atlas_texture
                .create_view(&wgpu::TextureViewDescriptor::default());
            let atlas_sampler = self.device.create_sampler(&wgpu::SamplerDescriptor {
                label: Some("atlas sampler"),
                mag_filter: wgpu::FilterMode::Linear,
                min_filter: wgpu::FilterMode::Linear,
                ..Default::default()
            });

            self.bind_group = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("bind group"),
                layout: &self.bind_group_layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: self.uniform_buffer.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::TextureView(&atlas_view),
                    },
                    wgpu::BindGroupEntry {
                        binding: 2,
                        resource: wgpu::BindingResource::Sampler(&atlas_sampler),
                    },
                ],
            });
        }

        // Upload atlas data.
        self.queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &self.atlas_texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            &self.atlas.data,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(self.atlas.width),
                rows_per_image: Some(self.atlas.height),
            },
            wgpu::Extent3d {
                width: self.atlas.width,
                height: self.atlas.height,
                depth_or_array_layers: 1,
            },
        );
    }
}
