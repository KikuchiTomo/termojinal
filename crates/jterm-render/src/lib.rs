//! GPU terminal renderer for jterm.
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
//! - **Color conversion**: Translates `jterm_vt::Color` variants (Default,
//!   Named, Indexed, Rgb) to GPU-ready `[f32; 4]` RGBA values, including
//!   the full xterm 256-color palette.

pub mod atlas;
pub mod color_convert;
pub mod renderer;

pub use atlas::{CellSize, FontConfig};
pub use renderer::{RenderError, Renderer};
