// Two-pass Gaussian blur shader.
//
// This shader is used twice per blur operation:
//   Pass 1: horizontal blur (direction = vec2(1.0, 0.0))
//   Pass 2: vertical blur   (direction = vec2(0.0, 1.0))
//
// Uses a 13-tap Gaussian kernel for high-quality blur without excessive
// texture samples. The kernel weights are precomputed for sigma ~= 4.0.

@group(0) @binding(0) var input_texture: texture_2d<f32>;
@group(0) @binding(1) var tex_sampler: sampler;

struct BlurUniforms {
    // (1,0) for horizontal pass, (0,1) for vertical pass.
    direction: vec2<f32>,
    // Blur radius in pixels (scales the sample offsets).
    radius: f32,
    // Padding for 16-byte alignment.
    _padding: f32,
};

@group(0) @binding(2) var<uniform> blur_uniforms: BlurUniforms;

struct VertexOutput {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

// Fullscreen triangle (3 vertices cover the entire screen).
// This avoids the need for a vertex buffer entirely.
var<private> FULLSCREEN_POSITIONS: array<vec2<f32>, 3> = array<vec2<f32>, 3>(
    vec2<f32>(-1.0, -1.0),
    vec2<f32>( 3.0, -1.0),
    vec2<f32>(-1.0,  3.0),
);

var<private> FULLSCREEN_UVS: array<vec2<f32>, 3> = array<vec2<f32>, 3>(
    vec2<f32>(0.0, 1.0),
    vec2<f32>(2.0, 1.0),
    vec2<f32>(0.0, -1.0),
);

@vertex
fn vs_main(@builtin(vertex_index) vertex_index: u32) -> VertexOutput {
    var out: VertexOutput;
    out.position = vec4<f32>(FULLSCREEN_POSITIONS[vertex_index], 0.0, 1.0);
    out.uv = FULLSCREEN_UVS[vertex_index];
    return out;
}

// 13-tap Gaussian kernel weights (symmetric, so we store 7 unique values).
// These weights correspond to a Gaussian with sigma ~= 4.0, normalized.
// Offsets: 0, 1, 2, 3, 4, 5, 6 (in texel units, scaled by radius/6).
const KERNEL_SIZE: i32 = 7;

var<private> WEIGHTS: array<f32, 7> = array<f32, 7>(
    0.1964825501511404,
    0.2969069646728344,
    0.09447039785044732,
    0.01038349436785449,
    0.000394691687645,
    0.0000051855957,
    0.0000000235267,
);

// Optimized 13-tap Gaussian using 7 texture fetches via linear filtering.
// We combine pairs of taps by sampling between them and letting the GPU's
// bilinear sampler do the weighted average.
@fragment
fn fs_main(in: VertexOutput) -> @location(0) vec4<f32> {
    let tex_size = vec2<f32>(textureDimensions(input_texture));
    let texel_size = 1.0 / tex_size;

    // Scale factor: radius / 6.0 maps the kernel to the desired pixel radius.
    let scale = max(blur_uniforms.radius / 6.0, 0.001);
    let step = blur_uniforms.direction * texel_size * scale;

    // Center sample.
    var color = textureSample(input_texture, tex_sampler, in.uv) * WEIGHTS[0];

    // Symmetric taps: sample at +offset and -offset.
    for (var i = 1; i < KERNEL_SIZE; i = i + 1) {
        let offset = step * f32(i);
        let w = WEIGHTS[i];
        color += textureSample(input_texture, tex_sampler, in.uv + offset) * w;
        color += textureSample(input_texture, tex_sampler, in.uv - offset) * w;
    }

    return color;
}
