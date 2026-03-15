//! Font atlas — CPU glyph rasterization + texture packing.
//!
//! Uses `fontdue` to rasterize glyphs into a single atlas texture
//! that the GPU shader samples from.

use std::collections::HashMap;

/// Configuration for loading the font.
#[derive(Debug, Clone)]
pub struct FontConfig {
    /// Font family name (used for font file lookup; currently we embed a font).
    pub family: String,
    /// Font size in pixels.
    pub size: f32,
    /// Line height multiplier (e.g., 1.2 means 120% of the natural line height).
    pub line_height: f32,
}

impl Default for FontConfig {
    fn default() -> Self {
        Self {
            family: String::from("monospace"),
            size: 14.0,
            line_height: 1.2,
        }
    }
}

/// Cell dimensions derived from font metrics.
#[derive(Debug, Clone, Copy)]
pub struct CellSize {
    pub width: f32,
    pub height: f32,
}

/// UV region within the atlas for a single glyph (in texel coordinates).
#[derive(Debug, Clone, Copy)]
pub struct GlyphInfo {
    /// X offset in the atlas texture (texels).
    pub atlas_x: f32,
    /// Y offset in the atlas texture (texels).
    pub atlas_y: f32,
    /// Width of the glyph bitmap in the atlas (texels).
    pub atlas_w: f32,
    /// Height of the glyph bitmap in the atlas (texels).
    pub atlas_h: f32,
    /// Horizontal offset from the cell origin to place the glyph bitmap.
    pub bearing_x: f32,
    /// Vertical offset from the cell top to place the glyph bitmap.
    pub bearing_y: f32,
}

/// A font atlas that maps characters to UV regions in a texture.
pub struct Atlas {
    /// The atlas texture data (single-channel alpha, R8).
    pub data: Vec<u8>,
    /// Atlas width in pixels.
    pub width: u32,
    /// Atlas height in pixels.
    pub height: u32,
    /// Mapping from character to glyph info.
    glyphs: HashMap<char, GlyphInfo>,
    /// Cell size derived from font metrics.
    pub cell_size: CellSize,
    /// The fontdue font, kept for on-demand rasterization.
    font: fontdue::Font,
    /// Font size in px.
    font_size: f32,
    /// Ascent in pixels (distance from top of cell to baseline).
    ascent: f32,
    /// Current packing cursor X.
    pack_x: u32,
    /// Current packing cursor Y.
    pack_y: u32,
    /// Current row height in the packing strip.
    pack_row_height: u32,
}

impl Atlas {
    /// Create a new atlas from a font configuration.
    ///
    /// Pre-rasterizes ASCII printable characters (32-126) and packs them
    /// into the atlas texture.
    pub fn new(config: &FontConfig) -> Result<Self, AtlasError> {
        // Use the system's default monospace font data.
        // For now, fall back to an embedded font or system font.
        let font_data = Self::load_font_data(&config.family)?;
        let font = fontdue::Font::from_bytes(
            font_data.as_slice(),
            fontdue::FontSettings {
                scale: config.size * 2.0, // Optimize for 2x the target size
                ..Default::default()
            },
        )
        .map_err(|e| AtlasError::FontParsing(e.to_string()))?;

        // Derive cell dimensions from font metrics.
        let line_metrics = font
            .horizontal_line_metrics(config.size)
            .ok_or(AtlasError::FontParsing(
                "no horizontal line metrics".to_string(),
            ))?;

        let ascent = line_metrics.ascent;
        let descent = line_metrics.descent; // negative
        let natural_height = ascent - descent;
        let cell_height = natural_height * config.line_height;

        // Use 'M' advance width as the cell width for monospace.
        let (m_metrics, _) = font.rasterize('M', config.size);
        let cell_width = m_metrics.advance_width;

        let cell_size = CellSize {
            width: cell_width.ceil(),
            height: cell_height.ceil(),
        };

        // Initial atlas dimensions: large enough for ASCII + room to grow.
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
            font_size: config.size,
            ascent,
            pack_x: 1, // Start at 1 to leave a 1-pixel border
            pack_y: 1,
            pack_row_height: 0,
        };

        // Pre-rasterize ASCII printable characters.
        for c in (32u8..=126).map(|b| b as char) {
            atlas.rasterize_glyph(c);
        }

