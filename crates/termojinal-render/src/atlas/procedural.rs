//! Procedural glyph rendering for block elements, shade characters, and box-drawing.

use super::{Atlas, GlyphInfo};

impl Atlas {
    /// Try to draw block elements, shade characters, box-drawing characters,
    /// and commonly-misrendered symbols procedurally.
    /// Returns None if the character is not handled.
    pub(crate) fn try_procedural_block(&mut self, c: char) -> Option<GlyphInfo> {
        let w = self.cell_w;
        let h = self.cell_h;
        let hw = w / 2; // half width
        let hh = h / 2; // half height

        // --- Characters that need procedural rendering for correctness ---
        // Backslash (U+005C): many Japanese fonts map this to ¥ (yen sign).
        // Render procedurally to guarantee a proper reverse solidus.
        if c == '\\' {
            return self.procedural_backslash();
        }
        // ⏵ (U+23F5 BLACK MEDIUM RIGHT-POINTING TRIANGLE): often missing from
        // monospace/system fonts, displays as '?' on many systems.
        if c == '\u{23F5}' {
            return self.procedural_right_pointing_triangle();
        }
        // ✔ (U+2714 HEAVY CHECK MARK): Apple Color Emoji renders this upside-down
        // due to sbix bitmap orientation issues; also missing from many fonts.
        if c == '\u{2714}' {
            return self.procedural_check_mark();
        }
        // ⏺ (U+23FA BLACK CIRCLE FOR RECORD): often missing from monospace/system
        // fonts, displays as '?' on many systems.
        if c == '\u{23FA}' {
            return self.procedural_filled_circle();
        }

        // --- Shade characters ---
        let shade = match c {
            '\u{2591}' => Some(64u8),  // LIGHT SHADE ~25%
            '\u{2592}' => Some(128u8), // MEDIUM SHADE ~50%
            '\u{2593}' => Some(192u8), // DARK SHADE ~75%
            _ => None,
        };
        if let Some(alpha) = shade {
            let mut bitmap = vec![0u8; (w * h) as usize];
            for pixel in &mut bitmap {
                *pixel = alpha;
            }
            let info = self.pack_cell_bitmap(&bitmap, w, h);
            return Some(info);
        }

        // --- Box-drawing characters (U+2500–U+257F) ---
        // Draw lines that extend to the exact cell edges to ensure seamless joining.
        if c >= '\u{2500}' && c <= '\u{257F}' {
            return self.try_procedural_box_drawing(c);
        }

        // --- Block elements (U+2580–U+259F) ---
        let regions: Vec<(u32, u32, u32, u32)> = match c {
            '\u{2588}' => vec![(0, 0, w, h)],   // FULL BLOCK
            '\u{2580}' => vec![(0, 0, w, hh)],  // UPPER HALF
            '\u{2584}' => vec![(0, hh, w, h)],  // LOWER HALF
            '\u{258C}' => vec![(0, 0, hw, h)],  // LEFT HALF
            '\u{2590}' => vec![(hw, 0, w, h)],  // RIGHT HALF
            '\u{2596}' => vec![(0, hh, hw, h)], // QUADRANT LOWER LEFT
            '\u{2597}' => vec![(hw, hh, w, h)], // QUADRANT LOWER RIGHT
            '\u{2598}' => vec![(0, 0, hw, hh)], // QUADRANT UPPER LEFT
            '\u{259D}' => vec![(hw, 0, w, hh)], // QUADRANT UPPER RIGHT
            '\u{2599}' => vec![(0, 0, hw, h), (hw, hh, w, h)],
            '\u{259B}' => vec![(0, 0, w, hh), (0, hh, hw, h)],
            '\u{259C}' => vec![(0, 0, w, hh), (hw, hh, w, h)],
            '\u{259F}' => vec![(hw, 0, w, hh), (0, hh, w, h)],
            _ => return None,
        };

        if regions.is_empty() {
            return None;
        }

        let mut bitmap = vec![0u8; (w * h) as usize];
        for (x0, y0, x1, y1) in &regions {
            for y in *y0..*y1 {
                for x in *x0..*x1 {
                    if x < w && y < h {
                        bitmap[(y * w + x) as usize] = 255;
                    }
                }
            }
        }
        let info = self.pack_cell_bitmap(&bitmap, w, h);
        Some(info)
    }

