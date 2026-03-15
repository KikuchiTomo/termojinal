// jterm terminal cell renderer
//
// Each cell is drawn as a quad (two triangles from 6 vertices).
// Per-instance data carries the grid position, atlas UV region,
// foreground/background colors, and attribute flags.

// Uniform: grid-to-NDC transform parameters.
struct Uniforms {
    // cell_size in NDC: vec2(cell_width_ndc, cell_height_ndc)
    cell_size: vec2<f32>,
    // grid offset in NDC (top-left corner)
    grid_offset: vec2<f32>,
    // atlas texture dimensions (width, height) for UV normalization
    atlas_size: vec2<f32>,
    // extra: cursor_col, cursor_row, cursor_color_r, cursor_color_g
    cursor_pos: vec4<f32>,
    // cursor_color_b, cursor_shape (0=block, 1=underline, 2=bar), blink_on (0 or 1), _pad
    cursor_extra: vec4<f32>,
}

@group(0) @binding(0) var<uniform> uniforms: Uniforms;
@group(0) @binding(1) var atlas_texture: texture_2d<f32>;
@group(0) @binding(2) var atlas_sampler: sampler;

struct CellInstance {
    @location(0) grid_pos: vec2<f32>,    // column, row
    @location(1) atlas_uv: vec4<f32>,    // x, y, w, h in texels
    @location(2) fg_color: vec4<f32>,    // foreground RGBA
    @location(3) bg_color: vec4<f32>,    // background RGBA
    @location(4) flags: u32,             // attribute bit flags
}

struct VertexOutput {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,         // UV within atlas (normalized)
    @location(1) fg_color: vec4<f32>,
    @location(2) bg_color: vec4<f32>,
    @location(3) flags: u32,
    @location(4) cell_uv: vec2<f32>,    // UV within the cell (0..1)
}

// Flag constants matching Attrs bitflags from jterm-vt
const FLAG_BOLD: u32          = 1u;
const FLAG_DIM: u32           = 2u;
const FLAG_ITALIC: u32        = 4u;
const FLAG_UNDERLINE: u32     = 8u;
const FLAG_REVERSE: u32       = 32u;
const FLAG_HIDDEN: u32        = 64u;
const FLAG_STRIKETHROUGH: u32 = 128u;
const FLAG_IS_CURSOR: u32     = 0x10000u;

// 6 vertices for a quad (two triangles)
// Vertex positions within a cell: (0,0), (1,0), (0,1), (1,0), (1,1), (0,1)
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
    instance: CellInstance,
) -> VertexOutput {
    let quad = QUAD_POS[vertex_index];

    // Cell position in NDC
    let cell_origin = uniforms.grid_offset + instance.grid_pos * uniforms.cell_size;
    // Flip Y: NDC goes from -1 (bottom) to +1 (top), but grid row 0 is at top
    let pos = vec2<f32>(
        cell_origin.x + quad.x * uniforms.cell_size.x,
        cell_origin.y - quad.y * uniforms.cell_size.y,
    );

    // Atlas UV (convert from texel coords to normalized 0..1)
    let uv = vec2<f32>(
        (instance.atlas_uv.x + quad.x * instance.atlas_uv.z) / uniforms.atlas_size.x,
        (instance.atlas_uv.y + quad.y * instance.atlas_uv.w) / uniforms.atlas_size.y,
    );

    var out: VertexOutput;
    out.position = vec4<f32>(pos, 0.0, 1.0);
    out.uv = uv;
    out.cell_uv = quad;
    out.flags = instance.flags;

    // Handle reverse video: swap fg and bg
    var fg = instance.fg_color;
    var bg = instance.bg_color;
    if (instance.flags & FLAG_REVERSE) != 0u {
        let tmp = fg;
        fg = bg;
        bg = tmp;
    }

    // Handle bold: brighten foreground
    if (instance.flags & FLAG_BOLD) != 0u {
        fg = vec4<f32>(min(fg.rgb * 1.2, vec3<f32>(1.0)), fg.a);
    }

    // Handle dim: darken foreground
    if (instance.flags & FLAG_DIM) != 0u {
        fg = vec4<f32>(fg.rgb * 0.6, fg.a);
    }

    out.fg_color = fg;
    out.bg_color = bg;

    return out;
}

@fragment
fn fs_main(in: VertexOutput) -> @location(0) vec4<f32> {
    // Hidden text: just show background
    if (in.flags & FLAG_HIDDEN) != 0u {
        return in.bg_color;
    }

    // Sample glyph alpha from atlas
    let glyph_alpha = textureSample(atlas_texture, atlas_sampler, in.uv).r;

    // Mix background and foreground based on glyph coverage
    var color = mix(in.bg_color.rgb, in.fg_color.rgb, glyph_alpha);
    var alpha = max(in.bg_color.a, glyph_alpha * in.fg_color.a);

    // Underline: draw a line near the bottom of the cell
    if (in.flags & FLAG_UNDERLINE) != 0u {
        if in.cell_uv.y > 0.875 && in.cell_uv.y < 0.9375 {
            color = in.fg_color.rgb;
            alpha = 1.0;
        }
    }

    // Strikethrough: draw a line through the middle of the cell
    if (in.flags & FLAG_STRIKETHROUGH) != 0u {
        if in.cell_uv.y > 0.46875 && in.cell_uv.y < 0.53125 {
            color = in.fg_color.rgb;
            alpha = 1.0;
        }
    }

    // Cursor rendering (block cursor fills the cell, underline/bar are partial)
    if (in.flags & FLAG_IS_CURSOR) != 0u {
        let cursor_shape = u32(uniforms.cursor_extra.y);
        let blink_on = uniforms.cursor_extra.z;
        let cursor_color = vec3<f32>(
            uniforms.cursor_pos.z,
            uniforms.cursor_pos.w,
            uniforms.cursor_extra.x,
        );

        if blink_on > 0.5 {
            if cursor_shape == 0u {
                // Block cursor: invert colors
                color = vec3<f32>(1.0) - color;
                alpha = 1.0;
            } else if cursor_shape == 1u {
                // Underline cursor
                if in.cell_uv.y > 0.875 {
                    color = cursor_color;
                    alpha = 1.0;
                }
            } else if cursor_shape == 2u {
                // Bar cursor
                if in.cell_uv.x < 0.1 {
                    color = cursor_color;
                    alpha = 1.0;
                }
            }
        }
    }

    return vec4<f32>(color, alpha);
}
