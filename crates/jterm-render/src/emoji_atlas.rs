//! Emoji atlas — color emoji rasterization via Core Text + texture packing.
//!
//! Uses Apple Color Emoji font through Core Text to render full-color (RGBA)
//! emoji glyphs. These are packed into a separate RGBA atlas texture that the
//! shader samples from when the FLAG_EMOJI bit is set on a cell instance.

use std::collections::HashMap;

use crate::atlas::GlyphInfo;

/// Returns `true` if the character should be rendered as a color emoji.
pub fn is_emoji(c: char) -> bool {
    matches!(c as u32,
        0x1F300..=0x1F9FF   // Miscellaneous Symbols and Pictographs, Emoticons, etc.
        | 0x2600..=0x27BF    // Miscellaneous Symbols, Dingbats
        | 0x1FA00..=0x1FA6F  // Chess Symbols
        | 0x1FA70..=0x1FAFF  // Symbols and Pictographs Extended-A
        | 0x231A..=0x231B    // Watch, Hourglass
        | 0x23E9..=0x23F3    // Various symbols
        | 0x23F8..=0x23FA
        | 0x25AA..=0x25AB
        | 0x25B6 | 0x25C0
        | 0x25FB..=0x25FE
    )
}

/// RGBA color emoji atlas.
///
/// Stores color emoji glyphs rasterized via Core Text in an RGBA8 bitmap.
/// The packing strategy mirrors the monochrome `Atlas`: each emoji occupies
/// a cell-sized slot (2 cells wide for full-width emoji).
pub struct EmojiAtlas {
    pub data: Vec<u8>,   // RGBA data (4 bytes per pixel)
    pub width: u32,
    pub height: u32,
    glyphs: HashMap<char, GlyphInfo>,
    cell_w: u32,
    cell_h: u32,
    font_size: f32,
    pack_x: u32,
    pack_y: u32,
    pack_row_height: u32,
}

impl EmojiAtlas {
    /// Create a new emoji atlas with the given cell dimensions.
    pub fn new(cell_w: u32, cell_h: u32, font_size: f32) -> Self {
        let atlas_width = 512u32;
        let atlas_height = 512u32;
        let data = vec![0u8; (atlas_width * atlas_height * 4) as usize];

        Self {
            data,
            width: atlas_width,
            height: atlas_height,
            glyphs: HashMap::new(),
            cell_w,
            cell_h,
            font_size,
            pack_x: 1,
            pack_y: 1,
            pack_row_height: 0,
        }
    }

    /// Look up a cached emoji glyph, or rasterize and cache it on demand.
    pub fn get_glyph(&mut self, c: char) -> Option<GlyphInfo> {
        if let Some(&info) = self.glyphs.get(&c) {
            return Some(info);
        }
        self.rasterize_emoji(c)
    }

    pub fn has_glyph(&self, c: char) -> bool {
        self.glyphs.contains_key(&c)
    }

    pub fn glyph_count(&self) -> usize {
        self.glyphs.len()
    }

    /// Rasterize an emoji character using Core Text and pack it into the atlas.
    #[cfg(target_os = "macos")]
    fn rasterize_emoji(&mut self, c: char) -> Option<GlyphInfo> {
        let (rgba, bmp_w, bmp_h) = rasterize_emoji_ct(c, self.font_size, self.cell_w, self.cell_h)?;

        // Emoji are typically 2 cells wide. Use 2x cell width as the entry size.
        let entry_w = self.cell_w * 2;
        let entry_h = self.cell_h;

        let padded_w = entry_w + 1;
        let padded_h = entry_h + 1;

        // Advance to next row if needed.
        if self.pack_x + padded_w > self.width {
            self.pack_x = 1;
            self.pack_y += self.pack_row_height + 1;
            self.pack_row_height = 0;
        }

        // Grow atlas if needed.
        if self.pack_y + padded_h > self.height {
            self.grow_atlas();
        }

        let atlas_x = self.pack_x;
        let atlas_y = self.pack_y;

        // Copy the RGBA bitmap into the atlas. The bitmap may be smaller or
        // equal to entry_w x entry_h, so center it.
        let offset_x = if bmp_w < entry_w { (entry_w - bmp_w) / 2 } else { 0 };
        let offset_y = if bmp_h < entry_h { (entry_h - bmp_h) / 2 } else { 0 };

        for row in 0..bmp_h.min(entry_h) {
            for col in 0..bmp_w.min(entry_w) {
                let src_idx = ((row * bmp_w + col) * 4) as usize;
                let dst_x = atlas_x + offset_x + col;
                let dst_y = atlas_y + offset_y + row;
                if dst_x < self.width && dst_y < self.height && src_idx + 3 < rgba.len() {
                    let dst_idx = ((dst_y * self.width + dst_x) * 4) as usize;
                    if dst_idx + 3 < self.data.len() {
                        self.data[dst_idx] = rgba[src_idx];
                        self.data[dst_idx + 1] = rgba[src_idx + 1];
                        self.data[dst_idx + 2] = rgba[src_idx + 2];
                        self.data[dst_idx + 3] = rgba[src_idx + 3];
                    }
                }
            }
        }

        // Advance packing cursor.
        self.pack_x += padded_w;
        self.pack_row_height = self.pack_row_height.max(padded_h);

        let info = GlyphInfo {
            atlas_x: atlas_x as f32,
            atlas_y: atlas_y as f32,
            atlas_w: entry_w as f32,
            atlas_h: entry_h as f32,
            bearing_x: 0.0,
            bearing_y: 0.0,
        };
        self.glyphs.insert(c, info);
        Some(info)
    }