    /// Draw box-drawing characters procedurally.
    /// Lines extend to the exact cell edges for seamless joining between cells.
    pub(crate) fn try_procedural_box_drawing(&mut self, c: char) -> Option<GlyphInfo> {
        let w = self.cell_w;
        let h = self.cell_h;
        let cx = w / 2; // center x
        let cy = h / 2; // center y

        // Line thickness: thin = 1px, heavy = 2-3px depending on cell size.
        let thin = 1u32.max(w / 10);
        let heavy = (thin * 2).max(2).min(w / 4);

        // Segments: (left, right, up, down) with thickness.
        // 0 = none, 1 = thin, 2 = heavy, 3 = double
        let (left, right, up, down) = match c {
            '\u{2500}' => (1, 1, 0, 0), // horizontal thin
            '\u{2501}' => (2, 2, 0, 0), // horizontal heavy
            '\u{2502}' => (0, 0, 1, 1), // vertical thin
            '\u{2503}' => (0, 0, 2, 2), // vertical heavy
            '\u{250C}' => (0, 1, 0, 1), // top-left thin
            '\u{2510}' => (1, 0, 0, 1), // top-right thin
            '\u{2514}' => (0, 1, 1, 0), // bottom-left thin
            '\u{2518}' => (1, 0, 1, 0), // bottom-right thin
            '\u{251C}' => (0, 1, 1, 1), // left-T thin
            '\u{2524}' => (1, 0, 1, 1), // right-T thin
            '\u{252C}' => (1, 1, 0, 1), // top-T thin
            '\u{2534}' => (1, 1, 1, 0), // bottom-T thin
            '\u{253C}' => (1, 1, 1, 1), // cross thin
            '\u{254C}' => (1, 1, 0, 0), // dashed horizontal (draw as solid)
            '\u{254E}' => (0, 0, 1, 1), // dashed vertical (draw as solid)
            '\u{2504}' => (1, 1, 0, 0), // triple-dash horizontal
            '\u{2506}' => (0, 0, 1, 1), // triple-dash vertical
            '\u{2508}' => (1, 1, 0, 0), // quad-dash horizontal
            '\u{250A}' => (0, 0, 1, 1), // quad-dash vertical
            // Heavy corners
            '\u{250D}' => (0, 2, 0, 1),
            '\u{250E}' => (0, 1, 0, 2),
            '\u{250F}' => (0, 2, 0, 2),
            '\u{2511}' => (2, 0, 0, 1),
            '\u{2512}' => (1, 0, 0, 2),
            '\u{2513}' => (2, 0, 0, 2),
            '\u{2515}' => (0, 2, 1, 0),
            '\u{2516}' => (0, 1, 2, 0),
            '\u{2517}' => (0, 2, 2, 0),
            '\u{2519}' => (2, 0, 1, 0),
            '\u{251A}' => (1, 0, 2, 0),
            '\u{251B}' => (2, 0, 2, 0),
            // Heavy T-junctions
            '\u{251D}' => (0, 2, 1, 1),
            '\u{251E}' => (0, 1, 2, 1),
            '\u{251F}' => (0, 1, 1, 2),
            '\u{2520}' => (0, 1, 2, 2),
            '\u{2521}' => (0, 2, 2, 1),
            '\u{2522}' => (0, 2, 1, 2),
            '\u{2523}' => (0, 2, 2, 2),
            '\u{2525}' => (2, 0, 1, 1),
            '\u{2526}' => (1, 0, 2, 1),
            '\u{2527}' => (1, 0, 1, 2),
            '\u{2528}' => (1, 0, 2, 2),
            '\u{2529}' => (2, 0, 2, 1),
            '\u{252A}' => (2, 0, 1, 2),
            '\u{252B}' => (2, 0, 2, 2),
            '\u{252D}' => (2, 1, 0, 1),
            '\u{252E}' => (1, 2, 0, 1),
            '\u{252F}' => (2, 2, 0, 1),
            '\u{2530}' => (1, 1, 0, 2),
            '\u{2531}' => (2, 1, 0, 2),
            '\u{2532}' => (1, 2, 0, 2),
            '\u{2533}' => (2, 2, 0, 2),
            '\u{2535}' => (2, 1, 1, 0),
            '\u{2536}' => (1, 2, 1, 0),
            '\u{2537}' => (2, 2, 1, 0),
            '\u{2538}' => (1, 1, 2, 0),
            '\u{2539}' => (2, 1, 2, 0),
            '\u{253A}' => (1, 2, 2, 0),
            '\u{253B}' => (2, 2, 2, 0),
            // Heavy crosses
            '\u{253D}' => (2, 1, 1, 1),
            '\u{253E}' => (1, 2, 1, 1),
            '\u{253F}' => (2, 2, 1, 1),
            '\u{2540}' => (1, 1, 2, 1),
            '\u{2541}' => (1, 1, 1, 2),
            '\u{2542}' => (1, 1, 2, 2),
            '\u{2543}' => (2, 1, 2, 1),
            '\u{2544}' => (1, 2, 2, 1),
            '\u{2545}' => (2, 1, 1, 2),
            '\u{2546}' => (1, 2, 1, 2),
            '\u{2547}' => (2, 2, 2, 1),
            '\u{2548}' => (2, 2, 1, 2),
            '\u{2549}' => (2, 1, 2, 2),
            '\u{254A}' => (1, 2, 2, 2),
            '\u{254B}' => (2, 2, 2, 2),
            // Double lines
            '\u{2550}' => (3, 3, 0, 0), // double horizontal
            '\u{2551}' => (0, 0, 3, 3), // double vertical
            '\u{2554}' => (0, 3, 0, 3),
            '\u{2557}' => (3, 0, 0, 3),
            '\u{255A}' => (0, 3, 3, 0),
            '\u{255D}' => (3, 0, 3, 0),
            '\u{2560}' => (0, 3, 3, 3),
            '\u{2563}' => (3, 0, 3, 3),
            '\u{2566}' => (3, 3, 0, 3),
            '\u{2569}' => (3, 3, 3, 0),
            '\u{256C}' => (3, 3, 3, 3),
            // Mixed single/double
            '\u{2552}' => (0, 3, 0, 1),
            '\u{2553}' => (0, 1, 0, 3),
            '\u{2555}' => (3, 0, 0, 1),
            '\u{2556}' => (1, 0, 0, 3),
            '\u{2558}' => (0, 3, 1, 0),
            '\u{2559}' => (0, 1, 3, 0),
            '\u{255B}' => (3, 0, 1, 0),
            '\u{255C}' => (1, 0, 3, 0),
            '\u{255E}' => (0, 3, 1, 1),
            '\u{255F}' => (0, 1, 3, 3),
            '\u{2561}' => (3, 0, 1, 1),
            '\u{2562}' => (1, 0, 3, 3),
            '\u{2564}' => (3, 3, 0, 1),
            '\u{2565}' => (1, 1, 0, 3),
            '\u{2567}' => (3, 3, 1, 0),
            '\u{2568}' => (1, 1, 3, 0),
            '\u{256A}' => (3, 3, 1, 1),
            '\u{256B}' => (1, 1, 3, 3),
            // Rounded corners
            '\u{256D}' => (0, 1, 0, 1),
            '\u{256E}' => (1, 0, 0, 1),
            '\u{256F}' => (1, 0, 1, 0),
            '\u{2570}' => (0, 1, 1, 0),
            _ => return None,
        };

        let mut bitmap = vec![0u8; (w * h) as usize];

        // Helper: draw a filled rect into bitmap
        let mut fill = |x0: u32, y0: u32, x1: u32, y1: u32| {
            for y in y0..y1.min(h) {
                for x in x0..x1.min(w) {
                    bitmap[(y * w + x) as usize] = 255;
                }
            }
        };

        let draw_segment = |fill: &mut dyn FnMut(u32, u32, u32, u32),
                            dir: u32,
                            thickness: u32,
                            is_double: bool| {
            if is_double {
                let gap = (thickness + 1).max(2);
                let t = thickness.max(1);
                match dir {
                    0 => {
                        // left
                        fill(0, cy - gap, cx, cy - gap + t);
                        fill(0, cy + gap - t, cx, cy + gap);
                    }
                    1 => {
                        // right
                        fill(cx, cy - gap, w, cy - gap + t);
                        fill(cx, cy + gap - t, w, cy + gap);
                    }
                    2 => {
                        // up
                        fill(cx - gap, 0, cx - gap + t, cy);
                        fill(cx + gap - t, 0, cx + gap, cy);
                    }
                    3 => {
                        // down
                        fill(cx - gap, cy, cx - gap + t, h);
                        fill(cx + gap - t, cy, cx + gap, h);
                    }
                    _ => {}
                }
            } else {
                let half_t = thickness / 2;
                match dir {
                    0 => fill(
                        0,
                        cy.saturating_sub(half_t),
                        cx + half_t,
                        cy + thickness - half_t,
                    ), // left
                    1 => fill(
                        cx.saturating_sub(half_t),
                        cy.saturating_sub(half_t),
                        w,
                        cy + thickness - half_t,
                    ), // right
                    2 => fill(
                        cx.saturating_sub(half_t),
                        0,
                        cx + thickness - half_t,
                        cy + half_t,
                    ), // up
                    3 => fill(
                        cx.saturating_sub(half_t),
                        cy.saturating_sub(half_t),
                        cx + thickness - half_t,
                        h,
                    ), // down
                    _ => {}
                }
            }
        };

        // Draw each segment.
        let segments = [(0u32, left), (1, right), (2, up), (3, down)];
        for (dir, style) in segments {
            if style == 0 {
                continue;
            }
            let is_double = style == 3;
            let thickness = if style == 2 { heavy } else { thin };
            draw_segment(&mut fill, dir, thickness, is_double);
        }

        let info = self.pack_cell_bitmap(&bitmap, w, h);
        Some(info)
    }

