//! Two-pass Gaussian blur renderer.
//!
//! Applies a high-quality Gaussian blur to a region of the screen by running
//! two render passes (horizontal then vertical). Used to create the frosted
//! glass effect behind overlays like the command palette.
//!
//! The blur source is a copy of the current framebuffer content. The caller
//! is responsible for copying the framebuffer before invoking the blur.

/// Uniform data for the blur shader.
#[repr(C)]
#[derive(Copy, Clone, Debug, bytemuck::Pod, bytemuck::Zeroable)]
struct BlurUniforms {
    /// Blur direction: `[1.0, 0.0]` for horizontal, `[0.0, 1.0]` for vertical.
    direction: [f32; 2],
    /// Blur radius in pixels.
    radius: f32,
    /// Padding for 16-byte alignment.
    _padding: f32,
}

/// Two-pass Gaussian blur for background content behind overlays.
///
/// Create once at startup via [`BlurRenderer::new`]. Call [`BlurRenderer::blur`]
/// to apply a Gaussian blur to a source texture, writing the result to a target.
pub struct BlurRenderer {
    pipeline: wgpu::RenderPipeline,
    sampler: wgpu::Sampler,
    /// Intermediate texture for the first blur pass.
    /// Lazily created/resized when `blur()` is called.
    intermediate_texture: Option<(wgpu::Texture, wgpu::TextureView)>,
    intermediate_size: (u32, u32),
    uniform_buffer: wgpu::Buffer,
    bind_group_layout: wgpu::BindGroupLayout,
    format: wgpu::TextureFormat,
}

