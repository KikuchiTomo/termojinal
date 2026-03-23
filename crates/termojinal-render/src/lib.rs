//! GPU terminal renderer for termojinal.
//!
//! Provides a wgpu-based renderer that draws terminal cells as textured quads
//! with a font atlas for glyph rendering.
//!
//! # Architecture
//!
//! - **Atlas**: CPU-side glyph rasterization using `fontdue`, packed into a
//!   single texture atlas uploaded to the GPU.
//! - **Renderer**: wgpu render pipeline using instanced rendering. Each terminal
//!   cell is one instance, drawn as a 6-vertex quad (two triangles).
//! - **Shader**: WGSL vertex + fragment shader handling cell positioning,
//!   glyph sampling, color application, and attribute effects (bold, dim,
//!   underline, strikethrough, reverse, cursor).
//! - **Color conversion**: Translates `termojinal_vt::Color` variants (Default,
//!   Named, Indexed, Rgb) to GPU-ready `[f32; 4]` RGBA values, including
//!   the full xterm 256-color palette.
//! - **Image rendering**: Separate pipeline for drawing inline images (Kitty
//!   Graphics, iTerm2 Inline Images, Sixel) as textured quads over cells.

pub mod atlas;
pub mod blur_renderer;
pub mod color_convert;
pub mod emoji_atlas;
pub mod image_render;
pub mod renderer;
pub mod rounded_rect_renderer;

pub use atlas::{CellSize, FontConfig};
pub use blur_renderer::BlurRenderer;
pub use color_convert::ThemePalette;
pub use image_render::ImageRenderer;
pub use renderer::{RenderError, Renderer, ScrollbarGeometry};
pub use rounded_rect_renderer::{RoundedRect, RoundedRectRenderer};