    /// Draw backslash (U+005C) procedurally as a diagonal line from
    /// upper-right to lower-left, avoiding the ¥ rendering bug in Japanese fonts.
    fn procedural_backslash(&mut self) -> Option<GlyphInfo> {
        let w = self.cell_w;
        let h = self.cell_h;
        let mut bitmap = vec![0u8; (w * h) as usize];

        // Use font metrics: draw within the ascent area, matching how a font
        // would render the character.  Leave some padding at edges.
        let pad_x = (w as f32 * 0.15).round() as u32;
        let pad_top = (self.cell_h as f32 * 0.15).round() as u32;
        let pad_bot = (self.cell_h as f32 * 0.10).round() as u32;

        let x0 = pad_x as f32;
        let y0 = pad_top as f32;
        let x1 = (w - pad_x) as f32;
        let y1 = (h - pad_bot) as f32;

        // Stroke thickness proportional to cell width, minimum 1px.
        let thickness = (w as f32 * 0.10).ceil().max(1.0);

        // Draw anti-aliased diagonal line using distance from line segment.
        let dx = x1 - x0;
        let dy = y1 - y0;
        let len = (dx * dx + dy * dy).sqrt();

        for row in 0..h {
            for col in 0..w {
                let px = col as f32 + 0.5;
                let py = row as f32 + 0.5;
                // Distance from point to line segment.
                let t = ((px - x0) * dx + (py - y0) * dy) / (len * len);
                let t = t.clamp(0.0, 1.0);
                let closest_x = x0 + t * dx;
                let closest_y = y0 + t * dy;
                let dist = ((px - closest_x).powi(2) + (py - closest_y).powi(2)).sqrt();
                let half_t = thickness / 2.0;
                let alpha = (half_t + 0.5 - dist).clamp(0.0, 1.0);
                if alpha > 0.0 {
                    bitmap[(row * w + col) as usize] = (alpha * 255.0) as u8;
                }
            }
        }

        let info = self.pack_cell_bitmap(&bitmap, w, h);
        Some(info)
    }

