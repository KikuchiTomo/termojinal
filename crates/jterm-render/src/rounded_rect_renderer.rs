//! GPU renderer for SDF-based rounded rectangles.
//!
//! Draws rounded rectangles with anti-aliased edges using a signed distance
//! field evaluated in the fragment shader. Supports configurable corner radius,
//! optional border, and optional drop shadow.
//!
//! Uses instanced rendering: each `RoundedRect` is one instance drawn as a
//! 6-vertex quad (two triangles), following the same pattern as the cell renderer.

/// A rounded rectangle to render.
///
/// Sent to the GPU as per-instance vertex data. The fragment shader evaluates
/// an SDF at each pixel to produce smooth edges, borders, and shadows.
#[repr(C)]
#[derive(Copy, Clone, Debug, bytemuck::Pod, bytemuck::Zeroable)]
pub struct RoundedRect {
    /// Rectangle position and size in pixels: `[x, y, width, height]`.
    pub rect: [f32; 4],
    /// Fill color RGBA (premultiplied alpha recommended).
    pub color: [f32; 4],
    /// Border color RGBA.
    pub border_color: [f32; 4],
    /// Rendering parameters:
    /// - `[0]` corner_radius: radius of rounded corners in pixels
    /// - `[1]` border_width: border thickness in pixels (0 = no border)
    /// - `[2]` shadow_radius: drop shadow blur radius in pixels (0 = no shadow)
    /// - `[3]` shadow_opacity: drop shadow opacity (0.0 - 1.0)
    pub params: [f32; 4],
}

/// Uniform data for the rounded rectangle shader.
#[repr(C)]
#[derive(Copy, Clone, Debug, bytemuck::Pod, bytemuck::Zeroable)]
struct Uniforms {
    /// Screen dimensions in pixels: `[width, height]`.
    screen_size: [f32; 2],
    /// Padding for 16-byte alignment.
    _padding: [f32; 2],
}

/// Renders rounded rectangles with optional blur, border, and shadow.
///
/// Create once at startup via [`RoundedRectRenderer::new`], then call
/// [`RoundedRectRenderer::render`] each frame with the rectangles to draw.
pub struct RoundedRectRenderer {
    pipeline: wgpu::RenderPipeline,
    uniform_buffer: wgpu::Buffer,
    #[allow(dead_code)]
    bind_group_layout: wgpu::BindGroupLayout,
    bind_group: wgpu::BindGroup,
    instance_buffer: wgpu::Buffer,
    instance_capacity: usize,
}

/// Maximum number of rounded rect instances before the buffer is resized.
const INITIAL_INSTANCE_CAPACITY: usize = 16;

