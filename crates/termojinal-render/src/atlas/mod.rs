//! Font atlas — CPU glyph rasterization + texture packing.
//!
//! Each glyph is rasterized into a **cell-sized** bitmap with the glyph
//! placed at the correct position using font metrics (bearing). The shader
//! can then map the cell quad directly to the atlas region without worrying
//! about glyph positioning.

mod coretext;
mod font_loader;
mod procedural;
mod tests;

use std::collections::HashMap;

/// Configuration for loading the font.
#[derive(Debug, Clone)]
pub struct FontConfig {
    pub family: String,
    pub size: f32,
    pub line_height: f32,
}

impl Default for FontConfig {
    fn default() -> Self {
        Self {
            family: String::from("monospace"),
            size: 16.0,
            line_height: 1.2,
        }
    }
}

/// Cell dimensions derived from font metrics.
#[derive(Debug, Clone, Copy)]
pub struct CellSize {
    pub width: f32,
    pub height: f32,
    /// Font ascent (distance from baseline to top of tallest glyph).
    pub ascent: f32,
    /// Font descent (negative, distance from baseline to bottom of lowest glyph).
    pub descent: f32,
}

/// UV region within the atlas for a single glyph (in texel coordinates).
#[derive(Debug, Clone, Copy)]
pub struct GlyphInfo {
    pub atlas_x: f32,
    pub atlas_y: f32,
    pub atlas_w: f32,
    pub atlas_h: f32,
    pub bearing_x: f32,
    pub bearing_y: f32,
}

/// A font atlas that maps characters to UV regions in a texture.
pub struct Atlas {
    pub data: Vec<u8>,
    pub width: u32,
    pub height: u32,
    pub(crate) glyphs: HashMap<char, GlyphInfo>,
    pub cell_size: CellSize,
    pub(crate) font: fontdue::Font,
    pub(crate) fallback_font: Option<fontdue::Font>,
    pub(crate) cjk_font: Option<fontdue::Font>,
    pub(crate) symbols_font: Option<fontdue::Font>,
    pub(crate) font_size: f32,
    pub(crate) ascent: f32,
    pub(crate) cell_w: u32,
    pub(crate) cell_h: u32,
    pub(crate) pack_x: u32,
    pub(crate) pack_y: u32,
    pub(crate) pack_row_height: u32,
    /// Whether to use CJK-aware character width calculation.
    pub cjk_width: bool,
}

impl Atlas {
    pub fn new(config: &FontConfig) -> Result<Self, AtlasError> {
        let font_data = Self::load_font_data(&config.family)?;
        let font =
            fontdue::Font::from_bytes(font_data.as_slice(), fontdue::FontSettings::default())
                .map_err(|e| AtlasError::FontParsing(e.to_string()))?;

        let line_metrics =
            font.horizontal_line_metrics(config.size)
                .ok_or(AtlasError::FontParsing(
                    "no horizontal line metrics".to_string(),
                ))?;

        let ascent = line_metrics.ascent;
        let descent = line_metrics.descent;
        let natural_height = ascent - descent;
        let cell_height = (natural_height * config.line_height).ceil();

        let (m_metrics, _) = font.rasterize('M', config.size);
        let cell_width = m_metrics.advance_width.ceil();

        let cell_size = CellSize {
            width: cell_width,
            height: cell_height,
            ascent,
            descent,
        };

        let cell_w = cell_width as u32;
        let cell_h = cell_height as u32;

        log::info!(
            "font metrics: ascent={ascent:.1}, descent={descent:.1}, \
             cell={}x{}, size={}",
            cell_w,
            cell_h,
            config.size
        );

        // Try to load a Nerd Font as fallback for PUA / box-drawing glyphs.
        let fallback_font = Self::load_fallback_nerd_font();
        // Try to load a CJK font as fallback for Japanese/Chinese/Korean characters.
        let cjk_font = Self::load_cjk_fallback_font();
        // Try to load a symbols font as fallback for Braille, geometric shapes,
        // misc symbols, arrows, etc. that the primary monospace font may lack.
        let symbols_font = Self::load_symbols_fallback_font();

        let atlas_width = 1024u32;
        let atlas_height = 1024u32;
        let data = vec![0u8; (atlas_width * atlas_height) as usize];

        let mut atlas = Self {
            data,
            width: atlas_width,
            height: atlas_height,
            glyphs: HashMap::new(),
            cell_size,
            font,
            fallback_font,
            cjk_font,
            symbols_font,
            font_size: config.size,
            // Shift baseline down by half the line_height extra space so text is
            // vertically centered in the cell, not top-aligned.
            ascent: ascent + (cell_height - natural_height) / 2.0,
            cell_w,
            cell_h,
            pack_x: 1,
            pack_y: 1,
            pack_row_height: 0,
            cjk_width: false,
        };

        // Pre-rasterize ASCII printable characters.
        for c in (32u8..=126).map(|b| b as char) {
            atlas.rasterize_glyph(c);
        }

        Ok(atlas)
    }

