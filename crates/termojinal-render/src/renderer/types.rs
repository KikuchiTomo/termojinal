//! Types, constants, and error definitions for the renderer.

/// Per-pane dirty rendering cache. Keyed by an opaque pane identifier.
pub(crate) type PaneKey = u64;

#[derive(Default)]
pub(crate) struct PaneCache {
    pub(crate) instances: Vec<CellInstance>,
    pub(crate) instance_count: usize,
    pub(crate) scroll_offset: usize,
    pub(crate) grid_dims: (usize, usize),
    pub(crate) selection: Option<((usize, usize), (usize, usize))>,
    pub(crate) row_instance_counts: Vec<usize>,
    /// Cached search matches so we can detect changes and force rebuild.
    pub(crate) search_matches: Option<Vec<(usize, usize, usize)>>,
    pub(crate) search_current_idx: Option<usize>,
    /// Cached link hover range (row, col_start, col_end) for change detection.
    pub(crate) link_hover: Option<(usize, usize, usize)>,
}

/// Per-cell instance data sent to the GPU.
///
/// Each cell is one instance; the vertex shader generates a quad from 6 vertices.
#[repr(C)]
#[derive(Clone, Copy, Debug, bytemuck::Pod, bytemuck::Zeroable)]
pub(crate) struct CellInstance {
    /// Grid position (column, row).
    pub(crate) grid_pos: [f32; 2],
    /// Atlas UV region: (x, y, w, h) in texels.
    pub(crate) atlas_uv: [f32; 4],
    /// Foreground color RGBA.
    pub(crate) fg_color: [f32; 4],
    /// Background color RGBA.
    pub(crate) bg_color: [f32; 4],
    /// Attribute flags (matches termojinal_vt::Attrs bits).
    pub(crate) flags: u32,
    /// Cell width multiplier (1.0 for normal, 2.0 for wide CJK chars).
    pub(crate) cell_width_scale: f32,
    /// Padding.
    pub(crate) _pad: [u32; 2],
}

/// Uniform data for the shader.
#[repr(C)]
#[derive(Clone, Copy, Debug, bytemuck::Pod, bytemuck::Zeroable)]
pub(crate) struct Uniforms {
    /// Cell size in NDC: (width, height).
    pub(crate) cell_size: [f32; 2],
    /// Grid offset in NDC (top-left corner).
    pub(crate) grid_offset: [f32; 2],
    /// Atlas texture size: (width, height).
    pub(crate) atlas_size: [f32; 2],
    /// Emoji atlas texture size: (width, height).
    pub(crate) emoji_atlas_size: [f32; 2],
    /// cursor_pos: (col, row, cursor_color_r, cursor_color_g)
    pub(crate) cursor_pos: [f32; 4],
    /// cursor_extra: (cursor_color_b, cursor_shape, blink_on, _pad)
    pub(crate) cursor_extra: [f32; 4],
}

/// Scrollbar geometry in pixel coordinates relative to the pane origin.
#[derive(Debug, Clone, Copy)]
pub struct ScrollbarGeometry {
    /// X position of the scrollbar track in pixels from the pane left edge.
    pub track_x: f32,
    /// Width of the scrollbar track in pixels.
    pub track_width: f32,
    /// Y position of the top of the thumb in pixels from the pane top.
    pub thumb_top: f32,
    /// Y position of the bottom of the thumb in pixels from the pane top.
    pub thumb_bottom: f32,
    /// Total height of the scrollbar track in pixels.
    pub total_height: f32,
    /// Number of visible rows.
    pub rows: usize,
    /// Number of scrollback lines.
    pub scrollback_len: usize,
}

/// Flag indicating this cell has an underline (matches Attrs::UNDERLINE bit).
pub(crate) const FLAG_UNDERLINE: u32 = 1 << 3;

/// Flag indicating this cell is the cursor cell.
pub(crate) const FLAG_IS_CURSOR: u32 = 0x10000;

/// Flag indicating this cell is selected (for selection highlighting).
pub(crate) const FLAG_SELECTED: u32 = 0x20000;

/// Flag indicating this cell contains an emoji rendered via the color emoji atlas.
pub(crate) const FLAG_EMOJI: u32 = 0x40000;

/// Flag indicating this cell is a search match highlight.
pub(crate) const FLAG_SEARCH: u32 = 0x80000;

/// Flag indicating this cell is the *current* (focused) search match highlight.
pub(crate) const FLAG_SEARCH_CURRENT: u32 = 0x100000;

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