    /// Draw ⏵ (U+23F5 BLACK MEDIUM RIGHT-POINTING TRIANGLE) procedurally.
    fn procedural_right_pointing_triangle(&mut self) -> Option<GlyphInfo> {
        let w = self.cell_w;
        let h = self.cell_h;
        let mut bitmap = vec![0u8; (w * h) as usize];

        // Triangle pointing right, centered in cell with some padding.
        let pad_x = (w as f32 * 0.20).round() as f32;
        let pad_y = (h as f32 * 0.20).round() as f32;

        let left = pad_x;
        let right = w as f32 - pad_x;
        let top = pad_y;
        let bottom = h as f32 - pad_y;
        let mid_y = (top + bottom) / 2.0;

        // Triangle vertices: left-top, left-bottom, right-center
        for row in 0..h {
            for col in 0..w {
                let px = col as f32 + 0.5;
                let py = row as f32 + 0.5;

                // Check if point is inside the triangle using edge tests.
                // Edge 1: left-top to right-center (top edge)
                let e1 = (right - left) * (py - top) - (mid_y - top) * (px - left);
                // Edge 2: right-center to left-bottom (right-bottom edge)
                let e2 = (left - right) * (py - mid_y) - (bottom - mid_y) * (px - right);
                let inside = e1 >= -0.5 && e2 >= -0.5 && px >= left - 0.5;

                if inside {
                    // Anti-alias edges using distance to nearest edge.
                    // Normalize: approximate distance in pixels.
                    let tri_h = bottom - top;
                    let tri_w = right - left;
                    let edge_len1 = (tri_w * tri_w + (tri_h / 2.0).powi(2)).sqrt();
                    let d1 = e1 / edge_len1;
                    let d2 = e2 / edge_len1;
                    let d3 = px - left;
                    let min_d = d1.min(d2).min(d3);
                    let alpha = min_d.clamp(0.0, 1.0);
                    bitmap[(row * w + col) as usize] = (alpha * 255.0) as u8;
                }
            }
        }

        let info = self.pack_cell_bitmap(&bitmap, w, h);
        Some(info)
    }

