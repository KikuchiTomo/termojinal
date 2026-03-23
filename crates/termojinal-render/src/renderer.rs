//! wgpu-based GPU terminal renderer.
//!
//! Sets up the wgpu render pipeline and renders terminal cells as textured quads
//! using instanced rendering for efficiency.

use std::collections::HashMap;
use std::sync::Arc;

use crate::atlas::{Atlas, CellSize, FontConfig};
use crate::blur_renderer::BlurRenderer;
use crate::color_convert::{self, ThemePalette};
use crate::emoji_atlas::{self, EmojiAtlas};
use crate::image_render::ImageRenderer;
use crate::rounded_rect_renderer::{RoundedRect, RoundedRectRenderer};

/// Per-pane dirty rendering cache. Keyed by an opaque pane identifier.
type PaneKey = u64;

#[derive(Default)]
struct PaneCache {
    instances: Vec<CellInstance>,
    instance_count: usize,
    scroll_offset: usize,
    grid_dims: (usize, usize),
    selection: Option<((usize, usize), (usize, usize))>,
    row_instance_counts: Vec<usize>,
}

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
    /// Attribute flags (matches termojinal_vt::Attrs bits).
    flags: u32,
    /// Cell width multiplier (1.0 for normal, 2.0 for wide CJK chars).
    cell_width_scale: f32,
    /// Padding.
    _pad: [u32; 2],
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
    /// Emoji atlas texture size: (width, height).
    emoji_atlas_size: [f32; 2],
    /// cursor_pos: (col, row, cursor_color_r, cursor_color_g)
    cursor_pos: [f32; 4],
    /// cursor_extra: (cursor_color_b, cursor_shape, blink_on, _pad)
    cursor_extra: [f32; 4],
}

/// Flag indicating this cell has an underline (matches Attrs::UNDERLINE bit).
const FLAG_UNDERLINE: u32 = 1 << 3;

/// Flag indicating this cell is the cursor cell.
const FLAG_IS_CURSOR: u32 = 0x10000;

/// Flag indicating this cell is selected (for selection highlighting).
const FLAG_SELECTED: u32 = 0x20000;

/// Flag indicating this cell contains an emoji rendered via the color emoji atlas.
const FLAG_EMOJI: u32 = 0x40000;