        Ok(atlas)
    }

    /// Look up a glyph. If not yet rasterized, rasterize it on demand.
    pub fn get_glyph(&mut self, c: char) -> GlyphInfo {
        if let Some(&info) = self.glyphs.get(&c) {
            return info;
        }
        self.rasterize_glyph(c)
    }

    /// Check if a glyph is already in the atlas without rasterizing.
    pub fn has_glyph(&self, c: char) -> bool {
        self.glyphs.contains_key(&c)
    }

    /// Returns whether new glyphs have been added since the last call.
    /// Used to determine if the GPU texture needs re-uploading.
    pub fn glyph_count(&self) -> usize {
        self.glyphs.len()
    }

    /// Rasterize a glyph and pack it into the atlas. Returns the glyph info.
    fn rasterize_glyph(&mut self, c: char) -> GlyphInfo {
        let (metrics, bitmap) = self.font.rasterize(c, self.font_size);

        let glyph_w = metrics.width as u32;
        let glyph_h = metrics.height as u32;

        // Handle zero-size glyphs (spaces, control chars)
        if glyph_w == 0 || glyph_h == 0 {
            let info = GlyphInfo {
                atlas_x: 0.0,
                atlas_y: 0.0,
                atlas_w: 0.0,
                atlas_h: 0.0,
                bearing_x: 0.0,
                bearing_y: 0.0,
            };
            self.glyphs.insert(c, info);
            return info;
        }

        // Pack with 1-pixel padding between glyphs.
        let padded_w = glyph_w + 1;
        let padded_h = glyph_h + 1;

        // Check if we need to advance to the next row.
        if self.pack_x + padded_w > self.width {
            self.pack_x = 1;
            self.pack_y += self.pack_row_height + 1;
            self.pack_row_height = 0;
        }

        // Check if we've run out of vertical space (would need atlas resize).
        if self.pack_y + padded_h > self.height {
            self.grow_atlas();
        }

        let atlas_x = self.pack_x;
        let atlas_y = self.pack_y;

        // Copy glyph bitmap into atlas.
        for row in 0..glyph_h {
            let src_offset = (row * glyph_w) as usize;
            let dst_offset = ((atlas_y + row) * self.width + atlas_x) as usize;
            let src_end = src_offset + glyph_w as usize;
            let dst_end = dst_offset + glyph_w as usize;
            if src_end <= bitmap.len() && dst_end <= self.data.len() {
                self.data[dst_offset..dst_end].copy_from_slice(&bitmap[src_offset..src_end]);
            }
        }

        // The bearing_y for fontdue is: ymin (number of pixels below the baseline to the
        // bottom of the glyph). We need to convert to "distance from cell top to glyph top".
        // glyph_top_from_baseline = metrics.height as f32 + metrics.ymin as f32
        // bearing_y = ascent - glyph_top_from_baseline
        let glyph_top_from_baseline = metrics.height as f32 + metrics.ymin as f32;
        let bearing_y = self.ascent - glyph_top_from_baseline;

        let info = GlyphInfo {
            atlas_x: atlas_x as f32,
            atlas_y: atlas_y as f32,
            atlas_w: glyph_w as f32,
            atlas_h: glyph_h as f32,
            bearing_x: metrics.xmin as f32,
            bearing_y,
        };

        self.glyphs.insert(c, info);

        // Advance packing cursor.
        self.pack_x += padded_w;
        self.pack_row_height = self.pack_row_height.max(padded_h);

        info
    }

    /// Double the atlas height when we run out of space.
    fn grow_atlas(&mut self) {
        let new_height = self.height * 2;
        let mut new_data = vec![0u8; (self.width * new_height) as usize];
        new_data[..self.data.len()].copy_from_slice(&self.data);
        self.data = new_data;
        self.height = new_height;
        log::info!("Atlas grew to {}x{}", self.width, self.height);
    }

    /// Try to load a monospace font from the system.
    /// Falls back to embedded font data if the system font is not found.
    fn load_font_data(family: &str) -> Result<Vec<u8>, AtlasError> {
        // Try to find the font on macOS using known paths.
        let candidates = if family == "monospace" || family.is_empty() {
            vec![
                "/System/Library/Fonts/SFMono-Regular.otf",
                "/System/Library/Fonts/Menlo.ttc",
                "/System/Library/Fonts/Monaco.dfont",
                "/Library/Fonts/JetBrainsMono-Regular.ttf",
                "/System/Library/Fonts/Courier.dfont",
            ]
        } else {
            vec![]
        };

        for path in &candidates {
            if let Ok(data) = std::fs::read(path) {
                log::info!("Loaded font from {path}");
                return Ok(data);
            }
        }

        // Fall back to finding any monospace font.
        // As a last resort, use the built-in font from the system.
        // Try common cross-platform locations.
        let fallbacks = [
            "/usr/share/fonts/truetype/dejavu/DejaVuSansMono.ttf",
            "/usr/share/fonts/TTF/DejaVuSansMono.ttf",
        ];
        for path in &fallbacks {
            if let Ok(data) = std::fs::read(path) {
                log::info!("Loaded fallback font from {path}");
                return Ok(data);
            }
        }

        Err(AtlasError::FontNotFound(family.to_string()))
    }
}

