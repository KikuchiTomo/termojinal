//! Font loading utilities for the Atlas.

use super::{Atlas, AtlasError};

impl Atlas {
    // TODO: load font path from ~/.config/termojinal/config.toml instead of hardcoding
    pub(crate) fn load_font_data(family: &str) -> Result<Vec<u8>, AtlasError> {
        let candidates = if family == "monospace" || family.is_empty() {
            vec![
                // Prefer single-file TTF/OTF over TTC (fontdue handles them better).
                "/System/Library/Fonts/SFNSMono.ttf",
                "/Library/Fonts/JetBrainsMono-Regular.ttf",
                "/System/Library/Fonts/Supplemental/Andale Mono.ttf",
                "/System/Library/Fonts/Supplemental/Courier New.ttf",
                // TTC files: fontdue uses collection_index=0 by default.
                "/System/Library/Fonts/Menlo.ttc",
                "/System/Library/Fonts/Courier.ttc",
            ]
        } else {
            vec![]
        };

        for path in &candidates {
            if let Ok(data) = std::fs::read(path) {
                log::info!("loaded font from {path}");
                return Ok(data);
            }
        }

        let fallbacks = [
            "/usr/share/fonts/truetype/dejavu/DejaVuSansMono.ttf",
            "/usr/share/fonts/TTF/DejaVuSansMono.ttf",
        ];
        for path in &fallbacks {
            if let Ok(data) = std::fs::read(path) {
                log::info!("loaded fallback font from {path}");
                return Ok(data);
            }
        }

        Err(AtlasError::FontNotFound(family.to_string()))
    }

    /// Try to find and load a Nerd Font from ~/Library/Fonts/ for fallback
    /// glyph rendering (PUA icons, box-drawing, etc.).
    pub(crate) fn load_fallback_nerd_font() -> Option<fontdue::Font> {
        let home = std::env::var("HOME").ok()?;
        let fonts_dir = std::path::PathBuf::from(&home).join("Library/Fonts");
        let entries = std::fs::read_dir(&fonts_dir).ok()?;

        for entry in entries.flatten() {
            let path = entry.path();
            let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");

            // Look for Nerd Font files (contain "Nerd" or "NF" in the name).
            let is_nerd = name.contains("Nerd") || name.contains(" NF ");
            let is_ttf = name.ends_with(".ttf") || name.ends_with(".otf");

            if is_nerd && is_ttf {
                if let Ok(data) = std::fs::read(&path) {
                    match fontdue::Font::from_bytes(
                        data.as_slice(),
                        fontdue::FontSettings::default(),
                    ) {
                        Ok(font) => {
                            log::info!("loaded fallback Nerd Font from {}", path.display());
                            return Some(font);
                        }
                        Err(e) => {
                            log::warn!("failed to parse fallback font {}: {e}", path.display());
                        }
                    }
                }
            }
        }

        log::info!(
            "no Nerd Font found in {}, fallback disabled",
            fonts_dir.display()
        );
        None
    }

    /// Try to find and load a CJK font from system font directories for fallback
    /// glyph rendering of Japanese/Chinese/Korean characters.
    pub(crate) fn load_cjk_fallback_font() -> Option<fontdue::Font> {
        // macOS system CJK font candidates.
        // Prefer single-file TTF/OTF over TTC since fontdue handles them better.
        // For TTC files, fontdue uses collection_index=0 by default.
        let candidates = [
            "/System/Library/Fonts/Supplemental/Hiragino Sans W3.ttc",
            "/System/Library/Fonts/\u{30D2}\u{30E9}\u{30AE}\u{30CE}\u{89D2}\u{30B4}\u{30B7}\u{30C3}\u{30AF} W3.ttc",
            "/System/Library/Fonts/Hiragino Sans GB.ttc",
            "/System/Library/Fonts/PingFang.ttc",
            "/System/Library/Fonts/STHeiti Light.ttc",
            "/Library/Fonts/Arial Unicode.ttf",
        ];

        for path in &candidates {
            if let Ok(data) = std::fs::read(path) {
                match fontdue::Font::from_bytes(data.as_slice(), fontdue::FontSettings::default()) {
                    Ok(font) => {
                        // Verify the font can actually render a common CJK character.
                        if font.lookup_glyph_index('\u{3042}') != 0 {
                            log::info!("loaded CJK fallback font from {path}");
                            return Some(font);
                        }
                        log::debug!("font {path} loaded but lacks CJK glyphs, skipping");
                    }
                    Err(e) => {
                        log::debug!("failed to parse CJK font {path}: {e}");
                    }
                }
            }
        }

        log::info!("no CJK fallback font found, CJK characters may not render");
        None
    }

    /// Try to load a symbols font for fallback rendering of Braille Patterns,
    /// geometric shapes, arrows, dingbats, and other Unicode symbols that
    /// monospace fonts typically lack. These are used by CLI tools for spinners,
    /// progress bars, and status indicators.
    pub(crate) fn load_symbols_fallback_font() -> Option<fontdue::Font> {
        // On macOS, Apple Symbols covers a wide range of Unicode symbols including
        // Braille Patterns (U+2800-28FF), Geometric Shapes, Arrows, Dingbats, etc.
        // Apple Braille is specifically for Braille Patterns.
        // LastResort covers virtually all Unicode as a final fallback.
        let candidates = [
            "/System/Library/Fonts/Apple Symbols.ttf",
            "/System/Library/Fonts/Apple Braille.ttf",
            "/System/Library/Fonts/LastResort.otf",
            // Linux fallbacks
            "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf",
            "/usr/share/fonts/TTF/DejaVuSans.ttf",
            "/usr/share/fonts/truetype/noto/NotoSansSymbols2-Regular.ttf",
        ];

        for path in &candidates {
            if let Ok(data) = std::fs::read(path) {
                match fontdue::Font::from_bytes(data.as_slice(), fontdue::FontSettings::default()) {
                    Ok(font) => {
                        // Verify the font can render a Braille Pattern character
                        // (U+280B = ⠋, commonly used in CLI spinners).
                        if font.lookup_glyph_index('\u{280B}') != 0 {
                            log::info!("loaded symbols fallback font from {path}");
                            return Some(font);
                        }
                        log::debug!("font {path} loaded but lacks Braille glyphs, skipping");
                    }
                    Err(e) => {
                        log::debug!("failed to parse symbols font {path}: {e}");
                    }
                }
            }
        }

        log::info!("no symbols fallback font found, Braille/symbol characters may not render");
        None
    }
}