    pub fn get_glyph(&mut self, c: char) -> GlyphInfo {
        if let Some(&info) = self.glyphs.get(&c) {
            return info;
        }
        self.rasterize_glyph(c)
    }

    pub fn has_glyph(&self, c: char) -> bool {
        self.glyphs.contains_key(&c)
    }

    pub fn glyph_count(&self) -> usize {
        self.glyphs.len()
    }

    /// Check if a cached glyph has all-zero pixels in the atlas.
    ///
    /// Returns `true` if the glyph was rasterized but produced no visible
    /// output (e.g. the font has no real glyph for this codepoint).
    /// Returns `false` if the glyph has visible pixels or is not cached.
    pub fn is_glyph_empty(&self, c: char) -> bool {
        let Some(&info) = self.glyphs.get(&c) else {
            return false;
        };
        if info.atlas_w <= 0.0 || info.atlas_h <= 0.0 {
            return true;
        }
        // Check the atlas bitmap region for any non-zero pixels.
        let ax = info.atlas_x as u32;
        let ay = info.atlas_y as u32;
        let aw = info.atlas_w as u32;
        let ah = info.atlas_h as u32;
        for row in ay..ay + ah {
            for col in ax..ax + aw {
                if row < self.height && col < self.width {
                    let idx = (row * self.width + col) as usize;
                    if idx < self.data.len() && self.data[idx] > 0 {
                        return false;
                    }
                }
            }
        }
        true
    }

    /// Returns true if the character is in a range that may need a fallback font:
    /// Private Use Area (Nerd Font icons), box-drawing, block elements, CJK,
    /// Braille patterns, geometric shapes, miscellaneous symbols, dingbats, arrows,
    /// and other symbol blocks commonly used in terminal output.
    pub(crate) fn needs_fallback_check(c: char) -> bool {
        matches!(c,
            '\u{E000}'..='\u{F8FF}'   // BMP Private Use Area (Nerd Font icons)
            | '\u{F0000}'..='\u{FFFFF}' // Supplementary PUA-A
            | '\u{2500}'..='\u{257F}'  // Box-drawing characters
            | '\u{2580}'..='\u{259F}'  // Block elements
            | '\u{2190}'..='\u{21FF}'  // Arrows (←↑→↓⇐⇑⇒⇓ etc.)
            | '\u{2200}'..='\u{22FF}'  // Mathematical Operators (∞≠≤≥ etc.)
            | '\u{2300}'..='\u{23FF}'  // Miscellaneous Technical (⌘⌥⌫⏎ etc.)
            | '\u{2460}'..='\u{24FF}'  // Enclosed Alphanumerics (①② etc.)
            | '\u{25A0}'..='\u{25FF}'  // Geometric Shapes (■□▲△○●◆◇◯ etc.)
            | '\u{2600}'..='\u{26FF}'  // Miscellaneous Symbols (☀☁☂★☆♠♣♥♦ etc.)
            | '\u{2700}'..='\u{27BF}'  // Dingbats (✓✗✘✚✜ etc.)
            | '\u{27C0}'..='\u{27EF}'  // Misc Mathematical Symbols-A
            | '\u{27F0}'..='\u{27FF}'  // Supplemental Arrows-A
            | '\u{2800}'..='\u{28FF}'  // Braille Patterns (spinners: ⠋⠙⠹⠸⠼⠴⠦⠧⠇⠏)
            | '\u{2900}'..='\u{297F}'  // Supplemental Arrows-B
            | '\u{2B00}'..='\u{2BFF}'  // Misc Symbols and Arrows
            | '\u{3000}'..='\u{9FFF}'  // CJK Unified Ideographs + Hiragana/Katakana
            | '\u{F900}'..='\u{FAFF}'  // CJK Compatibility Ideographs
            | '\u{AC00}'..='\u{D7AF}'  // Hangul
            | '\u{FF00}'..='\u{FFEF}'  // Halfwidth and Fullwidth Forms (！？ etc.)
            | '\u{FE30}'..='\u{FE4F}'  // CJK Compatibility Forms
            | '\u{FE50}'..='\u{FE6F}'  // Small Form Variants
            | '\u{20000}'..='\u{2A6DF}' // CJK Unified Ideographs Extension B
            | '\u{2A700}'..='\u{2B73F}' // CJK Unified Ideographs Extension C
            | '\u{1F000}'..='\u{1F02F}' // Mahjong Tiles
            | '\u{1F030}'..='\u{1F09F}' // Domino Tiles
        )
    }

