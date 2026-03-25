//! Tests for the Atlas.

#[cfg(test)]
mod tests {
    use crate::atlas::*;

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
            for c in (32u8..=126).map(|b| b as char) {
                assert!(
                    atlas.has_glyph(c),
                    "Missing glyph for '{c}' (0x{:02X})",
                    c as u32
                );
            }
        }
    }

    #[test]
    fn test_all_glyphs_are_cell_sized() {
        if let Some(atlas) = try_create_atlas() {
            let cell_w = atlas.cell_size.width;
            let cell_h = atlas.cell_size.height;
            for (&c, &info) in &atlas.glyphs {
                assert!(
                    (info.atlas_w - cell_w).abs() < 0.01 && (info.atlas_h - cell_h).abs() < 0.01,
                    "Glyph '{c}' has atlas size {}x{}, expected {}x{}",
                    info.atlas_w,
                    info.atlas_h,
                    cell_w,
                    cell_h
                );
            }
        }
    }

    #[test]
    fn test_on_demand_rasterize() {
        if let Some(mut atlas) = try_create_atlas() {
            let initial_count = atlas.glyph_count();
            let _info = atlas.get_glyph('\u{2500}');
            assert!(atlas.glyph_count() > initial_count);
        }
    }

    #[test]
    fn test_glyph_info_a() {
        if let Some(atlas) = try_create_atlas() {
            let glyph = atlas.glyphs.get(&'A').expect("'A' glyph missing");
            assert!(glyph.atlas_w > 0.0);
            assert!(glyph.atlas_h > 0.0);
        }
    }

    #[test]
    fn test_braille_spinner_chars_rasterize() {
        // Braille Pattern characters used by CLI spinners (e.g. Claude Code thinking animation).
        if let Some(mut atlas) = try_create_atlas() {
            let braille_spinner = [
                '\u{280B}', // ⠋
                '\u{2819}', // ⠙
                '\u{2839}', // ⠹
                '\u{2838}', // ⠸
                '\u{283C}', // ⠼
                '\u{2834}', // ⠴
                '\u{2826}', // ⠦
                '\u{2827}', // ⠧
                '\u{2807}', // ⠇
                '\u{280F}', // ⠏
            ];

            for &c in &braille_spinner {
                let initial = atlas.glyph_count();
                let glyph = atlas.get_glyph(c);
                assert!(
                    glyph.atlas_w > 0.0 && glyph.atlas_h > 0.0,
                    "Braille char '{}' (U+{:04X}) must have non-zero atlas size",
                    c,
                    c as u32
                );
                // Verify the glyph was actually rasterized (not just a blank slot).
                assert!(
                    atlas.has_glyph(c),
                    "Braille char '{}' (U+{:04X}) must be cached in atlas",
                    c,
                    c as u32
                );
                // Verify the atlas bitmap has non-zero pixels for this glyph
                // (i.e., actual glyph data, not just a transparent empty cell).
                let ax = glyph.atlas_x as u32;
                let ay = glyph.atlas_y as u32;
                let aw = glyph.atlas_w as u32;
                let ah = glyph.atlas_h as u32;
                let mut nonzero = 0usize;
                for row in ay..(ay + ah).min(atlas.height) {
                    for col in ax..(ax + aw).min(atlas.width) {
                        let idx = (row * atlas.width + col) as usize;
                        if idx < atlas.data.len() && atlas.data[idx] > 0 {
                            nonzero += 1;
                        }
                    }
                }
                assert!(
                    nonzero > 0,
                    "Braille char '{}' (U+{:04X}) must have visible pixels (got 0 non-zero)",
                    c,
                    c as u32
                );
                eprintln!(
                    "Braille '{}' (U+{:04X}): atlas_w={}, atlas_h={}, nonzero_pixels={}, new={}",
                    c,
                    c as u32,
                    glyph.atlas_w,
                    glyph.atlas_h,
                    nonzero,
                    atlas.glyph_count() > initial
                );
            }
        }
    }

    #[test]
    fn test_needs_fallback_check_includes_braille() {
        // Verify Braille Patterns are included in fallback check.
        assert!(Atlas::needs_fallback_check('\u{2800}')); // Empty braille
        assert!(Atlas::needs_fallback_check('\u{280B}')); // ⠋ (spinner)
        assert!(Atlas::needs_fallback_check('\u{28FF}')); // End of braille range

        // Also verify other symbol ranges used by CLI tools.
        assert!(Atlas::needs_fallback_check('\u{2714}')); // ✔ check mark
        assert!(Atlas::needs_fallback_check('\u{2718}')); // ✘ ballot x
        assert!(Atlas::needs_fallback_check('\u{25CF}')); // ● black circle
        assert!(Atlas::needs_fallback_check('\u{2190}')); // ← left arrow
        assert!(Atlas::needs_fallback_check('\u{2588}')); // █ full block (already handled)
    }

    #[test]
    fn test_problem_chars_rendering() {
        if let Some(mut atlas) = try_create_atlas() {
            let chars = [
                ('\u{276F}', "❯ Heavy Right-Pointing Angle"),
                ('\u{25EF}', "◯ Large Circle"),
                ('\u{2461}', "② Circled Digit Two"),
                ('\u{2713}', "✓ Check Mark"),
                ('\u{25B6}', "▶ Right Triangle"),
            ];
            for (c, name) in &chars {
                // Check primary font
                let (m, bmp) = atlas.font.rasterize(*c, atlas.font_size);
                let glyph_idx = atlas.font.lookup_glyph_index(*c);
                let nonzero_primary = bmp.iter().filter(|&&b| b > 0).count();
                eprintln!(
                    "{} U+{:04X} {:40} primary: glyph_idx={} {}x{} nonzero={}",
                    c, *c as u32, name, glyph_idx, m.width, m.height, nonzero_primary
                );
                // Check symbols font
                if let Some(ref sym) = atlas.symbols_font {
                    let sym_idx = sym.lookup_glyph_index(*c);
                    if sym_idx != 0 {
                        let (sm, sbmp) = sym.rasterize(*c, atlas.font_size);
                        let nonzero_sym = sbmp.iter().filter(|&&b| b > 0).count();
                        eprintln!(
                            "  symbols: glyph_idx={} {}x{} nonzero={}",
                            sym_idx, sm.width, sm.height, nonzero_sym
                        );
                    } else {
                        eprintln!("  symbols: not found");
                    }
                }
                // Check CJK font
                if let Some(ref cjk) = atlas.cjk_font {
                    let cjk_idx = cjk.lookup_glyph_index(*c);
                    if cjk_idx != 0 {
                        let (cm, cbmp) = cjk.rasterize(*c, atlas.font_size);
                        let nonzero_cjk = cbmp.iter().filter(|&&b| b > 0).count();
                        eprintln!(
                            "  cjk:     glyph_idx={} {}x{} nonzero={}",
                            cjk_idx, cm.width, cm.height, nonzero_cjk
                        );
                    } else {
                        eprintln!("  cjk:     not found");
                    }
                }
                // Now get the final glyph through the normal pipeline
                let glyph = atlas.get_glyph(*c);
                eprintln!(
                    "  final:   atlas_w={} atlas_h={}",
                    glyph.atlas_w, glyph.atlas_h
                );
                eprintln!();
            }
        }
    }

    #[test]
    fn test_packing_no_overlap() {
        if let Some(atlas) = try_create_atlas() {
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
                        "Overlap: ({},{},{},{}) vs ({},{},{},{})",
                        a.atlas_x,
                        a.atlas_y,
                        a.atlas_w,
                        a.atlas_h,
                        b.atlas_x,
                        b.atlas_y,
                        b.atlas_w,
                        b.atlas_h,
                    );
                }
            }
        }
    }
}
