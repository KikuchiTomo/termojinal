//! Convert `jterm_vt::Color` to GPU-friendly `[f32; 4]` RGBA values.
//!
//! Contains the full xterm 256-color palette as a constant lookup table.

use jterm_vt::Color;

/// Default foreground color (light gray).
pub const DEFAULT_FG: [f32; 4] = [0.85, 0.85, 0.85, 1.0];

/// Default background color (near-black).
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

/// Convert a `jterm_vt::Color` to an RGBA `[f32; 4]` suitable for the GPU.
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
}