    /// Returns true if the character is in CJK ranges (needs CJK-specific fallback).
    /// This includes not only CJK ideographs and kana, but also symbol ranges
    /// that are commonly rendered with CJK fonts (geometric shapes, enclosed
    /// alphanumerics, etc.) which have East Asian Ambiguous width.
    fn is_cjk(c: char) -> bool {
        matches!(c,
            '\u{3000}'..='\u{9FFF}'  // CJK Unified Ideographs + Hiragana/Katakana
            | '\u{F900}'..='\u{FAFF}'  // CJK Compatibility Ideographs
            | '\u{AC00}'..='\u{D7AF}'  // Hangul
            | '\u{FF00}'..='\u{FFEF}'  // Halfwidth and Fullwidth Forms (！？ etc.)
            | '\u{FE30}'..='\u{FE4F}'  // CJK Compatibility Forms
            | '\u{FE50}'..='\u{FE6F}'  // Small Form Variants
            | '\u{25A0}'..='\u{25FF}'  // Geometric Shapes (■□▲△○●◆◇◯ etc.)
            | '\u{2600}'..='\u{26FF}'  // Miscellaneous Symbols (commonly in CJK fonts)
            | '\u{2460}'..='\u{24FF}'  // Enclosed Alphanumerics (①②③ etc.)
            | '\u{20000}'..='\u{2A6DF}' // CJK Unified Ideographs Extension B
            | '\u{2A700}'..='\u{2B73F}' // CJK Unified Ideographs Extension C
        )
    }

