//! Convert `jterm_vt::Color` to GPU-friendly `[f32; 4]` RGBA values.
//!
//! Contains the full xterm 256-color palette as a constant lookup table,
//! and a `ThemePalette` for configurable ANSI 16-color overrides.

use jterm_vt::Color;

/// Default foreground color (light gray) — used as fallback when no theme is loaded.
pub const DEFAULT_FG: [f32; 4] = [0.85, 0.85, 0.85, 1.0];

/// Default background color (near-black) — used as fallback when no theme is loaded.
pub const DEFAULT_BG: [f32; 4] = [0.067, 0.067, 0.09, 1.0];

/// The 16 standard ANSI colors as (R, G, B) u8 triples.
/// Indices 0-7: normal, 8-15: bright.
const ANSI_16: [[u8; 3]; 16] = [
    [0, 0, 0],       // 0  Black
    [205, 49, 49],   // 1  Red
    [13, 188, 121],  // 2  Green
    [229, 229, 16],  // 3  Yellow
    [36, 114, 200],  // 4  Blue
    [188, 63, 188],  // 5  Magenta
    [17, 168, 205],  // 6  Cyan
    [229, 229, 229], // 7  White
    [102, 102, 102], // 8  Bright Black
    [241, 76, 76],   // 9  Bright Red
    [35, 209, 139],  // 10 Bright Green
    [245, 245, 67],  // 11 Bright Yellow
    [59, 142, 234],  // 12 Bright Blue
    [214, 112, 214], // 13 Bright Magenta
    [41, 184, 219],  // 14 Bright Cyan
    [255, 255, 255], // 15 Bright White
];

/// Full xterm 256-color palette as [R, G, B] u8 triples.
///
/// - Indices 0-15: standard ANSI colors
/// - Indices 16-231: 6x6x6 color cube
/// - Indices 232-255: grayscale ramp
const XTERM_256: [[u8; 3]; 256] = {
    let mut table = [[0u8; 3]; 256];

    // 0-15: ANSI colors
    let ansi = ANSI_16;
    let mut i = 0;
    while i < 16 {
        table[i] = ansi[i];
        i += 1;
    }

    // 16-231: 6x6x6 color cube
    // For each of 216 colors: index = 16 + 36*r + 6*g + b
    // where r, g, b in 0..6
    // Mapping: 0 -> 0, 1 -> 95, 2 -> 135, 3 -> 175, 4 -> 215, 5 -> 255
    let cube_values: [u8; 6] = [0, 95, 135, 175, 215, 255];
    let mut idx = 16;
    let mut r = 0usize;
    while r < 6 {
        let mut g = 0usize;
        while g < 6 {
            let mut b = 0usize;
            while b < 6 {
                table[idx] = [cube_values[r], cube_values[g], cube_values[b]];
                idx += 1;
                b += 1;
            }
            g += 1;
        }
        r += 1;
    }

    // 232-255: grayscale ramp (8 + 10*i for i in 0..24)
    let mut gi = 0usize;
    while gi < 24 {
        let v = (8 + 10 * gi) as u8;
        table[232 + gi] = [v, v, v];
        gi += 1;
    }

    table
};

// ---------------------------------------------------------------------------
// ThemePalette
// ---------------------------------------------------------------------------

/// A configurable color palette holding the ANSI 16 colors plus default bg/fg.
///
/// Created from a theme configuration section. The renderer holds one of these
/// and passes it to color conversion functions so that named/indexed ANSI colors
/// respect the user's theme.
#[derive(Debug, Clone)]
pub struct ThemePalette {
    /// The 16 ANSI colors as `[f32; 4]` RGBA arrays. Index matches `NamedColor as u8`.
    pub ansi: [[f32; 4]; 16],
    /// Default foreground color.
    pub fg: [f32; 4],
    /// Default background color.
    pub bg: [f32; 4],
}

impl Default for ThemePalette {
    fn default() -> Self {
        let mut ansi = [[0.0f32; 4]; 16];
        for (i, &[r, g, b]) in ANSI_16.iter().enumerate() {
            ansi[i] = [r as f32 / 255.0, g as f32 / 255.0, b as f32 / 255.0, 1.0];
        }
        Self {
            ansi,
            fg: DEFAULT_FG,
            bg: DEFAULT_BG,
        }
    }
}