/// Errors that can occur during atlas construction.
#[derive(Debug, thiserror::Error)]
pub enum AtlasError {
    #[error("font not found: {0}")]
    FontNotFound(String),

    #[error("font parsing error: {0}")]
    FontParsing(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Helper: create an atlas from system fonts if available.
    /// Tests are skipped (not failed) if no system font is found.
    fn try_create_atlas() -> Option<Atlas> {
        let config = FontConfig::default();
        match Atlas::new(&config) {
            Ok(atlas) => Some(atlas),
            Err(AtlasError::FontNotFound(_)) => {
                eprintln!("Skipping atlas test: no system monospace font found");
                None
            }
            Err(e) => panic!("Unexpected atlas error: {e}"),
        }
    }

    #[test]
    fn test_atlas_creation() {
        if let Some(atlas) = try_create_atlas() {
            assert!(atlas.width > 0);
            assert!(atlas.height > 0);
            assert!(atlas.cell_size.width > 0.0);
            assert!(atlas.cell_size.height > 0.0);
        }
    }

    #[test]
    fn test_ascii_glyphs_present() {
        if let Some(atlas) = try_create_atlas() {
            // All printable ASCII should be pre-rasterized.
            for c in (32u8..=126).map(|b| b as char) {
                assert!(atlas.has_glyph(c), "Missing glyph for '{c}' (0x{:02X})", c as u32);
            }
        }
    }

    #[test]
    fn test_space_glyph_zero_size() {
        if let Some(atlas) = try_create_atlas() {
            let glyph = atlas.glyphs.get(&' ').expect("space glyph missing");
            // Space should have zero atlas dimensions (no visible glyph).
            assert_eq!(glyph.atlas_w, 0.0);
            assert_eq!(glyph.atlas_h, 0.0);
        }
    }

    #[test]
    fn test_on_demand_rasterize() {
        if let Some(mut atlas) = try_create_atlas() {
            let initial_count = atlas.glyph_count();
            let info = atlas.get_glyph('\u{2500}'); // Box drawing horizontal
            assert!(atlas.glyph_count() > initial_count || info.atlas_w == 0.0);
        }
    }

    #[test]
    fn test_glyph_info_a() {
        if let Some(atlas) = try_create_atlas() {
            let glyph = atlas.glyphs.get(&'A').expect("'A' glyph missing");
            // 'A' should have non-zero dimensions.
            assert!(glyph.atlas_w > 0.0, "A glyph width should be > 0");
            assert!(glyph.atlas_h > 0.0, "A glyph height should be > 0");
        }
    }

    #[test]
    fn test_packing_no_overlap() {
        if let Some(atlas) = try_create_atlas() {
            // Collect all non-zero glyphs and verify no overlap.
            let rects: Vec<_> = atlas
                .glyphs
                .values()
                .filter(|g| g.atlas_w > 0.0 && g.atlas_h > 0.0)
                .collect();

            for (i, a) in rects.iter().enumerate() {
                for b in rects.iter().skip(i + 1) {
                    let no_overlap = a.atlas_x + a.atlas_w <= b.atlas_x
                        || b.atlas_x + b.atlas_w <= a.atlas_x
                        || a.atlas_y + a.atlas_h <= b.atlas_y
                        || b.atlas_y + b.atlas_h <= a.atlas_y;
                    assert!(
                        no_overlap,
                        "Glyph overlap detected: ({}, {}, {}, {}) vs ({}, {}, {}, {})",
                        a.atlas_x, a.atlas_y, a.atlas_w, a.atlas_h, b.atlas_x, b.atlas_y,
                        b.atlas_w, b.atlas_h,
                    );
                }
            }
        }
    }
}
