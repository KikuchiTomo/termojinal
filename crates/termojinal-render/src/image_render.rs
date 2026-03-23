//! GPU image renderer for terminal inline images.
//!
//! Manages per-image GPU textures and renders image placements as textured
//! quads on top of the terminal cell grid. Uses a separate render pipeline
//! from the cell renderer so image draw calls don't interfere with the
//! instanced cell rendering.

use std::collections::HashMap;

use termojinal_vt::image::{ImagePlacement, ImageStore};

/// A GPU-resident image (wgpu texture + bind group).
struct GpuImage {
    _texture: wgpu::Texture,
    bind_group: wgpu::BindGroup,
    #[allow(dead_code)]
    width: u32,
    #[allow(dead_code)]
    height: u32,
}

/// Per-image uniform data for the image shader.
#[repr(C)]
#[derive(Clone, Copy, Debug, bytemuck::Pod, bytemuck::Zeroable)]
struct ImageUniforms {
    /// Quad top-left position in NDC.
    quad_pos: [f32; 2],
    /// Quad size in NDC.
    quad_size: [f32; 2],
}

/// The image rendering subsystem.
///
/// Owns GPU resources for rendering inline images. Created once and passed
/// to the main renderer. Images are uploaded lazily when the `ImageStore`
/// reports them as dirty.
pub struct ImageRenderer {
    pipeline: wgpu::RenderPipeline,
    bind_group_layout: wgpu::BindGroupLayout,
    uniform_buffer: wgpu::Buffer,
    /// Cached GPU images keyed by image ID.
    gpu_images: HashMap<u32, GpuImage>,
}

impl ImageRenderer {
    /// Create a new image renderer.
    pub fn new(device: &wgpu::Device, surface_format: wgpu::TextureFormat) -> Self {
        let shader_source = include_str!("image_shader.wgsl");
        let shader_module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("image shader"),
            source: wgpu::ShaderSource::Wgsl(shader_source.into()),
        });

        let bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("image bind group layout"),
            entries: &[
                // Uniform buffer.
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::VERTEX,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                // Image texture.
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
                // Image sampler.
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });

        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("image pipeline layout"),
            bind_group_layouts: &[&bind_group_layout],
            push_constant_ranges: &[],
        });

        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("image pipeline"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader_module,
                entry_point: Some("vs_main"),
                compilation_options: Default::default(),
                buffers: &[], // No vertex buffer — quad vertices generated in shader.
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

        let uniform_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("image uniforms"),
            size: std::mem::size_of::<ImageUniforms>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        Self {
            pipeline,
            bind_group_layout,
            uniform_buffer,
            gpu_images: HashMap::new(),
        }
    }

    /// Synchronize GPU images with the image store.
    ///
    /// Uploads new images, removes deleted ones.
    pub fn sync_images(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        store: &ImageStore,
    ) {
        // Remove GPU images that are no longer in the store.
        let store_images = store.images();
        self.gpu_images.retain(|id, _| store_images.contains_key(id));

        // Upload new images.
        for (id, img) in store_images {
            if self.gpu_images.contains_key(id) {
                continue;
            }
            if img.width == 0 || img.height == 0 || img.data.is_empty() {
                continue;
            }

            let texture = device.create_texture(&wgpu::TextureDescriptor {
                label: Some(&format!("image {id}")),
                size: wgpu::Extent3d {
                    width: img.width,
                    height: img.height,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Rgba8UnormSrgb,
                usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
                view_formats: &[],
            });

            queue.write_texture(
                wgpu::TexelCopyTextureInfo {
                    texture: &texture,
                    mip_level: 0,
                    origin: wgpu::Origin3d::ZERO,
                    aspect: wgpu::TextureAspect::All,
                },
                &img.data,
                wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(img.width * 4),
                    rows_per_image: Some(img.height),
                },
                wgpu::Extent3d {
                    width: img.width,
                    height: img.height,
                    depth_or_array_layers: 1,
                },
            );

            let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
            let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
                label: Some("image sampler"),
                mag_filter: wgpu::FilterMode::Linear,
                min_filter: wgpu::FilterMode::Linear,
                ..Default::default()
            });

            let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some(&format!("image bind group {id}")),
                layout: &self.bind_group_layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: self.uniform_buffer.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::TextureView(&view),
                    },
                    wgpu::BindGroupEntry {
                        binding: 2,
                        resource: wgpu::BindingResource::Sampler(&sampler),
                    },
                ],
            });

            self.gpu_images.insert(
                *id,
                GpuImage {
                    _texture: texture,
                    bind_group,
                    width: img.width,
                    height: img.height,
                },
            );

            log::debug!("image_render: uploaded image id={id} {}x{}", img.width, img.height);
        }
    }

    /// Render all image placements.
    ///
    /// Should be called after cell rendering within the same render pass or
    /// as a separate pass on top of the cell content.
    ///
    /// `cell_width_px` and `cell_height_px` are the cell dimensions in pixels.
    /// `surface_width` and `surface_height` are the surface dimensions in pixels.
    /// `grid_offset_px` is the grid's top-left corner in pixels (for single-pane: padding).
    pub fn render_placements(
        &self,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        view: &wgpu::TextureView,
        placements: &[ImagePlacement],
        cell_width_px: f32,
        cell_height_px: f32,
        surface_width: f32,
        surface_height: f32,
        grid_offset_px: (f32, f32),
        viewport: Option<(u32, u32, u32, u32)>,
        scroll_offset: usize,
    ) {
        for placement in placements {
            let gpu_img = match self.gpu_images.get(&placement.image_id) {
                Some(img) => img,
                None => continue,
            };

            // Adjust row for scrollback viewing offset.
            // When scroll_offset > 0 the user is looking at history, so
            // images should shift down (visually further back in time).
            let display_row = placement.row + scroll_offset as isize;

            // Skip images entirely above or below the visible area.
            if display_row + placement.cell_rows as isize <= 0 {
                continue;
            }

            // Calculate quad position in pixels.
            let px_x = grid_offset_px.0 + placement.col as f32 * cell_width_px;
            let px_y = grid_offset_px.1 + display_row as f32 * cell_height_px;
            let px_w = placement.cell_cols as f32 * cell_width_px;
            let px_h = placement.cell_rows as f32 * cell_height_px;

            // Convert to NDC.
            let ndc_x = (px_x / surface_width) * 2.0 - 1.0;
            let ndc_y = 1.0 - (px_y / surface_height) * 2.0;
            let ndc_w = (px_w / surface_width) * 2.0;
            let ndc_h = (px_h / surface_height) * 2.0;

            let uniforms = ImageUniforms {
                quad_pos: [ndc_x, ndc_y],
                quad_size: [ndc_w, ndc_h],
            };

            queue.write_buffer(&self.uniform_buffer, 0, bytemuck::bytes_of(&uniforms));

            {
                let mut render_pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("image render pass"),
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

                render_pass.set_pipeline(&self.pipeline);
                render_pass.set_bind_group(0, &gpu_img.bind_group, &[]);
                render_pass.draw(0..6, 0..1);
            }
        }
    }

    /// Check if there are any GPU images loaded.
    pub fn has_images(&self) -> bool {
        !self.gpu_images.is_empty()
    }

    /// Clear all GPU images (e.g., on terminal reset).
    pub fn clear(&mut self) {
        self.gpu_images.clear();
    }
}