    /// Rasterize a glyph, place it at the correct bearing offset within a
    /// cell-sized bitmap, and pack that bitmap into the atlas.
    fn rasterize_glyph(&mut self, c: char) -> GlyphInfo {
        // Block elements and shade characters: draw procedurally to fill cells
        // perfectly (font glyphs leave gaps that break ASCII art).
        if let Some(info) = self.try_procedural_block(c) {
            self.glyphs.insert(c, info);
            return info;
        }

        let (metrics, bitmap) = self.font.rasterize(c, self.font_size);

        let glyph_w = metrics.width as u32;
        let glyph_h = metrics.height as u32;

        // Determine the display width: CJK/wide characters span 2 cells.
        let char_width = termojinal_vt::char_width(c, self.cjk_width) as u32;
        let entry_w = self.cell_w * char_width;
        let entry_h = self.cell_h;

        // Check if we should use a fallback font instead.
        // Use fallback when:
        //   - the primary returns a zero-size bitmap, OR
        //   - the primary font has no glyph (glyph index 0 = .notdef), OR
        //   - the bitmap has no visible pixels (font has codepoint but empty rendering)
        let primary_missing = (glyph_w == 0 || glyph_h == 0)
            || self.font.lookup_glyph_index(c) == 0
            || (Self::needs_fallback_check(c) && bitmap.iter().all(|&b| b == 0));

        let (metrics, bitmap) = if Self::is_cjk(c) {
            // CJK range characters: use CJK font for actual CJK ideographs/kana
            // (U+3000+), but for symbol ranges (geometric shapes, enclosed
            // alphanumerics, etc.) only prefer CJK font when cjk_width is enabled.
            // Otherwise, use primary font if it has a glyph (avoids squishing
            // wide CJK glyphs into narrow cells).
            let is_true_cjk = c >= '\u{3000}';
            let prefer_cjk = is_true_cjk || self.cjk_width;

            if prefer_cjk {
                if let Some(ref cjk) = self.cjk_font {
                    if cjk.lookup_glyph_index(c) != 0 {
                        cjk.rasterize(c, self.font_size)
                    } else if !primary_missing {
                        (metrics, bitmap)
                    } else if let Some(ref fb) = self.fallback_font {
                        if fb.lookup_glyph_index(c) != 0 {
                            fb.rasterize(c, self.font_size)
                        } else {
                            (metrics, bitmap)
                        }
                    } else {
                        (metrics, bitmap)
                    }
                } else if !primary_missing {
                    (metrics, bitmap)
                } else if let Some(ref fb) = self.fallback_font {
                    if fb.lookup_glyph_index(c) != 0 {
                        fb.rasterize(c, self.font_size)
                    } else {
                        (metrics, bitmap)
                    }
                } else {
                    (metrics, bitmap)
                }
            } else if !primary_missing {
                // Non-CJK locale: primary font has a glyph, use it.
                (metrics, bitmap)
            } else {
                // Primary missing, try CJK font, then other fallbacks.
                let mut result = None;
                if let Some(ref cjk) = self.cjk_font {
                    if cjk.lookup_glyph_index(c) != 0 {
                        result = Some(cjk.rasterize(c, self.font_size));
                    }
                }
                if result.is_none() {
                    if let Some(ref fb) = self.fallback_font {
                        if fb.lookup_glyph_index(c) != 0 {
                            result = Some(fb.rasterize(c, self.font_size));
                        }
                    }
                }
                result.unwrap_or((metrics, bitmap))
            }
        } else if primary_missing && Self::needs_fallback_check(c) {
            // Non-CJK fallback: try Nerd Font first, then symbols font.
            let mut result = None;
            if let Some(ref fb) = self.fallback_font {
                if fb.lookup_glyph_index(c) != 0 {
                    result = Some(fb.rasterize(c, self.font_size));
                }
            }
            if result.is_none() {
                if let Some(ref sym) = self.symbols_font {
                    if sym.lookup_glyph_index(c) != 0 {
                        result = Some(sym.rasterize(c, self.font_size));
                    }
                }
            }
            result.unwrap_or((metrics, bitmap))
        } else if primary_missing {
            // Last-resort fallback for any character the primary font lacks:
            // try symbols font, then Nerd Font.
            let mut result = None;
            if let Some(ref sym) = self.symbols_font {
                if sym.lookup_glyph_index(c) != 0 {
                    result = Some(sym.rasterize(c, self.font_size));
                }
            }
            if result.is_none() {
                if let Some(ref fb) = self.fallback_font {
                    if fb.lookup_glyph_index(c) != 0 {
                        result = Some(fb.rasterize(c, self.font_size));
                    }
                }
            }
            result.unwrap_or((metrics, bitmap))
        } else {
            (metrics, bitmap)
        };

        let glyph_w = metrics.width as u32;
        let glyph_h = metrics.height as u32;

        // Handle missing glyphs: try Core Text fallback before giving up.
        // A glyph is considered missing if it has zero size OR if it has
        // non-zero dimensions but all pixels are blank (empty rendering).
        let nonzero_count = bitmap.iter().filter(|&&b| b > 0).count();
        let glyph_empty = (glyph_w == 0 || glyph_h == 0) || nonzero_count == 0;

        // Log non-ASCII glyphs for debugging rendering issues.
        if c as u32 > 0x7F {
            log::debug!(
                "glyph U+{:04X} '{}': after_fallback {}x{} nonzero={} empty={}",
                c as u32,
                c,
                glyph_w,
                glyph_h,
                nonzero_count,
                glyph_empty,
            );
        }

        if glyph_empty {
            if c > ' ' && !c.is_control() {
                // Non-trivial character that fontdue couldn't render.
                // Try Core Text as a last-resort fallback.
                if let Some(info) = self.try_core_text_fallback(c, entry_w, entry_h) {
                    self.glyphs.insert(c, info);
                    return info;
                }
                log::debug!(
                    "glyph U+{:04X} '{}': Core Text fallback also failed",
                    c as u32,
                    c
                );
            }
            if glyph_w == 0 || glyph_h == 0 {
                let info = self.pack_cell_bitmap(
                    &vec![0u8; (entry_w * entry_h) as usize],
                    entry_w,
                    entry_h,
                );
                self.glyphs.insert(c, info);
                return info;
            }
        }

        let mut cell_bitmap = vec![0u8; (entry_w * entry_h) as usize];

        // If glyph is wider than cell or taller than cell, scale uniformly
        // to fit while preserving aspect ratio, then center the result.
        let (src_bitmap, src_w, src_h) = if glyph_w > entry_w || glyph_h > entry_h {
            let scale_x = entry_w as f32 / glyph_w as f32;
            let scale_y = entry_h as f32 / glyph_h as f32;
            let scale = scale_x.min(scale_y); // uniform scale: fit within cell
            let scaled_w = (glyph_w as f32 * scale).ceil() as u32;
            let scaled_h = (glyph_h as f32 * scale).ceil() as u32;
            let scaled_w = scaled_w.min(entry_w).max(1);
            let scaled_h = scaled_h.min(entry_h).max(1);

            let mut scaled = vec![0u8; (scaled_w * scaled_h) as usize];
            for row in 0..scaled_h {
                for col in 0..scaled_w {
                    let src_col = (col as f32 / scale).min((glyph_w - 1) as f32) as u32;
                    let src_row = (row as f32 / scale).min((glyph_h - 1) as f32) as u32;
                    let si = (src_row * glyph_w + src_col) as usize;
                    let di = (row * scaled_w + col) as usize;
                    if si < bitmap.len() && di < scaled.len() {
                        scaled[di] = bitmap[si];
                    }
                }
            }
            (scaled, scaled_w, scaled_h)
        } else {
            (bitmap, glyph_w, glyph_h)
        };

        // Center the scaled glyph within the cell.
        let offset_x = if src_w < entry_w {
            if glyph_w > entry_w {
                // Was scaled down: center horizontally.
                (entry_w - src_w) / 2
            } else {
                metrics.xmin.max(0) as u32
            }
        } else {
            0
        };
        let offset_y = if glyph_w > entry_w || glyph_h > entry_h {
            // Was scaled: center vertically in cell.
            (entry_h.saturating_sub(src_h)) / 2
        } else {
            let glyph_top_from_baseline = src_h as f32 + metrics.ymin as f32;
            (self.ascent - glyph_top_from_baseline).max(0.0) as u32
        };

        for row in 0..src_h.min(entry_h) {
            for col in 0..src_w.min(entry_w) {
                let dst_x = offset_x + col;
                let dst_y = offset_y + row;
                if dst_x < entry_w && dst_y < entry_h {
                    let src_idx = (row * src_w + col) as usize;
                    let dst_idx = (dst_y * entry_w + dst_x) as usize;
                    if src_idx < src_bitmap.len() && dst_idx < cell_bitmap.len() {
                        cell_bitmap[dst_idx] = src_bitmap[src_idx];
                    }
                }
            }
        }

        let info = self.pack_cell_bitmap(&cell_bitmap, entry_w, entry_h);
        self.glyphs.insert(c, info);
        info
    }