    /// Fallback for non-macOS: emoji rasterization is not supported.
    #[cfg(not(target_os = "macos"))]
    fn rasterize_emoji(&mut self, _c: char) -> Option<GlyphInfo> {
        None
    }

    fn grow_atlas(&mut self) {
        let new_height = self.height * 2;
        let mut new_data = vec![0u8; (self.width * new_height * 4) as usize];
        new_data[..self.data.len()].copy_from_slice(&self.data);
        self.data = new_data;
        self.height = new_height;
        log::info!("emoji atlas grew to {}x{}", self.width, self.height);
    }
}

/// Rasterize a single emoji character using Core Text / Core Graphics.
///
/// Returns `(rgba_pixels, width, height)` where `rgba_pixels` is in
/// premultiplied-alpha RGBA format suitable for direct GPU upload.
#[cfg(target_os = "macos")]
fn rasterize_emoji_ct(
    c: char,
    font_size: f32,
    cell_w: u32,
    cell_h: u32,
) -> Option<(Vec<u8>, u32, u32)> {
    use core_foundation::base::TCFType;
    use core_graphics::base::{kCGBitmapByteOrder32Big, kCGImageAlphaPremultipliedLast};
    use core_graphics::color_space::CGColorSpace;
    use core_graphics::context::CGContext;
    use core_graphics::geometry::{CGPoint, CGRect, CGSize};
    use core_text::font as ct_font;
    use core_text::font_descriptor::kCTFontOrientationDefault;
    use foreign_types::ForeignType;

    // Target bitmap size: 2 cells wide (emoji are full-width), 1 cell tall.
    let bmp_w = cell_w * 2;
    let bmp_h = cell_h;

    // Create an Apple Color Emoji CTFont.
    let ct_font = ct_font::new_from_name("Apple Color Emoji", font_size as f64).ok()?;

    // Get the glyph index for this character.
    let mut utf16_buf = [0u16; 2];
    let utf16 = c.encode_utf16(&mut utf16_buf);
    let utf16_len = utf16.len();
    let mut glyphs = [0u16; 2];
    let found = unsafe {
        ct_font.get_glyphs_for_characters(
            utf16_buf.as_ptr(),
            glyphs.as_mut_ptr(),
            utf16_len as core_foundation::base::CFIndex,
        )
    };
    if !found || glyphs[0] == 0 {
        return None;
    }

    // Create an RGBA CGContext.
    let color_space = CGColorSpace::create_device_rgb();
    let mut ctx = CGContext::create_bitmap_context(
        None,
        bmp_w as usize,
        bmp_h as usize,
        8,                          // bits per component
        bmp_w as usize * 4,        // bytes per row
        &color_space,
        kCGImageAlphaPremultipliedLast | kCGBitmapByteOrder32Big,
    );

    // Clear to transparent.
    ctx.set_rgb_fill_color(0.0, 0.0, 0.0, 0.0);
    ctx.fill_rect(CGRect::new(
        &CGPoint::new(0.0, 0.0),
        &CGSize::new(bmp_w as f64, bmp_h as f64),
    ));

    let glyph_id = glyphs[0];

    // Get glyph bounding box for positioning.
    let bbox = ct_font.get_bounding_rects_for_glyphs(
        kCTFontOrientationDefault,
        &[glyph_id],
    );

    let glyph_render_w = bbox.size.width;
    let glyph_render_h = bbox.size.height;

    // Scale factor: fit the emoji into the cell size.
    let scale = if glyph_render_w > 0.0 && glyph_render_h > 0.0 {
        let scale_x = bmp_w as f64 / glyph_render_w;
        let scale_y = bmp_h as f64 / glyph_render_h;
        scale_x.min(scale_y).min(1.0) // Don't upscale
    } else {
        1.0
    };

    // Position: center the glyph.
    let rendered_w = glyph_render_w * scale;
    let rendered_h = glyph_render_h * scale;
    let x_offset = (bmp_w as f64 - rendered_w) / 2.0 - bbox.origin.x * scale;
    let y_offset = (bmp_h as f64 - rendered_h) / 2.0 - bbox.origin.y * scale;

    // Apply scaling.
    ctx.scale(scale, scale);
    let pos = CGPoint::new(x_offset / scale, y_offset / scale);

    // Draw the glyph using the raw FFI to avoid consuming the context.
    extern "C" {
        fn CTFontDrawGlyphs(
            font: core_text::font::CTFontRef,
            glyphs: *const u16,
            positions: *const CGPoint,
            count: usize,
            context: *mut core_graphics::sys::CGContext,
        );
    }

    unsafe {
        CTFontDrawGlyphs(
            ct_font.as_concrete_TypeRef(),
            &glyph_id as *const u16,
            &pos as *const CGPoint,
            1,
            ctx.as_ptr(),
        );
    }

    // Extract pixel data. Core Graphics gives us RGBA with premultiplied alpha.
    let cg_data = ctx.data();
    let pixel_count = (bmp_w * bmp_h) as usize;
    let mut rgba = vec![0u8; pixel_count * 4];

    // Core Graphics bitmap origin is bottom-left; flip vertically.
    for row in 0..bmp_h as usize {
        let src_row = (bmp_h as usize - 1) - row;
        let src_offset = src_row * bmp_w as usize * 4;
        let dst_offset = row * bmp_w as usize * 4;
        let row_bytes = bmp_w as usize * 4;
        if src_offset + row_bytes <= cg_data.len() && dst_offset + row_bytes <= rgba.len() {
            rgba[dst_offset..dst_offset + row_bytes]
                .copy_from_slice(&cg_data[src_offset..src_offset + row_bytes]);
        }
    }

    Some((rgba, bmp_w, bmp_h))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_emoji() {
        // Common emoji
        assert!(is_emoji('\u{1F600}')); // grinning face
        assert!(is_emoji('\u{1F680}')); // rocket
        assert!(is_emoji('\u{2764}'));  // heavy heart (in Dingbats range)
        assert!(is_emoji('\u{1F4A9}')); // pile of poo

        // Not emoji
        assert!(!is_emoji('A'));
        assert!(!is_emoji('z'));
        assert!(!is_emoji('0'));
        assert!(!is_emoji(' '));
    }

    #[test]
    fn test_emoji_atlas_creation() {
        let atlas = EmojiAtlas::new(8, 16, 14.0);
        assert_eq!(atlas.width, 512);
        assert_eq!(atlas.height, 512);
        assert_eq!(atlas.glyph_count(), 0);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn test_emoji_rasterization() {
        let mut atlas = EmojiAtlas::new(8, 16, 14.0);
        let glyph = atlas.get_glyph('\u{1F600}'); // grinning face
        assert!(
            glyph.is_some(),
            "Should be able to rasterize a common emoji"
        );
        assert_eq!(atlas.glyph_count(), 1);

        // Requesting the same emoji again should return the cached glyph.
        let glyph2 = atlas.get_glyph('\u{1F600}');
        assert!(glyph2.is_some());
        assert_eq!(atlas.glyph_count(), 1);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn test_multiple_emoji() {
        let mut atlas = EmojiAtlas::new(8, 16, 14.0);
        let emojis = ['\u{1F600}', '\u{1F680}', '\u{1F4A9}', '\u{2764}'];
        for &c in &emojis {
            let _ = atlas.get_glyph(c);
        }
        assert!(atlas.glyph_count() >= 1);
    }
}
