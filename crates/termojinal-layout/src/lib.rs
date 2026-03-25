//! Immutable split pane tree for termojinal.
//!
//! All mutation methods return a new tree (functional / persistent style).
//! Internal nodes represent splits (direction + ratio + two children).
//! Leaf nodes represent panes (identified by [`PaneId`]).

pub mod types;
pub(crate) mod node;
pub mod tree;
mod tests;

pub use types::{Direction, PaneId, Rect, SplitDirection};
pub use tree::LayoutTree;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

pub(crate) const MIN_PANE_SIZE: f32 = 50.0;

/// Split a rectangle into two sub-rectangles.
pub(crate) fn split_rect(rect: Rect, dir: SplitDirection, ratio: f32) -> (Rect, Rect) {
    match dir {
        SplitDirection::Horizontal => {
            let w1 = rect.w * ratio;
            let w2 = rect.w - w1;
            (
                Rect {
                    x: rect.x,
                    y: rect.y,
                    w: w1,
                    h: rect.h,
                },
                Rect {
                    x: rect.x + w1,
                    y: rect.y,
                    w: w2,
                    h: rect.h,
                },
            )
        }
        SplitDirection::Vertical => {
            let h1 = rect.h * ratio;
            let h2 = rect.h - h1;
            (
                Rect {
                    x: rect.x,
                    y: rect.y,
                    w: rect.w,
                    h: h1,
                },
                Rect {
                    x: rect.x,
                    y: rect.y + h1,
                    w: rect.w,
                    h: h2,
                },
            )
        }
    }
}

/// Clamp ratio so neither child is smaller than `min_px` pixels.
pub(crate) fn clamp_ratio(ratio: f32, total: f32, min_px: f32) -> f32 {
    if total <= 0.0 {
        return 0.5;
    }
    let min_ratio = min_px / total;
    let max_ratio = 1.0 - min_ratio;
    if min_ratio > max_ratio {
        return 0.5;
    }
    ratio.clamp(min_ratio, max_ratio)
}