    /// Draw ✔ (U+2714 HEAVY CHECK MARK) procedurally.
    fn procedural_check_mark(&mut self) -> Option<GlyphInfo> {
        let w = self.cell_w;
        let h = self.cell_h;
        let mut bitmap = vec![0u8; (w * h) as usize];

        // Check mark shape: short stroke going down-right, then long stroke going up-right.
        // Coordinates relative to cell.
        let pad_x = (w as f32 * 0.10).round();
        let pad_y = (h as f32 * 0.15).round();

        // The "valley" point where the two strokes meet.
        let valley_x = pad_x + (w as f32 - 2.0 * pad_x) * 0.30;
        let valley_y = h as f32 - pad_y;

        // Left endpoint (start of short stroke, upper-left of valley).
        let left_x = pad_x;
        let left_y = h as f32 * 0.50;

        // Right endpoint (end of long stroke, upper-right).
        let right_x = w as f32 - pad_x;
        let right_y = pad_y;

        let thickness = (w as f32 * 0.14).ceil().max(1.5);

        // Draw two line segments with anti-aliasing.
        let segments: [(f32, f32, f32, f32); 2] = [
            (left_x, left_y, valley_x, valley_y),
            (valley_x, valley_y, right_x, right_y),
        ];

        for row in 0..h {
            for col in 0..w {
                let px = col as f32 + 0.5;
                let py = row as f32 + 0.5;

                let mut min_dist = f32::MAX;
                for &(x0, y0, x1, y1) in &segments {
                    let dx = x1 - x0;
                    let dy = y1 - y0;
                    let len_sq = dx * dx + dy * dy;
                    let t = if len_sq > 0.0 {
                        ((px - x0) * dx + (py - y0) * dy) / len_sq
                    } else {
                        0.0
                    };
                    let t = t.clamp(0.0, 1.0);
                    let cx = x0 + t * dx;
                    let cy = y0 + t * dy;
                    let dist = ((px - cx).powi(2) + (py - cy).powi(2)).sqrt();
                    min_dist = min_dist.min(dist);
                }

                let half_t = thickness / 2.0;
                let alpha = (half_t + 0.5 - min_dist).clamp(0.0, 1.0);
                if alpha > 0.0 {
                    let idx = (row * w + col) as usize;
                    bitmap[idx] = (alpha * 255.0).min(255.0) as u8;
                }
            }
        }

        let info = self.pack_cell_bitmap(&bitmap, w, h);
        Some(info)
    }

    /// Draw ⏺ (U+23FA BLACK CIRCLE FOR RECORD) procedurally.
    fn procedural_filled_circle(&mut self) -> Option<GlyphInfo> {
        let w = self.cell_w;
        let h = self.cell_h;
        let mut bitmap = vec![0u8; (w * h) as usize];

        let cx = w as f32 / 2.0;
        let cy = h as f32 / 2.0;
        // Radius: fit within the cell with some padding.
        let radius = (w.min(h) as f32 / 2.0) * 0.72;

        for row in 0..h {
            for col in 0..w {
                let px = col as f32 + 0.5;
                let py = row as f32 + 0.5;
                let dist = ((px - cx).powi(2) + (py - cy).powi(2)).sqrt();
                // Anti-alias the edge: fully opaque inside, smooth transition at edge.
                let alpha = (radius + 0.5 - dist).clamp(0.0, 1.0);
                if alpha > 0.0 {
                    bitmap[(row * w + col) as usize] = (alpha * 255.0) as u8;
                }
            }
        }

        let info = self.pack_cell_bitmap(&bitmap, w, h);
        Some(info)
    }
}