impl BlurRenderer {
    /// Create a new blur renderer.
    ///
    /// `format` should match the surface texture format.
    pub fn new(device: &wgpu::Device, format: wgpu::TextureFormat) -> Self {
        // Load the WGSL shader.
        let shader_source = include_str!("blur.wgsl");
        let shader_module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("blur shader"),
            source: wgpu::ShaderSource::Wgsl(shader_source.into()),
        });

        // Bind group layout: input texture, sampler, uniform buffer.
        let bind_group_layout =
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("blur bind group layout"),
                entries: &[
                    // Input texture.
                    wgpu::BindGroupLayoutEntry {
                        binding: 0,
                        visibility: wgpu::ShaderStages::FRAGMENT,
                        ty: wgpu::BindingType::Texture {
                            sample_type: wgpu::TextureSampleType::Float { filterable: true },
                            view_dimension: wgpu::TextureViewDimension::D2,
                            multisampled: false,
                        },
                        count: None,
                    },
                    // Sampler.
                    wgpu::BindGroupLayoutEntry {
                        binding: 1,
                        visibility: wgpu::ShaderStages::FRAGMENT,
                        ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                        count: None,
                    },
                    // Uniform buffer.
                    wgpu::BindGroupLayoutEntry {
                        binding: 2,
                        visibility: wgpu::ShaderStages::FRAGMENT,
                        ty: wgpu::BindingType::Buffer {
                            ty: wgpu::BufferBindingType::Uniform,
                            has_dynamic_offset: false,
                            min_binding_size: None,
                        },
                        count: None,
                    },
                ],
            });

        // Pipeline layout.
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("blur pipeline layout"),
            bind_group_layouts: &[&bind_group_layout],
            push_constant_ranges: &[],
        });

        // Sampler with clamp-to-edge to avoid bleeding at texture borders.
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("blur sampler"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            ..Default::default()
        });

        // Render pipeline. No vertex buffer — fullscreen triangle is generated
        // in the vertex shader.
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("blur pipeline"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader_module,
                entry_point: Some("vs_main"),
                compilation_options: Default::default(),
                buffers: &[],
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
                    format,
                    blend: None, // Blur replaces content, no blending needed.
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            multiview: None,
            cache: None,
        });

        // Uniform buffer (reused for both passes).
        let uniform_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("blur uniforms"),
            size: std::mem::size_of::<BlurUniforms>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        Self {
            pipeline,
            sampler,
            intermediate_texture: None,
            intermediate_size: (0, 0),
            uniform_buffer,
            bind_group_layout,
            format,
        }
    }

    /// Ensure the intermediate texture exists and matches the required size.
    fn ensure_intermediate_texture(&mut self, device: &wgpu::Device, width: u32, height: u32) {
        if self.intermediate_size == (width, height) && self.intermediate_texture.is_some() {
            return;
        }

        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("blur intermediate"),
            size: wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: self.format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        self.intermediate_texture = Some((texture, view));
        self.intermediate_size = (width, height);
    }

    /// Create a bind group for a blur pass.
    fn create_bind_group(
        &self,
        device: &wgpu::Device,
        input_view: &wgpu::TextureView,
        label: &str,
    ) -> wgpu::BindGroup {
        device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some(label),
            layout: &self.bind_group_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(input_view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&self.sampler),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: self.uniform_buffer.as_entire_binding(),
                },
            ],
        })
    }

    /// Apply a two-pass Gaussian blur.
    ///
    /// Reads from `source`, writes the blurred result to `target`.
    /// The blur is applied to the entire texture (fullscreen triangle).
    ///
    /// # Arguments
    ///
    /// * `encoder` - Command encoder to record blur passes into.
    /// * `device` - wgpu device (used for bind group and texture creation).
    /// * `queue` - wgpu queue (used to upload uniform data).
    /// * `source` - Input texture view (framebuffer copy).
    /// * `target` - Output texture view (where blurred result is written).
    /// * `radius` - Blur radius in pixels.
    /// * `width` - Texture width in pixels.
    /// * `height` - Texture height in pixels.
    pub fn blur(
        &mut self,
        encoder: &mut wgpu::CommandEncoder,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        source: &wgpu::TextureView,
        target: &wgpu::TextureView,
        radius: f32,
        width: u32,
        height: u32,
    ) {
        if radius <= 0.0 {
            return;
        }

        // Ensure intermediate texture is ready.
        self.ensure_intermediate_texture(device, width, height);
        let intermediate_view = &self.intermediate_texture.as_ref().unwrap().1;

        // --- Pass 1: Horizontal blur (source -> intermediate) ---
        let h_uniforms = BlurUniforms {
            direction: [1.0, 0.0],
            radius,
            _padding: 0.0,
        };
        queue.write_buffer(&self.uniform_buffer, 0, bytemuck::bytes_of(&h_uniforms));

        let h_bind_group = self.create_bind_group(device, source, "blur horizontal bind group");

        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("blur horizontal pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: intermediate_view,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
            });
            pass.set_pipeline(&self.pipeline);
            pass.set_bind_group(0, &h_bind_group, &[]);
            pass.draw(0..3, 0..1); // Fullscreen triangle.
        }

        // --- Pass 2: Vertical blur (intermediate -> target) ---
        let v_uniforms = BlurUniforms {
            direction: [0.0, 1.0],
            radius,
            _padding: 0.0,
        };
        queue.write_buffer(&self.uniform_buffer, 0, bytemuck::bytes_of(&v_uniforms));

        let v_bind_group =
            self.create_bind_group(device, intermediate_view, "blur vertical bind group");

        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("blur vertical pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: target,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
            });
            pass.set_pipeline(&self.pipeline);
            pass.set_bind_group(0, &v_bind_group, &[]);
            pass.draw(0..3, 0..1); // Fullscreen triangle.
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blur_uniforms_size_is_16_bytes() {
        assert_eq!(std::mem::size_of::<BlurUniforms>(), 16);
    }

    #[test]
    fn blur_uniforms_is_pod_and_zeroable() {
        let zero: BlurUniforms = bytemuck::Zeroable::zeroed();
        assert_eq!(zero.direction, [0.0; 2]);
        assert_eq!(zero.radius, 0.0);

        let u = BlurUniforms {
            direction: [1.0, 0.0],
            radius: 12.0,
            _padding: 0.0,
        };
        let bytes: &[u8] = bytemuck::bytes_of(&u);
        assert_eq!(bytes.len(), 16);
        let roundtrip: &BlurUniforms = bytemuck::from_bytes(bytes);
        assert_eq!(roundtrip.direction, [1.0, 0.0]);
        assert_eq!(roundtrip.radius, 12.0);
    }
}
