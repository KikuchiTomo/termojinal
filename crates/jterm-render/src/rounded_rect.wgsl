// Rounded rectangle renderer using Signed Distance Fields (SDF).
//
// Each rounded rectangle is drawn as a fullscreen-quad-per-instance.
// The fragment shader evaluates the SDF to produce pixel-perfect
// anti-aliased edges, optional border, and optional drop shadow.

// Per-instance input from the vertex buffer.
struct RoundedRectInstance {
    @location(0) rect: vec4<f32>,         // x, y, width, height (in pixels)
    @location(1) color: vec4<f32>,        // fill color RGBA
    @location(2) border_color: vec4<f32>, // border color RGBA
    @location(3) params: vec4<f32>,       // corner_radius, border_width, shadow_radius, shadow_opacity
};

// Screen dimensions uniform.
struct Uniforms {
    screen_size: vec2<f32>,
};

@group(0) @binding(0) var<uniform> uniforms: Uniforms;

struct VertexOutput {
    @builtin(position) position: vec4<f32>,
    @location(0) pixel_pos: vec2<f32>,     // fragment position in pixels
    @location(1) rect: vec4<f32>,          // pass-through: x, y, w, h
    @location(2) color: vec4<f32>,         // pass-through: fill color
    @location(3) border_color: vec4<f32>,  // pass-through: border color
    @location(4) params: vec4<f32>,        // pass-through: radius, border, shadow_r, shadow_a
};

// 6 vertices for a quad (two triangles), covering the instance bounding box
// with extra padding for the drop shadow.
var<private> QUAD_POS: array<vec2<f32>, 6> = array<vec2<f32>, 6>(
    vec2<f32>(0.0, 0.0),
    vec2<f32>(1.0, 0.0),
    vec2<f32>(0.0, 1.0),
    vec2<f32>(1.0, 0.0),
    vec2<f32>(1.0, 1.0),
    vec2<f32>(0.0, 1.0),
);

@vertex
fn vs_main(
    @builtin(vertex_index) vertex_index: u32,
    instance: RoundedRectInstance,
) -> VertexOutput {
    let quad = QUAD_POS[vertex_index];

    // Expand the quad beyond the rectangle to accommodate the drop shadow.
    let shadow_radius = instance.params.z;
    let padding = shadow_radius * 2.0 + 2.0; // extra margin for anti-aliasing

    // Bounding box in pixels (with shadow padding).
    let box_x = instance.rect.x - padding;
    let box_y = instance.rect.y - padding;
    let box_w = instance.rect.z + padding * 2.0;
    let box_h = instance.rect.w + padding * 2.0;

    // Pixel position of this vertex.
    let px = box_x + quad.x * box_w;
    let py = box_y + quad.y * box_h;

    // Convert pixel position to NDC: x in [-1, 1], y in [-1, 1].
    // Screen origin is top-left; NDC origin is bottom-left.
    let ndc_x = (px / uniforms.screen_size.x) * 2.0 - 1.0;
    let ndc_y = 1.0 - (py / uniforms.screen_size.y) * 2.0;

    var out: VertexOutput;
    out.position = vec4<f32>(ndc_x, ndc_y, 0.0, 1.0);
    out.pixel_pos = vec2<f32>(px, py);
    out.rect = instance.rect;
    out.color = instance.color;
    out.border_color = instance.border_color;
    out.params = instance.params;
    return out;
}

// Signed distance function for a rounded rectangle.
//
// `p`         - evaluation point
// `center`    - rectangle center
// `half_size` - half of width and height
// `radius`    - corner radius (clamped to not exceed half_size)
//
// Returns negative inside, zero on edge, positive outside.
fn sdf_rounded_rect(p: vec2<f32>, center: vec2<f32>, half_size: vec2<f32>, radius: f32) -> f32 {
    let r = min(radius, min(half_size.x, half_size.y));
    let q = abs(p - center) - half_size + vec2<f32>(r, r);
    return length(max(q, vec2<f32>(0.0, 0.0))) + min(max(q.x, q.y), 0.0) - r;
}

// Approximate Gaussian falloff for soft shadow.
// Uses a smooth exponential decay based on distance and radius.
fn shadow_falloff(distance: f32, radius: f32) -> f32 {
    if radius <= 0.0 {
        return 0.0;
    }
    let t = distance / radius;
    return exp(-t * t * 2.0);
}

@fragment
fn fs_main(in: VertexOutput) -> @location(0) vec4<f32> {
    let rect_x = in.rect.x;
    let rect_y = in.rect.y;
    let rect_w = in.rect.z;
    let rect_h = in.rect.w;

    let corner_radius = in.params.x;
    let border_width = in.params.y;
    let shadow_radius = in.params.z;
    let shadow_opacity = in.params.w;

    let center = vec2<f32>(rect_x + rect_w * 0.5, rect_y + rect_h * 0.5);
    let half_size = vec2<f32>(rect_w * 0.5, rect_h * 0.5);

    let p = in.pixel_pos;

    // Evaluate the SDF at this fragment.
    let dist = sdf_rounded_rect(p, center, half_size, corner_radius);

    // --- Drop shadow ---
    // The shadow is rendered outside the rectangle. It uses the same SDF
    // but fades out with a Gaussian-like falloff.
    var shadow_alpha = 0.0;
    if shadow_radius > 0.0 && shadow_opacity > 0.0 {
        // Only draw shadow outside the rect.
        let shadow_dist = max(dist, 0.0);
        shadow_alpha = shadow_falloff(shadow_dist, shadow_radius) * shadow_opacity;
    }

    // --- Fill ---
    // smoothstep for anti-aliased edge: 0.5px transition.
    let fill_alpha = 1.0 - smoothstep(-0.5, 0.5, dist);

    // --- Border ---
    var border_alpha = 0.0;
    if border_width > 0.0 {
        // The border occupies the region where dist is between
        // -border_width and 0.0 (i.e., just inside the edge).
        let inner_dist = dist + border_width;
        let outer_edge = 1.0 - smoothstep(-0.5, 0.5, dist);
        let inner_edge = 1.0 - smoothstep(-0.5, 0.5, inner_dist);
        border_alpha = outer_edge - inner_edge;
    }

    // Composite layers: shadow behind, fill on top, border on top of fill.
    // Shadow color: black with shadow alpha.
    var result = vec4<f32>(0.0, 0.0, 0.0, shadow_alpha);

    // Blend fill over shadow.
    let fill_color = vec4<f32>(in.color.rgb, in.color.a * fill_alpha);
    result = vec4<f32>(
        mix(result.rgb, fill_color.rgb, fill_color.a),
        result.a * (1.0 - fill_color.a) + fill_color.a,
    );

    // Blend border over fill.
    if border_alpha > 0.0 {
        let bc = vec4<f32>(in.border_color.rgb, in.border_color.a * border_alpha);
        result = vec4<f32>(
            mix(result.rgb, bc.rgb, bc.a),
            result.a * (1.0 - bc.a) + bc.a,
        );
    }

    // Discard fully transparent fragments to avoid unnecessary blending.
    if result.a < 0.001 {
        discard;
    }

    return result;
}