impl ThemePalette {
    /// Parse a hex color string (#RGB, #RRGGBB, or #RRGGBBAA) to `[f32; 4]`.
    fn parse_hex(s: &str) -> Option<[f32; 4]> {
        let s = s.trim_start_matches('#');
        if s.len() == 3 {
            let r = u8::from_str_radix(&s[0..1], 16).ok()?;
            let g = u8::from_str_radix(&s[1..2], 16).ok()?;
            let b = u8::from_str_radix(&s[2..3], 16).ok()?;
            Some([
                (r * 17) as f32 / 255.0,
                (g * 17) as f32 / 255.0,
                (b * 17) as f32 / 255.0,
                1.0,
            ])
        } else if s.len() == 6 {
            let r = u8::from_str_radix(&s[0..2], 16).ok()?;
            let g = u8::from_str_radix(&s[2..4], 16).ok()?;
            let b = u8::from_str_radix(&s[4..6], 16).ok()?;
            Some([r as f32 / 255.0, g as f32 / 255.0, b as f32 / 255.0, 1.0])
        } else if s.len() == 8 {
            let r = u8::from_str_radix(&s[0..2], 16).ok()?;
            let g = u8::from_str_radix(&s[2..4], 16).ok()?;
            let b = u8::from_str_radix(&s[4..6], 16).ok()?;
            let a = u8::from_str_radix(&s[6..8], 16).ok()?;
            Some([r as f32 / 255.0, g as f32 / 255.0, b as f32 / 255.0, a as f32 / 255.0])
        } else {
            None
        }
    }

    /// Helper: parse hex color or return fallback.
    fn hex_or(s: &str, fallback: [f32; 4]) -> [f32; 4] {
        Self::parse_hex(s).unwrap_or(fallback)
    }

    /// Build a `ThemePalette` from theme color strings.
    ///
    /// Each color string should be a hex value like `#RRGGBB`. If a color string
    /// cannot be parsed, the corresponding hardcoded ANSI default is used.
    ///
    /// The field names match `ThemeSection` in `src/config.rs`. This function
    /// takes individual string slices to avoid coupling `jterm-render` to the
    /// main binary's config types.
    pub fn from_theme_colors(
        background: &str,
        foreground: &str,
        black: &str,
        red: &str,
        green: &str,
        yellow: &str,
        blue: &str,
        magenta: &str,
        cyan: &str,
        white: &str,
        bright_black: &str,
        bright_red: &str,
        bright_green: &str,
        bright_yellow: &str,
        bright_blue: &str,
        bright_magenta: &str,
        bright_cyan: &str,
        bright_white: &str,
    ) -> Self {
        let defaults = Self::default();
        let ansi = [
            Self::hex_or(black, defaults.ansi[0]),
            Self::hex_or(red, defaults.ansi[1]),
            Self::hex_or(green, defaults.ansi[2]),
            Self::hex_or(yellow, defaults.ansi[3]),
            Self::hex_or(blue, defaults.ansi[4]),
            Self::hex_or(magenta, defaults.ansi[5]),
            Self::hex_or(cyan, defaults.ansi[6]),
            Self::hex_or(white, defaults.ansi[7]),
            Self::hex_or(bright_black, defaults.ansi[8]),
            Self::hex_or(bright_red, defaults.ansi[9]),
            Self::hex_or(bright_green, defaults.ansi[10]),
            Self::hex_or(bright_yellow, defaults.ansi[11]),
            Self::hex_or(bright_blue, defaults.ansi[12]),
            Self::hex_or(bright_magenta, defaults.ansi[13]),
            Self::hex_or(bright_cyan, defaults.ansi[14]),
            Self::hex_or(bright_white, defaults.ansi[15]),
        ];
        Self {
            ansi,
            fg: Self::hex_or(foreground, defaults.fg),
            bg: Self::hex_or(background, defaults.bg),
        }
    }
}

// ---------------------------------------------------------------------------
// Color conversion (palette-aware)
// ---------------------------------------------------------------------------