/// Flag indicating this cell is a search match highlight.
const FLAG_SEARCH: u32 = 0x80000;

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
    adapter: wgpu::Adapter,
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
    /// Retained font config (logical sizes) for rebuilding atlas on font size / DPI change.
    font_config: FontConfig,
    /// Display scale factor (e.g. 2.0 for Retina, 1.0 for FHD).
    pub scale_factor: f32,
    /// Color emoji atlas (RGBA).
    emoji_atlas: EmojiAtlas,
    emoji_texture: wgpu::Texture,
    emoji_texture_version: usize,
    /// Whether the cursor blink is in the "on" state.
    pub cursor_blink_on: bool,
    /// Background opacity (0.0 = fully transparent, 1.0 = opaque).
    pub bg_opacity: f32,
    /// Terminal default background color (from theme config, replaces DEFAULT_BG).
    pub default_bg: [f32; 4],
    /// IME preedit background color.
    pub preedit_bg: [f32; 4],
    /// Scrollbar thumb opacity.
    pub scrollbar_thumb_opacity: f32,
    /// Scrollbar track opacity.
    pub scrollbar_track_opacity: f32,
    /// Theme palette for ANSI 16-color overrides and default fg/bg.
    pub theme_palette: ThemePalette,

    // --- Dirty rendering: per-pane cache ---
    pane_caches: HashMap<PaneKey, PaneCache>,
    /// The pane key currently active (for single-pane render() calls).
    current_pane_key: PaneKey,
    /// Image renderer for inline terminal images (Kitty/iTerm2/Sixel).
    image_renderer: ImageRenderer,
    /// SDF-based rounded rectangle renderer for overlays (command palette, etc.).
    pub rounded_rect_renderer: RoundedRectRenderer,
    /// Two-pass Gaussian blur renderer for frosted-glass background effects.
    pub blur_renderer: BlurRenderer,
    /// The surface texture format (retained for recreating pipelines on format change).
    surface_format: wgpu::TextureFormat,
    /// Whether to use CJK-aware character width calculation.
    pub cjk_width: bool,
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
                    label: Some("termojinal device"),
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
            alpha_mode: {
                let caps = surface.get_capabilities(&adapter);
                if caps.alpha_modes.contains(&wgpu::CompositeAlphaMode::PostMultiplied) {
                    wgpu::CompositeAlphaMode::PostMultiplied
                } else if caps.alpha_modes.contains(&wgpu::CompositeAlphaMode::PreMultiplied) {
                    wgpu::CompositeAlphaMode::PreMultiplied
                } else {
                    wgpu::CompositeAlphaMode::Auto
                }
            },
            view_formats: vec![],
        };
        surface.configure(&device, &surface_config);

        // Build font atlas. Config font size is in logical points.
        // fontdue rasterizes in physical pixels, so scale by DPI factor.
        let scale = window.scale_factor() as f32;
        let scaled_config = FontConfig {
            size: font_config.size * scale,
            ..font_config.clone()
        };
        let atlas = Atlas::new(&scaled_config)?;

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

        // Build emoji atlas (use scaled font size to match atlas cell dimensions).
        let emoji_atlas = EmojiAtlas::new(
            atlas.cell_size.width as u32,
            atlas.cell_size.height as u32,
            scaled_config.size,
        );

        // Create emoji atlas texture (RGBA8).
        let emoji_texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("emoji atlas"),
            size: wgpu::Extent3d {
                width: emoji_atlas.width,
                height: emoji_atlas.height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8UnormSrgb,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });

        // Upload initial (empty) emoji atlas data.
        queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &emoji_texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            &emoji_atlas.data,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(emoji_atlas.width * 4),
                rows_per_image: Some(emoji_atlas.height),
            },
            wgpu::Extent3d {
                width: emoji_atlas.width,
                height: emoji_atlas.height,
                depth_or_array_layers: 1,
            },
        );

        let emoji_view = emoji_texture.create_view(&wgpu::TextureViewDescriptor::default());

        let emoji_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("emoji sampler"),
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

        // Bind group layout (5 entries: uniform, atlas texture, atlas sampler,
        // emoji texture, emoji sampler).
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
                    wgpu::BindGroupLayoutEntry {
                        binding: 3,
                        visibility: wgpu::ShaderStages::FRAGMENT,
                        ty: wgpu::BindingType::Texture {
                            sample_type: wgpu::TextureSampleType::Float { filterable: true },
                            view_dimension: wgpu::TextureViewDimension::D2,
                            multisampled: false,
                        },
                        count: None,
                    },
                    wgpu::BindGroupLayoutEntry {
                        binding: 4,
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
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: wgpu::BindingResource::TextureView(&emoji_view),
                },
                wgpu::BindGroupEntry {
                    binding: 4,
                    resource: wgpu::BindingResource::Sampler(&emoji_sampler),
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
                // cell_width_scale: f32 at location(5)
                wgpu::VertexAttribute {
                    format: wgpu::VertexFormat::Float32,
                    offset: 60,
                    shader_location: 5,
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

        // Create image renderer for inline terminal images.
        let image_renderer = ImageRenderer::new(&device, surface_format);

        // Create rounded rectangle renderer for overlay UI.
        let rounded_rect_renderer = RoundedRectRenderer::new(&device, surface_format);

        // Create blur renderer for frosted-glass effects.
        let blur_renderer = BlurRenderer::new(&device, surface_format);

        Ok(Self {
            adapter,
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
            font_config: font_config.clone(),
            scale_factor: scale,
            emoji_atlas,
            emoji_texture,
            emoji_texture_version: 0,
            cursor_blink_on: true,
            bg_opacity: 1.0,
            default_bg: color_convert::DEFAULT_BG,
            preedit_bg: [0.15, 0.15, 0.20, 1.0],
            scrollbar_thumb_opacity: 0.5,
            scrollbar_track_opacity: 0.1,
            theme_palette: ThemePalette::default(),
            pane_caches: HashMap::new(),
            current_pane_key: 0,
            image_renderer,
            rounded_rect_renderer,
            blur_renderer,
            surface_format,
            cjk_width: false,
        })
    }

    // -----------------------------------------------------------------------
    // Instance building helpers
    // -----------------------------------------------------------------------

    /// Build instance data for a single row of the terminal grid.
    fn build_row_instances(
        &mut self,
        terminal: &termojinal_vt::Terminal,
        row: usize,
        selection: Option<((usize, usize), (usize, usize))>,
        #[allow(unused_variables)]
        search_matches: Option<&[(usize, usize, usize)]>, // (row, col_start, col_end)
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

            // Check if this character is an emoji and get glyph from the
            // appropriate atlas.
            let (glyph, is_emoji_cell) = if emoji_atlas::is_emoji(c) {
                if let Some(eg) = self.emoji_atlas.get_glyph(c) {
                    (eg, true)
                } else {
                    (self.atlas.get_glyph(c), false)
                }
            } else {
                (self.atlas.get_glyph(c), false)
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
                for &(m_row, m_col_start, m_col_end) in matches {
                    if m_row == row && col >= m_col_start && col <= m_col_end {
                        flags |= FLAG_SEARCH;
                        break;
                    }
                }
            }

            let width_scale = if cell.width > 1 { cell.width as f32 } else { 1.0 };

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
    fn build_scrollbar_instances(
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
        let thumb_height_f =
            (rows as f64 / total_lines as f64 * rows as f64).max(1.0);
        let thumb_top_f =
            (scrollback_len - scroll_offset) as f64 / total_lines as f64 * rows as f64;

        let thumb_top = thumb_top_f.floor() as usize;
        let thumb_bottom =
            ((thumb_top_f + thumb_height_f).ceil() as usize).min(rows);

        // Place scrollbar at the right edge of the last column (inside clip area).
        let scrollbar_x = (cols as f32) - 0.2;

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

    /// Build or incrementally update instance data. Returns the total instance count.
    ///
    /// When the grid reports dirty rows and the grid dimensions/scroll offset
    /// haven't changed, only the dirty rows are rebuilt and patched into the
    /// GPU buffer, avoiding a full re-upload.
    /// Set which pane is being rendered (selects the per-pane cache).
    fn set_active_pane(&mut self, pane_key: PaneKey) {
        self.current_pane_key = pane_key;
    }

    fn update_instances(
        &mut self,
        terminal: &termojinal_vt::Terminal,
        selection: Option<((usize, usize), (usize, usize))>,
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

        let can_incremental = dims_match
            && scroll_match
            && selection_match
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

                let new_row_instances = self.build_row_instances(terminal, row, selection, None);

                let cache = &self.pane_caches[&key];
                let row_start_instance: usize =
                    cache.row_instance_counts[..row].iter().sum();
                let old_count = cache.row_instance_counts[row];

                if new_row_instances.len() == old_count {
                    let cache = self.pane_caches.get_mut(&key).unwrap();
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
                    return self.full_rebuild(terminal, selection);
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
            && !cache.instances.is_empty()
        {
            return cache.instance_count;
        }

        self.full_rebuild(terminal, selection)
    }

    /// Perform a full rebuild of all instance data for the current pane.
    fn full_rebuild(
        &mut self,
        terminal: &termojinal_vt::Terminal,
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
            let row_instances = self.build_row_instances(terminal, row, selection, None);
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
            self.queue.write_buffer(
                &self.instance_buffer,
                0,
                bytemuck::cast_slice(&instances),
            );
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

        grid.clear_dirty();
        count
    }

    // -----------------------------------------------------------------------
    // Uniform helpers
    // -----------------------------------------------------------------------

    /// Compute uniforms for full-surface rendering.
    fn compute_uniforms_full(&self, terminal: &termojinal_vt::Terminal) -> Uniforms {
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

        self.build_uniforms(terminal, cell_ndc_w, cell_ndc_h, grid_offset_x, grid_offset_y)
    }

    /// Build a Uniforms struct with the given transform parameters.
    fn build_uniforms(
        &self,
        terminal: &termojinal_vt::Terminal,
        cell_ndc_w: f32,
        cell_ndc_h: f32,
        grid_offset_x: f32,
        grid_offset_y: f32,
    ) -> Uniforms {
        let cursor_shape = match terminal.cursor_shape {
            termojinal_vt::CursorShape::Block | termojinal_vt::CursorShape::BlinkingBlock => 0.0,
            termojinal_vt::CursorShape::Underline | termojinal_vt::CursorShape::BlinkingUnderline => 1.0,
            termojinal_vt::CursorShape::Bar | termojinal_vt::CursorShape::BlinkingBar => 2.0,
        };

        Uniforms {
            cell_size: [cell_ndc_w, cell_ndc_h],
            grid_offset: [grid_offset_x, grid_offset_y],
            atlas_size: [self.atlas.width as f32, self.atlas.height as f32],
            emoji_atlas_size: [self.emoji_atlas.width as f32, self.emoji_atlas.height as f32],
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

    /// Clear the surface to the default background color. Submits immediately.
    pub fn clear_surface(&mut self, view: &wgpu::TextureView) {
        let mut encoder = self.device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
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
    ) -> Result<(), RenderError> {
        let (vp_x, vp_y, vp_w, vp_h) = viewport;

        // Sync image textures if the image store has been modified.
        if terminal.image_store.has_placements() {
            self.image_renderer
                .sync_images(&self.device, &self.queue, &terminal.image_store);
        }

        // Select the per-pane cache so dirty optimization works across panes.
        self.set_active_pane(pane_key);
        let instance_count = self.update_instances(terminal, selection);

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
        self.queue.write_buffer(&self.uniform_buffer, 0, bytemuck::bytes_of(&uniforms));

        // Create encoder and render pass.
        let mut encoder = self.device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
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
            );
        }

        // Submit immediately so this pane's data is flushed before the next pane writes.
        self.queue.submit(std::iter::once(encoder.finish()));

        // Render preedit overlay if present.
        if let Some(text) = preedit {
            let (vp_x, vp_y, vp_w, vp_h) = viewport;
            self.render_preedit_overlay(
                terminal,
                text,
                Some((vp_x, vp_y, vp_w, vp_h)),
                view,
            );
        }

        Ok(())
    }

    /// Render IME preedit (composition) text as underlined overlay cells at the
    /// terminal cursor position. Issues its own draw call with a separate
    /// encoder+submit so it works regardless of the main render path.
    fn render_preedit_overlay(
        &mut self,
        terminal: &termojinal_vt::Terminal,
        text: &str,
        viewport: Option<(u32, u32, u32, u32)>,
        view: &wgpu::TextureView,
    ) {
        if text.is_empty() {
            return;
        }

        let cursor_col = terminal.cursor_col;
        let cursor_row = terminal.cursor_row;

        let fg = self.theme_palette.fg;
        let bg = self.preedit_bg;

        let mut col_offset: usize = 0;
        let mut preedit_instances = Vec::new();

        for ch in text.chars() {
            let cw = termojinal_vt::char_width(ch, self.cjk_width);
            let glyph = self.atlas.get_glyph(ch);

            let width_scale = if cw > 1 { cw as f32 } else { 1.0 };
            preedit_instances.push(CellInstance {
                grid_pos: [(cursor_col + col_offset) as f32, cursor_row as f32],
                atlas_uv: [glyph.atlas_x, glyph.atlas_y, glyph.atlas_w, glyph.atlas_h],
                fg_color: fg,
                bg_color: bg,
                flags: FLAG_UNDERLINE,
                cell_width_scale: width_scale,
                _pad: [0; 2],
            });

            col_offset += cw;
        }

        if preedit_instances.is_empty() {
            return;
        }

        // Re-upload atlas if new glyphs were rasterized for preedit characters.
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

        let count = preedit_instances.len();

        // Ensure instance buffer is large enough.
        if count > self.instance_capacity {
            self.instance_capacity = count.next_power_of_two();
            self.instance_buffer = self.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("cell instances"),
                size: (self.instance_capacity * std::mem::size_of::<CellInstance>()) as u64,
                usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            // Invalidate pane caches since buffer was recreated.
            self.pane_caches.clear();
        }

        // Upload preedit instances.
        self.queue.write_buffer(
            &self.instance_buffer,
            0,
            bytemuck::cast_slice(&preedit_instances),
        );

        // Compute uniforms matching the current render mode.
        let uniforms = match viewport {
            Some((vp_x, vp_y, vp_w, vp_h)) => {
                self.compute_uniforms_viewport(terminal, vp_x, vp_y, vp_w, vp_h)
            }
            None => self.compute_uniforms_full(terminal),
        };
        self.queue
            .write_buffer(&self.uniform_buffer, 0, bytemuck::bytes_of(&uniforms));

        // Issue a draw call for the preedit instances.
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("preedit encoder"),
            });
        {
            let mut render_pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("preedit render pass"),
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
            if let Some((vp_x, vp_y, vp_w, vp_h)) = viewport {
                render_pass.set_scissor_rect(vp_x, vp_y, vp_w, vp_h);
            }
            render_pass.set_pipeline(&self.render_pipeline);
            render_pass.set_bind_group(0, &self.bind_group, &[]);
            render_pass.set_vertex_buffer(0, self.instance_buffer.slice(..));
            render_pass.draw(0..6, 0..count as u32);
        }
        self.queue.submit(std::iter::once(encoder.finish()));

        // Invalidate pane caches since we overwrote the instance buffer.
        self.pane_caches.clear();
    }

    /// Submit a separator draw. Call after all panes are rendered.
    pub fn submit_separator(
        &mut self,
        view: &wgpu::TextureView,
        x: u32,
        y: u32,
        width: u32,
        height: u32,
        color: [f32; 4],
    ) {
        let mut encoder = self.device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("separator encoder"),
        });
        self.draw_separator(&mut encoder, view, x, y, width, height, color);
        self.queue.submit(std::iter::once(encoder.finish()));
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
        let instance_count = self.update_instances(terminal, selection);

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
            );
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
                    cell_width_scale: 1.0,
                _pad: [0; 2],
                });
            }
        }

        // We need to temporarily set uniforms for the separator.
        let sep_uniforms = Uniforms {
            cell_size: [cell_ndc_w, cell_ndc_h],
            grid_offset: [sep_ndc_x, sep_ndc_y],
            atlas_size: [self.atlas.width as f32, self.atlas.height as f32],
            emoji_atlas_size: [self.emoji_atlas.width as f32, self.emoji_atlas.height as f32],
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

        // Invalidate all pane caches since we overwrote the instance buffer.
        self.pane_caches.clear();
    }

    /// Render a string of text at a specific pixel position on the surface.
    ///
    /// Each character is rendered as one cell instance. The text is positioned
    /// at `(px_x, px_y)` in physical pixel coordinates (top-left origin).
    /// `clip_rect` optionally overrides the scissor rect as `(x, y, w, h)`.
    pub fn render_text(
        &mut self,
        view: &wgpu::TextureView,
        text: &str,
        px_x: f32,
        px_y: f32,
        fg: [f32; 4],
        bg: [f32; 4],
    ) {
        self.render_text_clipped(view, text, px_x, px_y, fg, bg, None);
    }

    /// Like `render_text` but with an optional explicit scissor clip rect.
    pub fn render_text_clipped(
        &mut self,
        view: &wgpu::TextureView,
        text: &str,
        px_x: f32,
        px_y: f32,
        fg: [f32; 4],
        bg: [f32; 4],
        clip_rect: Option<(u32, u32, u32, u32)>,
    ) {
        if text.is_empty() {
            return;
        }

        let cell_w = self.atlas.cell_size.width;
        let cell_h = self.atlas.cell_size.height;
        let surface_w = self.surface_config.width as f32;
        let surface_h = self.surface_config.height as f32;

        // Build one instance per character.
        let mut instances = Vec::with_capacity(text.len());
        let mut col = 0usize;
        for c in text.chars() {
            let glyph = self.atlas.get_glyph(c);
            let cw = termojinal_vt::char_width(c, self.cjk_width);
            let width_scale = if cw > 1 { cw as f32 } else { 1.0 };
            instances.push(CellInstance {
                grid_pos: [col as f32, 0.0],
                atlas_uv: [
                    glyph.atlas_x,
                    glyph.atlas_y,
                    glyph.atlas_w,
                    glyph.atlas_h,
                ],
                fg_color: fg,
                bg_color: bg,
                flags: 0,
                cell_width_scale: width_scale,
                _pad: [0; 2],
            });
            col += cw;
        }

        if instances.is_empty() {
            return;
        }

        // Re-upload atlas if new glyphs were rasterized.
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

        // Compute NDC positioning for the text origin.
        let ndc_x = (px_x / surface_w) * 2.0 - 1.0;
        let ndc_y = 1.0 - (px_y / surface_h) * 2.0;
        let cell_ndc_w = (cell_w / surface_w) * 2.0;
        let cell_ndc_h = (cell_h / surface_h) * 2.0;

        let text_uniforms = Uniforms {
            cell_size: [cell_ndc_w, cell_ndc_h],
            grid_offset: [ndc_x, ndc_y],
            atlas_size: [self.atlas.width as f32, self.atlas.height as f32],
            emoji_atlas_size: [self.emoji_atlas.width as f32, self.emoji_atlas.height as f32],
            cursor_pos: [0.0; 4],
            cursor_extra: [0.0; 4],
        };

        self.queue
            .write_buffer(&self.uniform_buffer, 0, bytemuck::bytes_of(&text_uniforms));

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

        self.queue.write_buffer(
            &self.instance_buffer,
            0,
            bytemuck::cast_slice(&instances),
        );

        let mut encoder = self.device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("text encoder"),
        });

        {
            let mut render_pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("text render pass"),
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

            // Clip to the text region (or custom clip rect).
            let (clip_x, clip_y, clip_w, clip_h) = if let Some((cx, cy, cw, ch)) = clip_rect {
                (cx, cy, cw, ch)
            } else {
                let text_width = (col as f32 * cell_w).ceil() as u32;
                let text_height = cell_h.ceil() as u32;
                (px_x as u32, px_y as u32, text_width, text_height)
            };
            render_pass.set_scissor_rect(
                clip_x.min(surface_w as u32),
                clip_y.min(surface_h as u32),
                clip_w.min(surface_w as u32 - clip_x.min(surface_w as u32)),
                clip_h.min(surface_h as u32 - clip_y.min(surface_h as u32)),
            );

            render_pass.set_pipeline(&self.render_pipeline);
            render_pass.set_bind_group(0, &self.bind_group, &[]);
            render_pass.set_vertex_buffer(0, self.instance_buffer.slice(..));
            render_pass.draw(0..6, 0..instances.len() as u32);
        }

        self.queue.submit(std::iter::once(encoder.finish()));

        // Invalidate pane caches since we overwrote the instance buffer.
        self.pane_caches.clear();
    }

    /// Change the font size by recreating the atlas and emoji atlas with the new size.
    ///
    /// After calling this, all panes must be resized (since cell dimensions change).
    /// Change the logical font size (in points). Rebuilds the atlas at `size * scale_factor`.
    pub fn set_font_size(&mut self, size: f32) -> Result<(), RenderError> {
        self.font_config = FontConfig { size, ..self.font_config.clone() };
        let scaled_config = FontConfig {
            size: size * self.scale_factor,
            ..self.font_config.clone()
        };
        let mut new_atlas = Atlas::new(&scaled_config)?;
        new_atlas.cjk_width = self.cjk_width;
        self.atlas = new_atlas;

        // Recreate atlas texture with the new atlas dimensions.
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

        // Upload new atlas data.
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
        self.atlas_texture_version = self.atlas.glyph_count();

        // Recreate emoji atlas with new cell dimensions (use scaled font size).
        self.emoji_atlas = EmojiAtlas::new(
            self.atlas.cell_size.width as u32,
            self.atlas.cell_size.height as u32,
            size * self.scale_factor,
        );

        // Recreate emoji texture.
        self.emoji_texture = self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("emoji atlas"),
            size: wgpu::Extent3d {
                width: self.emoji_atlas.width,
                height: self.emoji_atlas.height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8UnormSrgb,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });

        // Upload emoji atlas data.
        self.queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &self.emoji_texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            &self.emoji_atlas.data,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(self.emoji_atlas.width * 4),
                rows_per_image: Some(self.emoji_atlas.height),
            },
            wgpu::Extent3d {
                width: self.emoji_atlas.width,
                height: self.emoji_atlas.height,
                depth_or_array_layers: 1,
            },
        );
        self.emoji_texture_version = 0;

        // Recreate bind group with new texture views.
        self.recreate_bind_group();

        // Invalidate all caches.
        self.pane_caches.clear();

        log::info!("font size changed to {size}");
        Ok(())
    }

    /// Set the present mode (e.g., for ProMotion 120Hz displays).
    /// Try to set a present mode. Returns true if the mode is supported.
    pub fn try_set_present_mode(&mut self, mode: wgpu::PresentMode) -> bool {
        let caps = self.surface.get_capabilities(&self.adapter);
        if caps.present_modes.contains(&mode) {
            self.surface_config.present_mode = mode;
            self.surface.configure(&self.device, &self.surface_config);
            true
        } else {
            false
        }
    }

    /// Update the theme palette for live theme switching.
    ///
    /// Replaces the current palette and clears all per-pane render caches
    /// so that the next frame is fully re-rendered with the new colors.
    pub fn set_theme(&mut self, palette: ThemePalette) {
        self.default_bg = palette.bg;
        self.theme_palette = palette;
        self.pane_caches.clear();
    }

    /// Handle a window resize.
    pub fn resize(&mut self, width: u32, height: u32) {
        if width == 0 || height == 0 {
            return;
        }
        self.surface_config.width = width;
        self.surface_config.height = height;
        self.surface.configure(&self.device, &self.surface_config);
        self.pane_caches.clear();
    }

    /// Get the cell size in pixels.
    pub fn cell_size(&self) -> CellSize {
        self.atlas.cell_size
    }

    /// Set CJK ambiguous width mode on the atlas.
    pub fn atlas_set_cjk_width(&mut self, cjk: bool) {
        self.atlas.cjk_width = cjk;
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

    /// Synchronize GPU image textures with the terminal's image store.
    ///
    /// Call this before rendering when the image store has been modified
    /// (i.e., when `image_store.take_dirty()` returns true).
    pub fn sync_images(&mut self, store: &termojinal_vt::ImageStore) {
        self.image_renderer.sync_images(&self.device, &self.queue, store);
    }

    /// Get a mutable reference to the image renderer.
    pub fn image_renderer_mut(&mut self) -> &mut ImageRenderer {
        &mut self.image_renderer
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

            self.recreate_bind_group();
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

    /// Re-upload the emoji atlas texture to the GPU.
    fn reupload_emoji_atlas(&mut self) {
        let needs_recreate = self.emoji_atlas.width != self.emoji_texture.width()
            || self.emoji_atlas.height != self.emoji_texture.height();

        if needs_recreate {
            self.emoji_texture = self.device.create_texture(&wgpu::TextureDescriptor {
                label: Some("emoji atlas"),
                size: wgpu::Extent3d {
                    width: self.emoji_atlas.width,
                    height: self.emoji_atlas.height,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Rgba8UnormSrgb,
                usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
                view_formats: &[],
            });

            self.recreate_bind_group();
        }

        // Upload emoji atlas data (RGBA, 4 bytes per pixel).
        self.queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &self.emoji_texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            &self.emoji_atlas.data,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(self.emoji_atlas.width * 4),
                rows_per_image: Some(self.emoji_atlas.height),
            },
            wgpu::Extent3d {
                width: self.emoji_atlas.width,
                height: self.emoji_atlas.height,
                depth_or_array_layers: 1,
            },
        );
    }

    /// Recreate the bind group with current atlas and emoji texture views.
    fn recreate_bind_group(&mut self) {
        let atlas_view = self
            .atlas_texture
            .create_view(&wgpu::TextureViewDescriptor::default());
        let atlas_sampler = self.device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("atlas sampler"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        let emoji_view = self
            .emoji_texture
            .create_view(&wgpu::TextureViewDescriptor::default());
        let emoji_sampler = self.device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("emoji sampler"),
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
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: wgpu::BindingResource::TextureView(&emoji_view),
                },
                wgpu::BindGroupEntry {
                    binding: 4,
                    resource: wgpu::BindingResource::Sampler(&emoji_sampler),
                },
            ],
        });
    }

    // -----------------------------------------------------------------------
    // Overlay rendering API (rounded rects + blur)
    // -----------------------------------------------------------------------

    /// Render rounded rectangle overlays (e.g., command palette background).
    pub fn render_rounded_rects(
        &mut self,
        encoder: &mut wgpu::CommandEncoder,
        view: &wgpu::TextureView,
        rects: &[RoundedRect],
    ) {
        let screen_width = self.surface_config.width as f32;
        let screen_height = self.surface_config.height as f32;
        self.rounded_rect_renderer.render(
            encoder, view, &self.device, &self.queue,
            screen_width, screen_height, rects,
        );
    }

    /// Submit rounded rectangle overlays immediately (creates its own encoder).
    ///
    /// Convenience wrapper around [`Self::render_rounded_rects`] that mirrors
    /// the pattern of [`Self::submit_separator`].
    pub fn submit_rounded_rects(
        &mut self,
        view: &wgpu::TextureView,
        rects: &[RoundedRect],
    ) {
        let mut encoder = self.device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("rounded_rect encoder"),
        });
        self.render_rounded_rects(&mut encoder, view, rects);
        self.queue.submit(std::iter::once(encoder.finish()));
    }

    /// Apply a two-pass Gaussian blur to the framebuffer.
    pub fn blur_region(
        &mut self,
        encoder: &mut wgpu::CommandEncoder,
        source: &wgpu::TextureView,
        target: &wgpu::TextureView,
        radius: f32,
    ) {
        let width = self.surface_config.width;
        let height = self.surface_config.height;
        self.blur_renderer.blur(
            encoder, &self.device, &self.queue,
            source, target, radius, width, height,
        );
    }

    /// Get the surface format used by this renderer.
    pub fn surface_format(&self) -> wgpu::TextureFormat {
        self.surface_format
    }

    /// Get the current surface dimensions in physical pixels.
    pub fn surface_size(&self) -> (u32, u32) {
        (self.surface_config.width, self.surface_config.height)
    }
}