    /// Pack a bitmap into the atlas, returning the GlyphInfo.
    pub(crate) fn pack_cell_bitmap(
        &mut self,
        bitmap: &[u8],
        entry_w: u32,
        entry_h: u32,
    ) -> GlyphInfo {
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

        // Copy cell bitmap into atlas.
        for row in 0..entry_h {
            let src_offset = (row * entry_w) as usize;
            let dst_offset = ((atlas_y + row) * self.width + atlas_x) as usize;
            let src_end = src_offset + entry_w as usize;
            let dst_end = dst_offset + entry_w as usize;
            if src_end <= bitmap.len() && dst_end <= self.data.len() {
                self.data[dst_offset..dst_end].copy_from_slice(&bitmap[src_offset..src_end]);
            }
        }

        // Advance packing cursor.
        self.pack_x += padded_w;
        self.pack_row_height = self.pack_row_height.max(padded_h);

        GlyphInfo {
            atlas_x: atlas_x as f32,
            atlas_y: atlas_y as f32,
            atlas_w: entry_w as f32,
            atlas_h: entry_h as f32,
            bearing_x: 0.0, // Baked into the bitmap.
            bearing_y: 0.0,
        }
    }

    fn grow_atlas(&mut self) {
        let new_height = self.height * 2;
        let mut new_data = vec![0u8; (self.width * new_height) as usize];
        new_data[..self.data.len()].copy_from_slice(&self.data);
        self.data = new_data;
        self.height = new_height;
        log::info!("atlas grew to {}x{}", self.width, self.height);
    }
}

#[derive(Debug, thiserror::Error)]
pub enum AtlasError {
    #[error("font not found: {0}")]
    FontNotFound(String),

    #[error("font parsing error: {0}")]
    FontParsing(String),
}
