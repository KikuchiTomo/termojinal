//! Procedural glyph rendering for block elements, shade characters, and box-drawing.

use super::{Atlas, GlyphInfo};

impl Atlas {
    /// Try to draw block elements, shade characters, and box-drawing characters
    /// procedurally. Returns None if the character is not handled.
    pub(crate) fn try_procedural_block(&mut self, c: char) -> Option<GlyphInfo> {
        let w = self.cell_w;
        let h = self.cell_h;
        let hw = w / 2; // half width
        let hh = h / 2; // half height

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
}
