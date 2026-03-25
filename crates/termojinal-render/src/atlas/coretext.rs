//! Core Text fallback for glyph rendering on macOS.

use super::{Atlas, GlyphInfo};

impl Atlas {
    /// Last-resort fallback: rasterize a text character using Core Text.
    ///
    /// This catches any character that fontdue and all fallback fonts fail to
    /// render (e.g. ❯ U+276F, certain dingbats, rare symbols).  Core Text
    /// handles font cascading automatically and can render virtually any
    /// character that the system has a font for.
    #[cfg(target_os = "macos")]
    pub(crate) fn try_core_text_fallback(
        &mut self,
        c: char,
        entry_w: u32,
        entry_h: u32,
    ) -> Option<GlyphInfo> {
        use core_foundation::base::TCFType;
        use core_graphics::base::{kCGBitmapByteOrder32Big, kCGImageAlphaPremultipliedLast};
        use core_graphics::color_space::CGColorSpace;
        use core_graphics::context::CGContext;
        use core_graphics::geometry::{CGPoint, CGRect, CGSize};
        use core_text::font as ct_font;
        use foreign_types::ForeignType;

        // Use the same font size as the atlas.
        let size = self.font_size as f64;

        // Try multiple fonts to find one that has this glyph.
        let font_names = [".AppleSystemUIFont", "Apple Symbols", "Menlo", "LastResort"];
        let mut ct_found = None;
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
                    ct_found = Some(f);
                    break;
                }
            }
        }
        let ct = ct_found?;

        // Render into an RGBA bitmap, then extract grayscale from alpha.
        let bmp_w = entry_w;
        let bmp_h = entry_h;
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

        // Draw white text on transparent background.
        ctx.set_rgb_fill_color(1.0, 1.0, 1.0, 1.0);

        // Build CTLine for text rendering.
        use core_foundation::attributed_string::CFMutableAttributedString;
        use core_foundation::string::CFString;

        extern "C" {
            fn CTLineCreateWithAttributedString(
                attr_string: core_foundation::base::CFTypeRef,
            ) -> *mut std::ffi::c_void;
            fn CTLineDraw(
                line: *const std::ffi::c_void,
                context: *mut core_graphics::sys::CGContext,
            );
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
            // Without this, Core Text defaults to black, which still works for
            // alpha extraction but setting it explicitly is more correct.
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

        // Position text: baseline centered in cell.
        let ascent = ct.ascent();
        let descent = ct.descent();
        let text_h = ascent + descent.abs();
        let baseline_y = ((bmp_h as f64 - text_h) / 2.0 + descent.abs()).max(0.0);

        // Get advance width for centering.
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

        ctx.set_text_position(text_x, baseline_y);

        unsafe {
            CTLineDraw(line, ctx.as_ptr());
            CFRelease(line);
        }

        // Extract grayscale from the RGBA bitmap (use white channel intensity).
        let cg_data = ctx.data();
        let total_bytes = (bmp_w * bmp_h * 4) as usize;
        if total_bytes > cg_data.len() {
            return None;
        }

        // CG renders bottom-up; flip to top-down for our atlas.
        let mut gray = vec![0u8; (bmp_w * bmp_h) as usize];
        for row in 0..bmp_h {
            let src_row = bmp_h - 1 - row; // flip
            for col in 0..bmp_w {
                let src_idx = ((src_row * bmp_w + col) * 4) as usize;
                // Use the red channel (white text → R=G=B=alpha).
                let alpha = cg_data[src_idx + 3];
                let di = (row * bmp_w + col) as usize;
                gray[di] = alpha;
            }
        }

        // Check we got any visible pixels.
        if gray.iter().all(|&v| v == 0) {
            return None;
        }

        let info = self.pack_cell_bitmap(&gray, bmp_w, bmp_h);
        log::debug!("Core Text fallback rendered U+{:04X} '{}'", c as u32, c);
        Some(info)
    }

    #[cfg(not(target_os = "macos"))]
    pub(crate) fn try_core_text_fallback(
        &mut self,
        _c: char,
        _entry_w: u32,
        _entry_h: u32,
    ) -> Option<GlyphInfo> {
        None
    }
}