/// Convert a `jterm_vt::Color` to an RGBA `[f32; 4]` using the given palette.
///
/// Named ANSI colors (0-15) are resolved through the palette so they respect
/// the user's theme. Indexed colors 0-15 also go through the palette; 16-255
/// use the standard xterm color cube / grayscale ramp.
///
/// `is_fg` determines which default color to use when `Color::Default` is encountered.
pub fn color_to_rgba_themed(color: Color, is_fg: bool, palette: &ThemePalette) -> [f32; 4] {
    match color {
        Color::Default => {
            if is_fg { palette.fg } else { palette.bg }
        }
        Color::Named(named) => {
            let idx = named as u8 as usize;
            palette.ansi[idx]
        }
        Color::Indexed(idx) => {
            if (idx as usize) < 16 {
                palette.ansi[idx as usize]
            } else {
                let [r, g, b] = XTERM_256[idx as usize];
                [r as f32 / 255.0, g as f32 / 255.0, b as f32 / 255.0, 1.0]
            }
        }
        Color::Rgb(r, g, b) => [r as f32 / 255.0, g as f32 / 255.0, b as f32 / 255.0, 1.0],
    }
}

/// Convert a `jterm_vt::Color` to an RGBA `[f32; 4]` suitable for the GPU.
///
/// Uses hardcoded ANSI defaults. Prefer `color_to_rgba_themed()` when a
/// `ThemePalette` is available.
///
/// `is_fg` determines which default color to use when `Color::Default` is encountered.
pub fn color_to_rgba(color: Color, is_fg: bool) -> [f32; 4] {
    match color {
        Color::Default => {
            if is_fg {
                DEFAULT_FG
            } else {
                DEFAULT_BG
            }
        }
        Color::Named(named) => {
            let idx = named as u8 as usize;
            let [r, g, b] = ANSI_16[idx];
            [r as f32 / 255.0, g as f32 / 255.0, b as f32 / 255.0, 1.0]
        }
        Color::Indexed(idx) => {
            let [r, g, b] = XTERM_256[idx as usize];
            [r as f32 / 255.0, g as f32 / 255.0, b as f32 / 255.0, 1.0]
        }
        Color::Rgb(r, g, b) => [r as f32 / 255.0, g as f32 / 255.0, b as f32 / 255.0, 1.0],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_fg() {
        let c = color_to_rgba(Color::Default, true);
        assert_eq!(c, DEFAULT_FG);
    }

    #[test]
    fn test_default_bg() {
        let c = color_to_rgba(Color::Default, false);
        assert_eq!(c, DEFAULT_BG);
    }

    #[test]
    fn test_named_red() {
        let c = color_to_rgba(Color::Named(jterm_vt::NamedColor::Red), true);
        assert!((c[0] - 205.0 / 255.0).abs() < 0.001);
        assert!((c[1] - 49.0 / 255.0).abs() < 0.001);
        assert!((c[2] - 49.0 / 255.0).abs() < 0.001);
        assert_eq!(c[3], 1.0);
    }

    #[test]
    fn test_named_bright_white() {
        let c = color_to_rgba(Color::Named(jterm_vt::NamedColor::BrightWhite), true);
        assert_eq!(c, [1.0, 1.0, 1.0, 1.0]);
    }

    #[test]
    fn test_indexed_16_is_black() {
        // Index 16 = color cube (0,0,0) => RGB(0,0,0)
        let c = color_to_rgba(Color::Indexed(16), true);
        assert_eq!(c, [0.0, 0.0, 0.0, 1.0]);
    }

    #[test]
    fn test_indexed_231() {
        // Index 231 = color cube (5,5,5) => RGB(255,255,255)
        let c = color_to_rgba(Color::Indexed(231), true);
        assert_eq!(c, [1.0, 1.0, 1.0, 1.0]);
    }

    #[test]
    fn test_indexed_grayscale_232() {
        // Index 232 = grayscale 8
        let c = color_to_rgba(Color::Indexed(232), true);
        assert!((c[0] - 8.0 / 255.0).abs() < 0.001);
        assert_eq!(c[0], c[1]);
        assert_eq!(c[1], c[2]);
    }

    #[test]
    fn test_indexed_grayscale_255() {
        // Index 255 = grayscale 8 + 10*23 = 238
        let c = color_to_rgba(Color::Indexed(255), true);
        assert!((c[0] - 238.0 / 255.0).abs() < 0.001);
    }

    #[test]
    fn test_rgb_direct() {
        let c = color_to_rgba(Color::Rgb(128, 64, 32), false);
        assert!((c[0] - 128.0 / 255.0).abs() < 0.001);
        assert!((c[1] - 64.0 / 255.0).abs() < 0.001);
        assert!((c[2] - 32.0 / 255.0).abs() < 0.001);
        assert_eq!(c[3], 1.0);
    }

    #[test]
    fn test_xterm_256_cube_spot_check() {
        // Index 196 = 16 + 36*5 + 6*0 + 0 = 196 => RGB(255, 0, 0) — pure red
        let c = color_to_rgba(Color::Indexed(196), true);
        assert_eq!(c, [1.0, 0.0, 0.0, 1.0]);
    }

    #[test]
    fn test_xterm_256_cube_spot_check_2() {
        // Index 21 = 16 + 36*0 + 6*0 + 5 = 21 => RGB(0, 0, 255) — pure blue
        let c = color_to_rgba(Color::Indexed(21), true);
        assert_eq!(c, [0.0, 0.0, 1.0, 1.0]);
    }

    // --- ThemePalette tests ---

    #[test]
    fn test_palette_default_matches_constants() {
        let palette = ThemePalette::default();
        assert_eq!(palette.fg, DEFAULT_FG);
        assert_eq!(palette.bg, DEFAULT_BG);
    }

    #[test]
    fn test_themed_default_fg() {
        let palette = ThemePalette::default();
        let c = color_to_rgba_themed(Color::Default, true, &palette);
        assert_eq!(c, DEFAULT_FG);
    }

    #[test]
    fn test_themed_default_bg() {
        let palette = ThemePalette::default();
        let c = color_to_rgba_themed(Color::Default, false, &palette);
        assert_eq!(c, DEFAULT_BG);
    }

    #[test]
    fn test_themed_named_uses_palette() {
        let mut palette = ThemePalette::default();
        // Override red to pure red
        palette.ansi[1] = [1.0, 0.0, 0.0, 1.0];
        let c = color_to_rgba_themed(Color::Named(jterm_vt::NamedColor::Red), true, &palette);
        assert_eq!(c, [1.0, 0.0, 0.0, 1.0]);
    }

    #[test]
    fn test_themed_indexed_0_15_uses_palette() {
        let mut palette = ThemePalette::default();
        palette.ansi[4] = [0.5, 0.5, 0.5, 1.0]; // override blue
        let c = color_to_rgba_themed(Color::Indexed(4), true, &palette);
        assert_eq!(c, [0.5, 0.5, 0.5, 1.0]);
    }

    #[test]
    fn test_themed_indexed_16_plus_uses_xterm() {
        let palette = ThemePalette::default();
        // Index 196 should still be pure red from xterm table
        let c = color_to_rgba_themed(Color::Indexed(196), true, &palette);
        assert_eq!(c, [1.0, 0.0, 0.0, 1.0]);
    }

    #[test]
    fn test_themed_rgb_passthrough() {
        let palette = ThemePalette::default();
        let c = color_to_rgba_themed(Color::Rgb(128, 64, 32), false, &palette);
        assert!((c[0] - 128.0 / 255.0).abs() < 0.001);
        assert_eq!(c[3], 1.0);
    }

    #[test]
    fn test_from_theme_colors_parses_hex() {
        let palette = ThemePalette::from_theme_colors(
            "#1E1E2E", "#CDD6F4",
            "#45475A", "#F38BA8", "#A6E3A1", "#F9E2AF",
            "#89B4FA", "#F5C2E7", "#94E2D5", "#BAC2DE",
            "#585B70", "#F38BA8", "#A6E3A1", "#F9E2AF",
            "#89B4FA", "#F5C2E7", "#94E2D5", "#A6ADC8",
        );
        // Background should be Catppuccin Mocha base
        assert!((palette.bg[0] - 0x1E as f32 / 255.0).abs() < 0.001);
        // Red (index 1) should be Catppuccin red
        assert!((palette.ansi[1][0] - 0xF3 as f32 / 255.0).abs() < 0.001);
    }

    #[test]
    fn test_parse_hex_rgb_short() {
        let c = ThemePalette::parse_hex("#F00").unwrap();
        assert_eq!(c, [1.0, 0.0, 0.0, 1.0]);
    }

    #[test]
    fn test_parse_hex_invalid() {
        assert!(ThemePalette::parse_hex("not-a-color").is_none());
        assert!(ThemePalette::parse_hex("#GG0000").is_none());
    }
}