impl RoundedRectRenderer {
    /// Create a new rounded rectangle renderer.
    ///
    /// `format` should match the surface texture format.
    pub fn new(device: &wgpu::Device, format: wgpu::TextureFormat) -> Self {
        // Load the WGSL shader.
        let shader_source = include_str!("rounded_rect.wgsl");
        let shader_module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("rounded_rect shader"),
            source: wgpu::ShaderSource::Wgsl(shader_source.into()),
        });

        // Bind group layout: one uniform buffer for screen dimensions.
        let bind_group_layout =
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("rounded_rect bind group layout"),
                entries: &[wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::VERTEX | wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                }],
            });

        // Uniform buffer.
        let uniform_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("rounded_rect uniforms"),
            size: std::mem::size_of::<Uniforms>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        // Bind group.
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("rounded_rect bind group"),
            layout: &bind_group_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: uniform_buffer.as_entire_binding(),
            }],
        });

        // Pipeline layout.
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("rounded_rect pipeline layout"),
            bind_group_layouts: &[&bind_group_layout],
            push_constant_ranges: &[],
        });

        // Instance buffer layout: 4 vec4<f32> attributes.
        let instance_layout = wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<RoundedRect>() as u64,
            step_mode: wgpu::VertexStepMode::Instance,
            attributes: &[
                // rect: vec4<f32> at location(0)
                wgpu::VertexAttribute {
                    format: wgpu::VertexFormat::Float32x4,
                    offset: 0,
                    shader_location: 0,
                },
                // color: vec4<f32> at location(1)
                wgpu::VertexAttribute {
                    format: wgpu::VertexFormat::Float32x4,
                    offset: 16,
                    shader_location: 1,
                },
                // border_color: vec4<f32> at location(2)
                wgpu::VertexAttribute {
                    format: wgpu::VertexFormat::Float32x4,
                    offset: 32,
                    shader_location: 2,
                },
                // params: vec4<f32> at location(3)
                wgpu::VertexAttribute {
                    format: wgpu::VertexFormat::Float32x4,
                    offset: 48,
                    shader_location: 3,
                },
            ],
        };

        // Render pipeline with alpha blending (overlays on top of existing content).
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("rounded_rect pipeline"),
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
                    format,
                    blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            multiview: None,
            cache: None,
        });

        // Instance buffer.
        let instance_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("rounded_rect instances"),
            size: (INITIAL_INSTANCE_CAPACITY * std::mem::size_of::<RoundedRect>()) as u64,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        Self {
            pipeline,
            uniform_buffer,
            bind_group_layout,
            bind_group,
            instance_buffer,
            instance_capacity: INITIAL_INSTANCE_CAPACITY,
        }
    }

    /// Submit rounded rectangles to render this frame.
    ///
    /// Encodes a render pass into the provided `encoder` that draws the
    /// given rectangles on top of existing surface content (load + store).
    ///
    /// # Arguments
    ///
    /// * `encoder` - Command encoder to record draw commands into.
    /// * `view` - Texture view to render onto.
    /// * `device` - wgpu device (used to resize the instance buffer if needed).
    /// * `queue` - wgpu queue (used to upload instance and uniform data).
    /// * `screen_width` - Surface width in physical pixels.
    /// * `screen_height` - Surface height in physical pixels.
    /// * `rects` - Slice of rounded rectangles to draw.
    pub fn render(
        &mut self,
        encoder: &mut wgpu::CommandEncoder,
        view: &wgpu::TextureView,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        screen_width: f32,
        screen_height: f32,
        rects: &[RoundedRect],
    ) {
        if rects.is_empty() {
            return;
        }

        // Upload uniforms.
        let uniforms = Uniforms {
            screen_size: [screen_width, screen_height],
            _padding: [0.0; 2],
        };
        queue.write_buffer(&self.uniform_buffer, 0, bytemuck::bytes_of(&uniforms));

        // Grow instance buffer if needed.
        if rects.len() > self.instance_capacity {
            self.instance_capacity = rects.len().next_power_of_two();
            self.instance_buffer = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("rounded_rect instances"),
                size: (self.instance_capacity * std::mem::size_of::<RoundedRect>()) as u64,
                usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
        }

        // Upload instance data.
        queue.write_buffer(
            &self.instance_buffer,
            0,
            bytemuck::cast_slice(rects),
        );

        // Encode the render pass.
        {
            let mut render_pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("rounded_rect render pass"),
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
            render_pass.set_pipeline(&self.pipeline);
            render_pass.set_bind_group(0, &self.bind_group, &[]);
            render_pass.set_vertex_buffer(0, self.instance_buffer.slice(..));
            render_pass.draw(0..6, 0..rects.len() as u32);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rounded_rect_is_pod_and_zeroable() {
        // Verify that RoundedRect satisfies Pod and Zeroable constraints.
        let zero: RoundedRect = bytemuck::Zeroable::zeroed();
        assert_eq!(zero.rect, [0.0; 4]);
        assert_eq!(zero.color, [0.0; 4]);
        assert_eq!(zero.border_color, [0.0; 4]);
        assert_eq!(zero.params, [0.0; 4]);

        // Verify Pod: can cast slice to bytes and back.
        let rect = RoundedRect {
            rect: [100.0, 200.0, 400.0, 300.0],
            color: [0.1, 0.2, 0.3, 0.9],
            border_color: [1.0, 1.0, 1.0, 0.5],
            params: [12.0, 1.0, 8.0, 0.3],
        };
        let bytes: &[u8] = bytemuck::bytes_of(&rect);
        assert_eq!(bytes.len(), std::mem::size_of::<RoundedRect>());
        let roundtrip: &RoundedRect = bytemuck::from_bytes(bytes);
        assert_eq!(roundtrip.rect, rect.rect);
        assert_eq!(roundtrip.color, rect.color);
    }

    #[test]
    fn rounded_rect_size_is_64_bytes() {
        // 4 vec4<f32> = 4 * 16 = 64 bytes. No padding needed.
        assert_eq!(std::mem::size_of::<RoundedRect>(), 64);
    }

    #[test]
    fn uniforms_size_is_16_bytes() {
        // vec2<f32> + vec2<f32> padding = 16 bytes.
        assert_eq!(std::mem::size_of::<Uniforms>(), 16);
    }

    /// Rust-side equivalent of the SDF function for unit testing.
    fn sdf_rounded_rect(
        p: [f32; 2],
        center: [f32; 2],
        half_size: [f32; 2],
        radius: f32,
    ) -> f32 {
        let r = radius.min(half_size[0].min(half_size[1]));
        let qx = (p[0] - center[0]).abs() - half_size[0] + r;
        let qy = (p[1] - center[1]).abs() - half_size[1] + r;
        let outside = (qx.max(0.0) * qx.max(0.0) + qy.max(0.0) * qy.max(0.0)).sqrt();
        let inside = qx.max(qy).min(0.0);
        outside + inside - r
    }

    #[test]
    fn sdf_center_is_negative() {
        // Center of a 100x100 rect should be well inside (negative distance).
        let d = sdf_rounded_rect([50.0, 50.0], [50.0, 50.0], [50.0, 50.0], 10.0);
        assert!(d < 0.0, "center should be inside, got {d}");
    }

    #[test]
    fn sdf_outside_is_positive() {
        // Point well outside the rect should have positive distance.
        let d = sdf_rounded_rect([200.0, 200.0], [50.0, 50.0], [50.0, 50.0], 10.0);
        assert!(d > 0.0, "outside point should be positive, got {d}");
    }

    #[test]
    fn sdf_on_edge_is_near_zero() {
        // Point on the flat edge (no corner rounding effect) should be ~0.
        // Right edge at y=center: x = center_x + half_size_x = 100.
        let d = sdf_rounded_rect([100.0, 50.0], [50.0, 50.0], [50.0, 50.0], 0.0);
        assert!(d.abs() < 0.01, "edge should be ~0, got {d}");
    }

    #[test]
    fn sdf_corner_radius_pushes_corner_inward() {
        // Corner of a rect with radius=10: the actual corner at (100, 100)
        // for a rect centered at (50,50) with half_size (50,50) should be
        // outside when radius > 0.
        let d_no_radius = sdf_rounded_rect([100.0, 100.0], [50.0, 50.0], [50.0, 50.0], 0.0);
        let d_with_radius = sdf_rounded_rect([100.0, 100.0], [50.0, 50.0], [50.0, 50.0], 10.0);
        assert!(
            d_with_radius > d_no_radius,
            "corner should be further outside with radius: no_r={d_no_radius}, r={d_with_radius}"
        );
    }
}
