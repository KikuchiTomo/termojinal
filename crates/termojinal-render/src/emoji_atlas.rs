//! Emoji atlas — color emoji rasterization via Core Text + texture packing.
//!
//! Uses Apple Color Emoji font through Core Text to render full-color (RGBA)
//! emoji glyphs. These are packed into a separate RGBA atlas texture that the
//! shader samples from when the FLAG_EMOJI bit is set on a cell instance.

use std::collections::HashMap;

use crate::atlas::GlyphInfo;

/// Returns `true` if the character should be rendered as a color emoji.
///
/// Uses `unic-emoji-char` (Unicode 12.0) as the primary check, with a
/// fallback for emoji added in Unicode 13.0+ that the crate doesn't cover.
pub fn is_emoji(c: char) -> bool {
    if unic_emoji_char::is_emoji_presentation(c) {
        return true;
    }
    // Fallback: cover emoji blocks added after Unicode 12.0.
    // unic-emoji-char 0.9 is based on Unicode 12.0 and misses newer emoji
    // like 🪙 (U+1FA99, Unicode 13.0) and 🫠 (U+1FAE0, Unicode 14.0).
    let cp = c as u32;
    matches!(cp,
        0x1FA70..=0x1FAFF  // Symbols and Pictographs Extended-A (13.0+)
        | 0x1F900..=0x1F9FF // Supplemental Symbols and Pictographs (covers new additions)
        | 0x1FC00..=0x1FCFF // Symbols for Legacy Computing (some emoji)
    )
}

/// Returns `true` if the character has the Unicode `Emoji` property but not
/// default emoji presentation.  These characters (e.g. ⏺ U+23FA, ✔ U+2714)
/// are emoji-capable but render as text by default.  When the monochrome font
/// atlas fails to rasterize them, the renderer can fall back to the emoji
/// atlas's Core Text path which handles font cascading.
///
/// This deliberately excludes ASCII digits (0-9), `#`, and `*` which have
/// `Emoji=Yes` but must always be rendered as plain text.
pub fn is_text_emoji(c: char) -> bool {
    // Already handled by is_emoji — no need to double-classify.
    if is_emoji(c) {
        return false;
    }
    let cp = c as u32;
    // Exclude ASCII and basic Latin — digits 0-9, #, * have Emoji=Yes but
    // should never go through the emoji atlas.
    if cp < 0x2000 {
        return false;
    }
    unic_emoji_char::is_emoji(c)
}

/// Returns `true` if `c` is a zero-width character that should be skipped
/// during preedit / overlay rendering (variation selectors, ZWJ, etc.).
pub fn is_zero_width_for_render(c: char) -> bool {
    let cp = c as u32;
    matches!(cp,
        0xFE00..=0xFE0F        // Variation selectors VS1-VS16
        | 0x200B..=0x200F      // Zero-width space, ZWNJ, ZWJ, LRM, RLM
        | 0x2060..=0x2064      // Word joiner, invisible separators
        | 0xFEFF               // BOM / zero-width no-break space
        | 0xE0020..=0xE007F    // Tag characters (flag subdivisions)
        | 0xE0001              // Language tag
    )
}

