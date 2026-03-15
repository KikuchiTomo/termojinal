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
        })
    }

    /// Render the terminal grid to the surface.
    pub fn render(&mut self, terminal: &jterm_vt::Terminal) -> Result<(), RenderError> {
        let grid = terminal.grid();
        let cols = grid.cols();
        let rows = grid.rows();
        let total_cells = cols * rows;

        // Build instance data for every cell.
        let mut instances = Vec::with_capacity(total_cells);
        for row in 0..rows {
            for col in 0..cols {
                let cell = grid.cell(col, row);

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

                // Mark cursor cell.
                if terminal.modes.cursor_visible
                    && col == terminal.cursor_col
                    && row == terminal.cursor_row
                {
                    flags |= FLAG_IS_CURSOR;
                }

                instances.push(CellInstance {
                    grid_pos: [col as f32, row as f32],
                    atlas_uv: [glyph.atlas_x, glyph.atlas_y, glyph.atlas_w, glyph.atlas_h],
                    fg_color: fg,
                    bg_color: bg,
                    flags,
                    _pad: [0; 3],
                });
            }
        }

        // Re-upload atlas texture if new glyphs were rasterized.
        let current_glyph_count = self.atlas.glyph_count();
        if current_glyph_count != self.atlas_texture_version {
            self.reupload_atlas();
            self.atlas_texture_version = current_glyph_count;
        }

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

        // Upload instance data.
        if !instances.is_empty() {
            self.queue.write_buffer(
                &self.instance_buffer,
                0,
                bytemuck::cast_slice(&instances),
            );
        }

        // Update uniforms.
        let cell_w = self.atlas.cell_size.width;
        let cell_h = self.atlas.cell_size.height;
        let surface_w = self.surface_config.width as f32;
        let surface_h = self.surface_config.height as f32;

        // Convert cell size to NDC (Normalized Device Coordinates).
        // NDC X: -1..+1 (width = 2), NDC Y: -1..+1 (height = 2)
        let cell_ndc_w = (cell_w / surface_w) * 2.0;
        let cell_ndc_h = (cell_h / surface_h) * 2.0;

        // Grid starts at top-left of the window.
        let grid_offset_x = -1.0;
        let grid_offset_y = 1.0;

        // Determine cursor shape for the shader.
        let cursor_shape = match terminal.cursor_shape {
            jterm_vt::CursorShape::Block | jterm_vt::CursorShape::BlinkingBlock => 0.0,
            jterm_vt::CursorShape::Underline | jterm_vt::CursorShape::BlinkingUnderline => 1.0,
            jterm_vt::CursorShape::Bar | jterm_vt::CursorShape::BlinkingBar => 2.0,
        };

        let uniforms = Uniforms {
            cell_size: [cell_ndc_w, cell_ndc_h],
            grid_offset: [grid_offset_x, grid_offset_y],
            atlas_size: [self.atlas.width as f32, self.atlas.height as f32],
            _pad0: [0.0; 2],
            cursor_pos: [
                terminal.cursor_col as f32,
                terminal.cursor_row as f32,
                0.85, // cursor color R (match default FG)
                0.85, // cursor color G
            ],
            cursor_extra: [
                0.85, // cursor color B
                cursor_shape,
                if self.cursor_blink_on { 1.0 } else { 0.0 },
                0.0,
            ],
        };

        self.queue
            .write_buffer(&self.uniform_buffer, 0, bytemuck::bytes_of(&uniforms));

        // Get the next surface texture.
        let output = self.surface.get_current_texture()?;
        let view = output
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());

        // Encode render pass.
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("render encoder"),
            });

        {
            let mut render_pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("cell render pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color {
                            r: color_convert::DEFAULT_BG[0] as f64,
                            g: color_convert::DEFAULT_BG[1] as f64,
                            b: color_convert::DEFAULT_BG[2] as f64,
                            a: 1.0,
                        }),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
            });

            render_pass.set_pipeline(&self.render_pipeline);
            render_pass.set_bind_group(0, &self.bind_group, &[]);
            render_pass.set_vertex_buffer(0, self.instance_buffer.slice(..));
            render_pass.draw(0..6, 0..instances.len() as u32);
        }

        self.queue.submit(std::iter::once(encoder.finish()));
        output.present();

        Ok(())
    }

    /// Handle a window resize.
    pub fn resize(&mut self, width: u32, height: u32) {
        if width == 0 || height == 0 {
            return;
        }
        self.surface_config.width = width;
        self.surface_config.height = height;
        self.surface.configure(&self.device, &self.surface_config);
    }

    /// Get the cell size in pixels.
    pub fn cell_size(&self) -> CellSize {
        self.atlas.cell_size
    }

    /// Calculate grid dimensions (cols, rows) for a given pixel size.
    pub fn grid_size(&self, width: u32, height: u32) -> (u16, u16) {
        let cols = (width as f32 / self.atlas.cell_size.width).floor() as u16;
        let rows = (height as f32 / self.atlas.cell_size.height).floor() as u16;
        (cols.max(1), rows.max(1))
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