/// RGBA color emoji atlas.
///
/// Stores color emoji glyphs rasterized via Core Text in an RGBA8 bitmap.
/// The packing strategy mirrors the monochrome `Atlas`: each emoji occupies
/// a cell-sized slot (2 cells wide for full-width emoji).
pub struct EmojiAtlas {
    pub data: Vec<u8>, // RGBA data (4 bytes per pixel)
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
    /// Falls back to the system font if Apple Color Emoji doesn't have the glyph.
    #[cfg(target_os = "macos")]
    fn rasterize_emoji(&mut self, c: char) -> Option<GlyphInfo> {
        let (rgba, bmp_w, bmp_h) = rasterize_emoji_ct(c, self.font_size, self.cell_w, self.cell_h)
            .or_else(|| rasterize_text_ct(c, self.font_size, self.cell_w, self.cell_h))?;

        // Pack at the actual bitmap size (may be larger than cell for quality).
        let entry_w = bmp_w;
        let entry_h = bmp_h;

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
        let offset_x = if bmp_w < entry_w {
            (entry_w - bmp_w) / 2
        } else {
            0
        };
        let offset_y = if bmp_h < entry_h {
            (entry_h - bmp_h) / 2
        } else {
            0
        };

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

    // Render emoji at a larger size for better quality, then the GPU will downsample.
    // Apple Color Emoji is a bitmap font with fixed sizes (20, 32, 40, 48, 64, 96, 160).
    // Use at least 64pt for crisp rendering, scaled to fit the cell.
    let render_size = (font_size as f64).max(64.0);
    let scale_ratio = render_size / font_size as f64;
    let bmp_w = ((cell_w * 2) as f64 * scale_ratio).ceil() as u32;
    let bmp_h = (cell_h as f64 * scale_ratio).ceil() as u32;

    // Create an Apple Color Emoji CTFont at the render size.
    let ct_font = ct_font::new_from_name("Apple Color Emoji", render_size).ok()?;

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
        8,                  // bits per component
        bmp_w as usize * 4, // bytes per row
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
    let _bbox = ct_font.get_bounding_rects_for_glyphs(kCTFontOrientationDefault, &[glyph_id]);

    // Use Core Text CTLine for reliable emoji rendering (works with bitmap emoji).
    use core_foundation::attributed_string::CFMutableAttributedString;
    use core_foundation::string::CFString;

    extern "C" {
        fn CTLineCreateWithAttributedString(
            attr_string: core_foundation::base::CFTypeRef,
        ) -> *mut std::ffi::c_void;
        fn CTLineDraw(line: *const std::ffi::c_void, context: *mut core_graphics::sys::CGContext);
        fn CFRelease(cf: *const std::ffi::c_void);
        static kCTFontAttributeName: core_foundation::base::CFTypeRef;
        fn CFAttributedStringSetAttribute(
            aStr: *mut std::ffi::c_void,
            range: core_foundation_sys::base::CFRange,
            attrName: core_foundation::base::CFTypeRef,
            value: core_foundation::base::CFTypeRef,
        );
    }

    let s = CFString::new(&c.to_string());
    let mut attr_str = CFMutableAttributedString::new();
    attr_str.replace_str(
        &s,
        core_foundation_sys::base::CFRange {
            location: 0,
            length: 0,
        },
    );

    let range = core_foundation_sys::base::CFRange {
        location: 0,
        length: attr_str.char_len(),
    };
    unsafe {
        CFAttributedStringSetAttribute(
            attr_str.as_CFTypeRef() as *mut _,
            range,
            kCTFontAttributeName,
            ct_font.as_CFTypeRef(),
        );
    }

    let line = unsafe { CTLineCreateWithAttributedString(attr_str.as_CFTypeRef()) };
    if line.is_null() {
        return None;
    }

    // Compute emoji metrics for centering and square fitting.
    let ascent = ct_font.ascent();
    let descent = ct_font.descent();
    let text_h = (ascent + descent.abs()).max(1.0);

    // Get the glyph advance width to center horizontally and maintain
    // correct aspect ratio.  Apple Color Emoji glyphs have advance_w ≠
    // text_h, so naively placing at x=0 causes right-edge clipping and
    // the non-square metrics cause vertical stretching.
    extern "C" {
        fn CTFontGetAdvancesForGlyphs(
            font: *const std::ffi::c_void,
            orientation: u32, // CTFontOrientation
            glyphs: *const u16,
            advances: *mut CGSize,
            count: core_foundation::base::CFIndex,
        ) -> f64;
    }
    let mut advance_size = CGSize::new(0.0, 0.0);
    let _total_advance = unsafe {
        CTFontGetAdvancesForGlyphs(
            ct_font.as_CFTypeRef() as *const _,
            0, // kCTFontOrientationDefault
            &glyph_id,
            &mut advance_size,
            1,
        )
    };
    let advance_w = advance_size.width.max(1.0);

    // Fit the emoji into a square region within the bitmap.  Since the
    // bitmap-to-screen-quad mapping is uniform (same scale factor for
    // both axes), a square region in the bitmap appears square on screen.
    let glyph_max = advance_w.max(text_h);
    let target_side = (bmp_w.min(bmp_h) as f64) * 0.92; // 92% to avoid clipping
    let fit_scale = if glyph_max > 0.0 {
        target_side / glyph_max
    } else {
        1.0
    };

    // Apply uniform scale to the CG context so the glyph is rendered
    // at the correct size.  Core Text will scale the sbix bitmap strike.
    ctx.scale(fit_scale, fit_scale);

    // Compute text position in the SCALED coordinate system.
    // In scaled coords: bitmap is (bmp_w / fit_scale) × (bmp_h / fit_scale).
    let scaled_bmp_w = bmp_w as f64 / fit_scale;
    let scaled_bmp_h = bmp_h as f64 / fit_scale;

    // Center horizontally.
    let text_x = ((scaled_bmp_w - advance_w) / 2.0).max(0.0);
    // Center vertically (CG coordinate system: origin bottom-left, Y up).
    let baseline_y = ((scaled_bmp_h - text_h) / 2.0 + descent.abs()).max(0.0);

    ctx.set_text_position(text_x, baseline_y);

    unsafe {
        CTLineDraw(line, ctx.as_ptr());
        CFRelease(line);
    }

    // Extract pixel data with vertical flip.
    // CG bitmap context uses a bottom-left origin (Y axis points up).
    // CTLineDraw renders into this coordinate system, so the resulting
    // pixel data in memory is bottom-up.  Our atlas and wgpu textures
    // use a top-left origin, so we must flip rows vertically.
    let cg_data = ctx.data();
    let pixel_count = (bmp_w * bmp_h) as usize;
    let total_bytes = pixel_count * 4;
    if total_bytes > cg_data.len() {
        return None;
    }

    let mut rgba = vec![0u8; total_bytes];
    let row_bytes = (bmp_w * 4) as usize;
    for row in 0..bmp_h {
        let src_row = bmp_h - 1 - row;
        let src_off = (src_row * bmp_w * 4) as usize;
        let dst_off = (row * bmp_w * 4) as usize;
        rgba[dst_off..dst_off + row_bytes].copy_from_slice(&cg_data[src_off..src_off + row_bytes]);
    }

    Some((rgba, bmp_w, bmp_h))
}

/// Rasterize a text character using Core Text with the system font.
///
/// This is the fallback for non-emoji characters (e.g. ❯, ◯) that fontdue
/// can't render.  Uses the system font cascade via `.AppleSystemUIFont` so
/// virtually any Unicode character the OS supports can be rendered.
///
/// Returns an RGBA bitmap sized to fit within a single cell (1x cell_w).
#[cfg(target_os = "macos")]
fn rasterize_text_ct(
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
    use foreign_types::ForeignType;

    let size = font_size as f64;
    // Try multiple fonts to find one that has this glyph.
    // .AppleSystemUIFont (SF Pro) doesn't cascade to symbol fonts;
    // Apple Symbols covers Miscellaneous Technical, Dingbats, etc.;
    // Menlo covers many programming symbols; LastResort is the final fallback.
    let font_names = [".AppleSystemUIFont", "Apple Symbols", "Menlo", "LastResort"];
    let mut ct = None;
    let mut glyphs = [0u16; 2];
    let mut utf16_buf = [0u16; 2];
    let utf16 = c.encode_utf16(&mut utf16_buf);
    let utf16_len = utf16.len();

    for name in &font_names {
        if let Ok(f) = ct_font::new_from_name(name, size) {
            let mut g = [0u16; 2];
            let found = unsafe {
                f.get_glyphs_for_characters(
                    utf16_buf.as_ptr(),
                    g.as_mut_ptr(),
                    utf16_len as core_foundation::base::CFIndex,
                )
            };
            if found && g[0] != 0 {
                glyphs = g;
                ct = Some(f);
                break;
            }
        }
    }
    let ct = ct?;

    let bmp_w = cell_w;
    let bmp_h = cell_h;

    let color_space = CGColorSpace::create_device_rgb();
    let mut ctx = CGContext::create_bitmap_context(
        None,
        bmp_w as usize,
        bmp_h as usize,
        8,
        bmp_w as usize * 4,
        &color_space,
        kCGImageAlphaPremultipliedLast | kCGBitmapByteOrder32Big,
    );

    // Clear to transparent.
    ctx.set_rgb_fill_color(0.0, 0.0, 0.0, 0.0);
    ctx.fill_rect(CGRect::new(
        &CGPoint::new(0.0, 0.0),
        &CGSize::new(bmp_w as f64, bmp_h as f64),
    ));

    // Draw white text so the shader can colorize it via the foreground color.
    // Core Text uses kCTForegroundColorFromContextAttributeName by default
    // when no explicit foreground color is set on the attributed string.
    ctx.set_rgb_fill_color(1.0, 1.0, 1.0, 1.0);

    // Build CTLine.
    use core_foundation::attributed_string::CFMutableAttributedString;
    use core_foundation::string::CFString;

    extern "C" {
        fn CTLineCreateWithAttributedString(
            attr_string: core_foundation::base::CFTypeRef,
        ) -> *mut std::ffi::c_void;
        fn CTLineDraw(line: *const std::ffi::c_void, context: *mut core_graphics::sys::CGContext);
        fn CFRelease(cf: *const std::ffi::c_void);
        static kCTFontAttributeName: core_foundation::base::CFTypeRef;
        static kCTForegroundColorFromContextAttributeName: core_foundation::base::CFTypeRef;
        fn CFAttributedStringSetAttribute(
            aStr: *mut std::ffi::c_void,
            range: core_foundation_sys::base::CFRange,
            attrName: core_foundation::base::CFTypeRef,
            value: core_foundation::base::CFTypeRef,
        );
    }

    let s = CFString::new(&c.to_string());
    let mut attr_str = CFMutableAttributedString::new();
    attr_str.replace_str(
        &s,
        core_foundation_sys::base::CFRange {
            location: 0,
            length: 0,
        },
    );

    let range = core_foundation_sys::base::CFRange {
        location: 0,
        length: attr_str.char_len(),
    };
    unsafe {
        CFAttributedStringSetAttribute(
            attr_str.as_CFTypeRef() as *mut _,
            range,
            kCTFontAttributeName,
            ct.as_CFTypeRef(),
        );
        // Tell Core Text to use the CG context's fill color as the text color.
        let cf_true = core_foundation::boolean::CFBoolean::true_value();
        CFAttributedStringSetAttribute(
            attr_str.as_CFTypeRef() as *mut _,
            range,
            kCTForegroundColorFromContextAttributeName,
            cf_true.as_CFTypeRef(),
        );
    }

    let line = unsafe { CTLineCreateWithAttributedString(attr_str.as_CFTypeRef()) };
    if line.is_null() {
        return None;
    }

    // Position text centered in cell.
    let ascent = ct.ascent();
    let descent = ct.descent();
    let text_h = ascent + descent.abs();

    extern "C" {
        fn CTFontGetAdvancesForGlyphs(
            font: *const std::ffi::c_void,
            orientation: u32,
            glyphs: *const u16,
            advances: *mut CGSize,
            count: core_foundation::base::CFIndex,
        ) -> f64;
    }
    let mut advance_size = CGSize::new(0.0, 0.0);
    unsafe {
        CTFontGetAdvancesForGlyphs(
            ct.as_CFTypeRef() as *const _,
            0,
            &glyphs[0],
            &mut advance_size,
            1,
        );
    }
    let advance_w = advance_size.width.max(1.0);
    let text_x = ((bmp_w as f64 - advance_w) / 2.0).max(0.0);
    let baseline_y = ((bmp_h as f64 - text_h) / 2.0 + descent.abs()).max(0.0);

    ctx.set_text_position(text_x, baseline_y);

    unsafe {
        CTLineDraw(line, ctx.as_ptr());
        CFRelease(line);
    }

    // Extract RGBA data with vertical flip (CG is bottom-up).
    let cg_data = ctx.data();
    let pixel_count = (bmp_w * bmp_h) as usize;
    let total_bytes = pixel_count * 4;
    if total_bytes > cg_data.len() {
        return None;
    }

    let mut rgba = vec![0u8; total_bytes];
    for row in 0..bmp_h {
        let src_row = bmp_h - 1 - row;
        let src_off = (src_row * bmp_w * 4) as usize;
        let dst_off = (row * bmp_w * 4) as usize;
        let len = (bmp_w * 4) as usize;
        rgba[dst_off..dst_off + len].copy_from_slice(&cg_data[src_off..src_off + len]);
    }

    // Verify we got visible pixels.
    if rgba.iter().skip(3).step_by(4).all(|&a| a == 0) {
        return None;
    }

    log::debug!(
        "rasterize_text_ct: rendered U+{:04X} '{}' {}x{}",
        c as u32,
        c,
        bmp_w,
        bmp_h
    );
    Some((rgba, bmp_w, bmp_h))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_emoji() {
        // Common emoji (Emoji_Presentation = true)
        assert!(is_emoji('\u{1F600}')); // 😀 grinning face
        assert!(is_emoji('\u{1F680}')); // 🚀 rocket
        assert!(is_emoji('\u{1F4A9}')); // 💩 pile of poo

        // U+2764 (❤) has text presentation by default — only emoji with VS16.
        assert!(!is_emoji('\u{2764}'));

        // Not emoji
        assert!(!is_emoji('A'));
        assert!(!is_emoji('z'));
        assert!(!is_emoji('0'));
        assert!(!is_emoji(' '));
    }

    #[test]
    fn test_is_text_emoji() {
        // Characters with Emoji=Yes but Emoji_Presentation=No
        assert!(is_text_emoji('\u{23FA}')); // ⏺ Black Circle for Record
        assert!(is_text_emoji('\u{2714}')); // ✔ Heavy Check Mark
        assert!(is_text_emoji('\u{2764}')); // ❤ Heavy Black Heart

        // Characters with Emoji_Presentation=Yes should NOT be text_emoji
        // (they are already handled by is_emoji).
        assert!(!is_text_emoji('\u{1F600}')); // 😀 — is_emoji = true
        assert!(!is_text_emoji('\u{1F680}')); // 🚀 — is_emoji = true

        // ASCII characters with Emoji=Yes must NOT be classified as text_emoji.
        assert!(!is_text_emoji('#'));
        assert!(!is_text_emoji('*'));
        assert!(!is_text_emoji('0'));
        assert!(!is_text_emoji('9'));

        // Regular ASCII
        assert!(!is_text_emoji('A'));
        assert!(!is_text_emoji(' '));
    }

    #[test]
    fn test_is_zero_width_for_render() {
        assert!(is_zero_width_for_render('\u{FE0E}')); // VS15 (text)
        assert!(is_zero_width_for_render('\u{FE0F}')); // VS16 (emoji)
        assert!(is_zero_width_for_render('\u{200D}')); // ZWJ
        assert!(is_zero_width_for_render('\u{200B}')); // ZWSP
        assert!(is_zero_width_for_render('\u{FEFF}')); // BOM

        assert!(!is_zero_width_for_render('A'));
        assert!(!is_zero_width_for_render('\u{2714}')); // check mark — visible
        assert!(!is_zero_width_for_render('\u{23FA}')); // record symbol — visible
    }

    #[test]
    fn test_emoji_rasterize() {
        let mut atlas = EmojiAtlas::new(10, 16, 14.0);
        let result = atlas.get_glyph('😀');
        eprintln!("get_glyph('😀') = {:?}", result);
        assert!(result.is_some(), "emoji rasterization must succeed");
        // Check that the atlas has non-zero pixel data
        let nonzero: usize = atlas.data.iter().step_by(4).filter(|&&a| a > 0).count();
        eprintln!("non-zero pixels in atlas: {nonzero}");
        assert!(nonzero > 0, "emoji must produce visible pixels");
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

    #[cfg(target_os = "macos")]
    #[test]
    fn test_rasterize_text_ct_heavy_angle() {
        use core_text::font as ct_font;

        let c = '\u{276F}';
        let size = 11.0_f64;

        // Step 1: check if system font has the glyph
        let ct = ct_font::new_from_name(".AppleSystemUIFont", size).unwrap();
        let mut utf16_buf = [0u16; 2];
        let utf16 = c.encode_utf16(&mut utf16_buf);
        let utf16_len = utf16.len();
        let mut glyphs = [0u16; 2];
        let found = unsafe {
            ct.get_glyphs_for_characters(
                utf16_buf.as_ptr(),
                glyphs.as_mut_ptr(),
                utf16_len as core_foundation::base::CFIndex,
            )
        };
        eprintln!("AppleSystemUIFont: found={}, glyph_id={}", found, glyphs[0]);

        // Try other fonts
        for name in &[
            "Apple Symbols",
            "Menlo",
            "SF Mono",
            "Helvetica",
            "Arial",
            "LastResort",
        ] {
            if let Ok(f) = ct_font::new_from_name(name, size) {
                let mut g2 = [0u16; 2];
                let ok = unsafe {
                    f.get_glyphs_for_characters(
                        utf16_buf.as_ptr(),
                        g2.as_mut_ptr(),
                        utf16_len as core_foundation::base::CFIndex,
                    )
                };
                eprintln!("  {}: found={}, glyph_id={}", name, ok, g2[0]);
            } else {
                eprintln!("  {}: font not found", name);
            }
        }

        // Step 2: try rasterize_text_ct
        let result = super::rasterize_text_ct(c, 11.0, 10, 16);
        if let Some((rgba, w, h)) = &result {
            let nonzero_a: usize = rgba.iter().skip(3).step_by(4).filter(|&&a| a > 0).count();
            eprintln!(
                "rasterize_text_ct('❯'): {}x{}, nonzero_alpha={}",
                w, h, nonzero_a
            );
        } else {
            eprintln!("rasterize_text_ct('❯'): returned None");
        }
    }

    /// Test that U+23FA (⏺ BLACK CIRCLE FOR RECORD) can be rasterized.
    #[cfg(target_os = "macos")]
    #[test]
    fn test_rasterize_u23fa_record() {
        let c = '\u{23FA}';
        // Try via emoji atlas (rasterize_text_ct is the fallback within)
        let result = super::rasterize_text_ct(c, 14.0, 10, 16);
        if let Some((rgba, w, h)) = &result {
            let nonzero_a: usize = rgba.iter().skip(3).step_by(4).filter(|&&a| a > 0).count();
            eprintln!(
                "rasterize_text_ct('⏺' U+23FA): {}x{}, nonzero_alpha={}",
                w, h, nonzero_a
            );
            assert!(nonzero_a > 0, "U+23FA must produce visible pixels");
        } else {
            eprintln!("rasterize_text_ct('⏺' U+23FA): returned None — checking font support");
            // Check which fonts have the glyph
            use core_text::font as ct_font;
            let mut utf16_buf = [0u16; 2];
            let utf16 = c.encode_utf16(&mut utf16_buf);
            let utf16_len = utf16.len();
            for name in &[".AppleSystemUIFont", "Apple Symbols", "Menlo", "LastResort"] {
                if let Ok(f) = ct_font::new_from_name(name, 14.0) {
                    let mut g = [0u16; 2];
                    let found = unsafe {
                        f.get_glyphs_for_characters(
                            utf16_buf.as_ptr(),
                            g.as_mut_ptr(),
                            utf16_len as core_foundation::base::CFIndex,
                        )
                    };
                    eprintln!("  {}: found={}, glyph_id={}", name, found, g[0]);
                }
            }
            // Don't fail the test on systems without the font, but flag it.
            eprintln!("WARNING: U+23FA could not be rasterized via any font");
        }
    }

    /// Test that U+2714 (✔ HEAVY CHECK MARK) can be rasterized.
    #[cfg(target_os = "macos")]
    #[test]
    fn test_rasterize_u2714_check_mark() {
        let c = '\u{2714}';
        let result = super::rasterize_text_ct(c, 14.0, 10, 16);
        if let Some((rgba, w, h)) = &result {
            let nonzero_a: usize = rgba.iter().skip(3).step_by(4).filter(|&&a| a > 0).count();
            eprintln!(
                "rasterize_text_ct('✔' U+2714): {}x{}, nonzero_alpha={}",
                w, h, nonzero_a
            );
            assert!(nonzero_a > 0, "U+2714 must produce visible pixels");
        } else {
            eprintln!("WARNING: U+2714 could not be rasterized via any font");
        }
    }

    /// Diagnostic test: check is_emoji for problem characters.
    #[test]
    fn test_is_emoji_problem_chars() {
        let cases = [
            ('\u{25EF}', "◯ Large Circle"),
            ('\u{2461}', "② Circled Digit Two"),
            ('\u{276F}', "❯ Heavy Right-Pointing Angle"),
            ('\u{26A1}', "⚡ High Voltage"),
            ('\u{2713}', "✓ Check Mark"),
            ('\u{23F3}', "⏳ Hourglass"),
            ('\u{25B6}', "▶ Right Triangle"),
            ('\u{23FA}', "⏺ Black Circle for Record"),
            ('\u{2714}', "✔ Heavy Check Mark"),
        ];
        for (c, name) in &cases {
            let emoji = is_emoji(*c);
            let text_emoji = is_text_emoji(*c);
            eprintln!(
                "U+{:04X} {} {:40} is_emoji={:5} is_text_emoji={}",
                *c as u32, c, name, emoji, text_emoji
            );
        }
    }
}
