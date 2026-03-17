//! Immutable split pane tree for jterm.
//!
//! All mutation methods return a new tree (functional / persistent style).
//! Internal nodes represent splits (direction + ratio + two children).
//! Leaf nodes represent panes (identified by [`PaneId`]).

use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Public types
// ---------------------------------------------------------------------------

/// Unique identifier for a pane.
pub type PaneId = u64;

/// Rectangle in pixel coordinates.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

impl Rect {
    fn center(&self) -> (f32, f32) {
        (self.x + self.w / 2.0, self.y + self.h / 2.0)
    }
}

/// Split direction.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SplitDirection {
    /// Left | Right
    Horizontal,
    /// Top / Bottom
    Vertical,
}

/// Navigation direction.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Direction {
    Up,
    Down,
    Left,
    Right,
    /// Next pane in tree order.
    Next,
    /// Previous pane in tree order.
    Prev,
}

// ---------------------------------------------------------------------------
// Internal tree node (immutable, cheaply cloneable via Box)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
enum Node {
    Leaf(PaneId),
    Split {
        direction: SplitDirection,
        /// Fraction of space given to the *first* (left / top) child (0.0–1.0).
        ratio: f32,
        first: Box<Node>,
        second: Box<Node>,
    },
}

impl Node {
    /// Collect all pane IDs in tree order (left-to-right / top-to-bottom).
    fn pane_ids(&self) -> Vec<PaneId> {
        match self {
            Node::Leaf(id) => vec![*id],
            Node::Split { first, second, .. } => {
                let mut v = first.pane_ids();
                v.extend(second.pane_ids());
                v
            }
        }
    }

    /// Number of leaf panes.
    fn pane_count(&self) -> usize {
        match self {
            Node::Leaf(_) => 1,
            Node::Split { first, second, .. } => first.pane_count() + second.pane_count(),
        }
    }

    /// Does this subtree contain the given pane?
    fn contains(&self, pane: PaneId) -> bool {
        match self {
            Node::Leaf(id) => *id == pane,
            Node::Split { first, second, .. } => first.contains(pane) || second.contains(pane),
        }
    }

    /// Split a leaf pane, returning the new subtree and the newly created PaneId.
    fn split(&self, target: PaneId, dir: SplitDirection, new_id: PaneId) -> (Node, bool) {
        match self {
            Node::Leaf(id) if *id == target => {
                let new_node = Node::Split {
                    direction: dir,
                    ratio: 0.5,
                    first: Box::new(Node::Leaf(*id)),
                    second: Box::new(Node::Leaf(new_id)),
                };
                (new_node, true)
            }
            Node::Leaf(_) => (self.clone(), false),
            Node::Split {
                direction,
                ratio,
                first,
                second,
            } => {
                let (new_first, found) = first.split(target, dir, new_id);
                if found {
                    return (
                        Node::Split {
                            direction: *direction,
                            ratio: *ratio,
                            first: Box::new(new_first),
                            second: second.clone(),
                        },
                        true,
                    );
                }
                let (new_second, found) = second.split(target, dir, new_id);
                (
                    Node::Split {
                        direction: *direction,
                        ratio: *ratio,
                        first: first.clone(),
                        second: Box::new(new_second),
                    },
                    found,
                )
            }
        }
    }

    /// Close a pane. Returns `None` if the pane was the only leaf at root level,
    /// otherwise returns the new subtree.
    fn close(&self, target: PaneId) -> Option<Node> {
        match self {
            Node::Leaf(id) => {
                if *id == target {
                    None // Removed the leaf itself — parent decides what to do.
                } else {
                    Some(self.clone())
                }
            }
            Node::Split {
                direction,
                ratio,
                first,
                second,
            } => {
                if first.contains(target) {
                    match first.close(target) {
                        Some(new_first) => Some(Node::Split {
                            direction: *direction,
                            ratio: *ratio,
                            first: Box::new(new_first),
                            second: second.clone(),
                        }),
                        // The first child was fully consumed — promote the second child.
                        None => Some(*second.clone()),
                    }
                } else if second.contains(target) {
                    match second.close(target) {
                        Some(new_second) => Some(Node::Split {
                            direction: *direction,
                            ratio: *ratio,
                            first: first.clone(),
                            second: Box::new(new_second),
                        }),
                        None => Some(*first.clone()),
                    }
                } else {
                    Some(self.clone())
                }
            }
        }
    }

    /// Compute pixel rectangles for every leaf pane.
    fn layout(&self, rect: Rect) -> Vec<(PaneId, Rect)> {
        match self {
            Node::Leaf(id) => vec![(*id, rect)],
            Node::Split {
                direction,
                ratio,
                first,
                second,
            } => {
                let (r1, r2) = split_rect(rect, *direction, *ratio);
                let mut v = first.layout(r1);
                v.extend(second.layout(r2));
                v
            }
        }
    }

    /// Adjust the split ratio of the *nearest ancestor split* in the given
    /// `direction` that contains `pane` on one side. `delta` is in pixels and
    /// `rect` is the bounding rectangle of `self`.
    fn resize(&self, pane: PaneId, dir: SplitDirection, delta: f32, rect: Rect) -> Node {
        match self {
            Node::Leaf(_) => self.clone(),
            Node::Split {
                direction,
                ratio,
                first,
                second,
            } => {
                // If this split matches the requested direction AND the target
                // pane is in one of the children, adjust the ratio here.
                if *direction == dir {
                    let in_first = first.contains(pane);
                    let in_second = second.contains(pane);
                    if in_first || in_second {
                        let (r1, r2) = split_rect(rect, *direction, *ratio);
                        // Try to resize deeper first.
                        if in_first && first.pane_count() > 1 {
                            let new_first = first.resize(pane, dir, delta, r1);
                            return Node::Split {
                                direction: *direction,
                                ratio: *ratio,
                                first: Box::new(new_first),
                                second: second.clone(),
                            };
                        }
                        if in_second && second.pane_count() > 1 {
                            let new_second = second.resize(pane, dir, delta, r2);
                            return Node::Split {
                                direction: *direction,
                                ratio: *ratio,
                                first: first.clone(),
                                second: Box::new(new_second),
                            };
                        }

                        // This is the innermost matching split — adjust ratio.
                        let total = match dir {
                            SplitDirection::Horizontal => rect.w,
                            SplitDirection::Vertical => rect.h,
                        };
                        if total <= 0.0 {
                            return self.clone();
                        }
                        // If the pane is in the first child, a positive delta
                        // grows the first child. If in the second child, a
                        // positive delta grows the second (shrinks first).
                        let ratio_delta = delta / total;
                        let new_ratio = if in_first {
                            *ratio + ratio_delta
                        } else {
                            *ratio - ratio_delta
                        };
                        let new_ratio = clamp_ratio(new_ratio, total, MIN_PANE_SIZE);
                        return Node::Split {
                            direction: *direction,
                            ratio: new_ratio,
                            first: first.clone(),
                            second: second.clone(),
                        };
                    }
                }
                // Direction doesn't match or pane isn't here — recurse.
                let (r1, r2) = split_rect(rect, *direction, *ratio);
                if first.contains(pane) {
                    Node::Split {
                        direction: *direction,
                        ratio: *ratio,
                        first: Box::new(first.resize(pane, dir, delta, r1)),
                        second: second.clone(),
                    }
                } else if second.contains(pane) {
                    Node::Split {
                        direction: *direction,
                        ratio: *ratio,
                        first: first.clone(),
                        second: Box::new(second.resize(pane, dir, delta, r2)),
                    }
                } else {
                    self.clone()
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

const MIN_PANE_SIZE: f32 = 50.0;

/// Split a rectangle into two sub-rectangles.
fn split_rect(rect: Rect, dir: SplitDirection, ratio: f32) -> (Rect, Rect) {
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
fn clamp_ratio(ratio: f32, total: f32, min_px: f32) -> f32 {
    if total <= 0.0 {
        return 0.5;
    }
    let min_ratio = min_px / total;
    let max_ratio = 1.0 - min_ratio;
    if min_ratio > max_ratio {
        // Total space is too small to satisfy min for both sides — just center.
        return 0.5;
    }
    ratio.clamp(min_ratio, max_ratio)
}

// ---------------------------------------------------------------------------
// LayoutTree — public API
// ---------------------------------------------------------------------------

/// The immutable split-pane tree.
///
/// Every mutation method returns a *new* `LayoutTree`; the original is not
/// modified.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LayoutTree {
    root: Node,
    next_id: PaneId,
    focused: PaneId,
    zoomed: bool,
}

impl LayoutTree {
    /// Create a tree with a single pane.
    pub fn new(root_pane: PaneId) -> Self {
        Self {
            root: Node::Leaf(root_pane),
            next_id: root_pane + 1,
            focused: root_pane,
            zoomed: false,
        }
    }

    /// Set the next pane ID that will be generated on the next `split`.
    ///
    /// This is used to synchronize pane ID generation across multiple
    /// layout trees (e.g., when using workspaces/tabs).
    pub fn set_next_id(&mut self, id: PaneId) {
        self.next_id = id;
    }

    /// Split `pane` in the given direction. Returns a new tree and the ID of
    /// the newly created pane.
    pub fn split(&self, pane: PaneId, direction: SplitDirection) -> (Self, PaneId) {
        let new_id = self.next_id;
        let (new_root, _found) = self.root.split(pane, direction, new_id);
        (
            Self {
                root: new_root,
                next_id: new_id + 1,
                focused: new_id,
                zoomed: false, // un-zoom on split
            },
            new_id,
        )
    }

    /// Close a pane. Returns `None` if it was the last pane.
    pub fn close(&self, pane: PaneId) -> Option<Self> {
        let new_root = self.root.close(pane)?;
        // If the focused pane was closed, focus the first remaining pane.
        let new_focused = if self.focused == pane {
            *new_root.pane_ids().first().unwrap()
        } else {
            self.focused
        };
        Some(Self {
            root: new_root,
            next_id: self.next_id,
            focused: new_focused,
            zoomed: false, // un-zoom on close
        })
    }

    /// Resize the split boundary near `pane` in the given `direction` by
    /// `delta` pixels (positive = grow the pane in that direction).
    pub fn resize(&self, pane: PaneId, direction: SplitDirection, delta: f32) -> Self {
        // We need a bounding rect for the resize computation. We use a
        // unit rectangle so callers can express delta in absolute pixels
        // relative to whatever total size they consider current. To make
        // this work properly we use a large default so ratio math stays
        // reasonable. Callers should pass pixel deltas.
        //
        // Actually, we don't know the total size here, so we use a
        // normalised rect (1000x1000). The delta should be expressed in the
        // same coordinate system the caller uses for `panes()`.
        let rect = Rect {
            x: 0.0,
            y: 0.0,
            w: 1000.0,
            h: 1000.0,
        };
        let new_root = self.root.resize(pane, direction, delta, rect);
        Self {
            root: new_root,
            next_id: self.next_id,
            focused: self.focused,
            zoomed: self.zoomed,
        }
    }

    /// Get all pane IDs with their pixel rectangles for the given total size.
    ///
    /// When zoomed, only the focused pane is returned, filling the entire area.
    pub fn panes(&self, total_width: f32, total_height: f32) -> Vec<(PaneId, Rect)> {
        if self.zoomed {
            vec![(
                self.focused,
                Rect {
                    x: 0.0,
                    y: 0.0,
                    w: total_width,
                    h: total_height,
                },
            )]
        } else {
            self.root.layout(Rect {
                x: 0.0,
                y: 0.0,
                w: total_width,
                h: total_height,
            })
        }
    }

    /// Currently focused pane.
    pub fn focused(&self) -> PaneId {
        self.focused
    }

    /// Return a new tree with the given pane focused.
    pub fn focus(&self, pane: PaneId) -> Self {
        if !self.root.contains(pane) {
            log::warn!("focus: pane {} does not exist, keeping current focus", pane);
            return self.clone();
        }
        Self {
            root: self.root.clone(),
            next_id: self.next_id,
            focused: pane,
            zoomed: self.zoomed,
        }
    }

    /// Navigate from the currently focused pane in the given direction.
    pub fn navigate(&self, direction: Direction) -> Self {
        let ids = self.root.pane_ids();
        if ids.len() <= 1 {
            return self.clone();
        }

        let new_focus = match direction {
            Direction::Next | Direction::Prev => {
                let idx = ids.iter().position(|id| *id == self.focused).unwrap_or(0);
                match direction {
                    Direction::Next => ids[(idx + 1) % ids.len()],
                    Direction::Prev => ids[(idx + ids.len() - 1) % ids.len()],
                    _ => unreachable!(),
                }
            }
            dir => {
                // Geometric navigation: compute rects, find the closest pane
                // whose center is in the requested direction.
                let rects = self.root.layout(Rect {
                    x: 0.0,
                    y: 0.0,
                    w: 1000.0,
                    h: 1000.0,
                });
                let current_rect = rects
                    .iter()
                    .find(|(id, _)| *id == self.focused)
                    .map(|(_, r)| *r);
                let Some(cur) = current_rect else {
                    return self.clone();
                };
                let (cx, cy) = cur.center();

                let mut best: Option<(PaneId, f32)> = None;
                for (id, r) in &rects {
                    if *id == self.focused {
                        continue;
                    }
                    let (px, py) = r.center();
                    let in_direction = match dir {
                        Direction::Up => py < cy,
                        Direction::Down => py > cy,
                        Direction::Left => px < cx,
                        Direction::Right => px > cx,
                        _ => false,
                    };
                    if !in_direction {
                        continue;
                    }
                    let dist = (px - cx).powi(2) + (py - cy).powi(2);
                    if best.map_or(true, |(_, d)| dist < d) {
                        best = Some((*id, dist));
                    }
                }
                match best {
                    Some((id, _)) => id,
                    None => self.focused, // no pane in that direction
                }
            }
        };

        Self {
            root: self.root.clone(),
            next_id: self.next_id,
            focused: new_focus,
            zoomed: self.zoomed,
        }
    }

    /// Toggle zoom on the focused pane.
    pub fn toggle_zoom(&self) -> Self {
        // Only allow zoom when there are at least 2 panes.
        if self.root.pane_count() <= 1 {
            return self.clone();
        }
        Self {
            root: self.root.clone(),
            next_id: self.next_id,
            focused: self.focused,
            zoomed: !self.zoomed,
        }
    }

    /// Whether the focused pane is currently zoomed.
    pub fn is_zoomed(&self) -> bool {
        self.zoomed
    }

    /// Total number of panes.
    pub fn pane_count(&self) -> usize {
        self.root.pane_count()
    }

    /// Whether the tree contains a pane with the given ID.
    pub fn contains(&self, pane: PaneId) -> bool {
        self.root.contains(pane)
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // -- Construction -------------------------------------------------------

    #[test]
    fn new_tree_has_one_pane() {
        let tree = LayoutTree::new(0);
        assert_eq!(tree.pane_count(), 1);
        assert_eq!(tree.focused(), 0);
        assert!(tree.contains(0));
        assert!(!tree.contains(1));
    }

    // -- Split --------------------------------------------------------------

    #[test]
    fn split_horizontal() {
        let tree = LayoutTree::new(0);
        let (tree, new_id) = tree.split(0, SplitDirection::Horizontal);
        assert_eq!(tree.pane_count(), 2);
        assert_eq!(new_id, 1);
        assert!(tree.contains(0));
        assert!(tree.contains(new_id));
        // Focus moves to the new pane.
        assert_eq!(tree.focused(), new_id);
    }

    #[test]
    fn split_vertical() {
        let tree = LayoutTree::new(0);
        let (tree, new_id) = tree.split(0, SplitDirection::Vertical);
        assert_eq!(tree.pane_count(), 2);
        assert!(tree.contains(0));
        assert!(tree.contains(new_id));
    }

    #[test]
    fn split_produces_equal_rects_horizontal() {
        let tree = LayoutTree::new(0);
        let (tree, _) = tree.split(0, SplitDirection::Horizontal);
        let rects = tree.panes(800.0, 600.0);
        assert_eq!(rects.len(), 2);
        let (_, r0) = rects.iter().find(|(id, _)| *id == 0).unwrap();
        let (_, r1) = rects.iter().find(|(id, _)| *id == 1).unwrap();
        assert!((r0.w - 400.0).abs() < 0.01);
        assert!((r1.w - 400.0).abs() < 0.01);
        assert!((r0.h - 600.0).abs() < 0.01);
        assert!((r1.h - 600.0).abs() < 0.01);
        assert!((r0.x - 0.0).abs() < 0.01);
        assert!((r1.x - 400.0).abs() < 0.01);
    }

    #[test]
    fn split_produces_equal_rects_vertical() {
        let tree = LayoutTree::new(0);
        let (tree, _) = tree.split(0, SplitDirection::Vertical);
        let rects = tree.panes(800.0, 600.0);
        assert_eq!(rects.len(), 2);
        let (_, r0) = rects.iter().find(|(id, _)| *id == 0).unwrap();
        let (_, r1) = rects.iter().find(|(id, _)| *id == 1).unwrap();
        assert!((r0.h - 300.0).abs() < 0.01);
        assert!((r1.h - 300.0).abs() < 0.01);
        assert!((r0.y - 0.0).abs() < 0.01);
        assert!((r1.y - 300.0).abs() < 0.01);
    }

    #[test]
    fn multiple_splits() {
        let tree = LayoutTree::new(0);
        let (tree, p1) = tree.split(0, SplitDirection::Horizontal);
        let (tree, p2) = tree.split(p1, SplitDirection::Vertical);
        assert_eq!(tree.pane_count(), 3);
        assert!(tree.contains(0));
        assert!(tree.contains(p1));
        assert!(tree.contains(p2));
    }

    #[test]
    fn split_nonexistent_pane_is_noop() {
        let tree = LayoutTree::new(0);
        let (tree2, new_id) = tree.split(999, SplitDirection::Horizontal);
        // The tree should still only have 1 pane (the split target was not found).
        assert_eq!(tree2.pane_count(), 1);
        assert_eq!(new_id, 1); // ID counter still increments
    }

    // -- Close --------------------------------------------------------------

    #[test]
    fn close_last_pane_returns_none() {
        let tree = LayoutTree::new(0);
        assert!(tree.close(0).is_none());
    }

    #[test]
    fn close_one_of_two() {
        let tree = LayoutTree::new(0);
        let (tree, p1) = tree.split(0, SplitDirection::Horizontal);
        let tree = tree.close(p1).unwrap();
        assert_eq!(tree.pane_count(), 1);
        assert!(tree.contains(0));
        assert!(!tree.contains(p1));
    }

    #[test]
    fn close_refocuses_when_focused_pane_closed() {
        let tree = LayoutTree::new(0);
        let (tree, p1) = tree.split(0, SplitDirection::Horizontal);
        // p1 is now focused
        assert_eq!(tree.focused(), p1);
        let tree = tree.close(p1).unwrap();
        // Focus should move to remaining pane.
        assert_eq!(tree.focused(), 0);
    }

    #[test]
    fn close_preserves_focus_when_other_pane_closed() {
        let tree = LayoutTree::new(0);
        let (tree, p1) = tree.split(0, SplitDirection::Horizontal);
        let tree = tree.focus(0);
        let tree = tree.close(p1).unwrap();
        assert_eq!(tree.focused(), 0);
    }

    #[test]
    fn close_nonexistent_pane_returns_unchanged() {
        let tree = LayoutTree::new(0);
        let (tree, _) = tree.split(0, SplitDirection::Horizontal);
        let tree2 = tree.close(999);
        assert!(tree2.is_some());
        assert_eq!(tree2.unwrap().pane_count(), 2);
    }

    // -- Panes / layout -----------------------------------------------------

    #[test]
    fn single_pane_fills_area() {
        let tree = LayoutTree::new(0);
        let rects = tree.panes(1920.0, 1080.0);
        assert_eq!(rects.len(), 1);
        let (id, r) = &rects[0];
        assert_eq!(*id, 0);
        assert!((r.x).abs() < 0.01);
        assert!((r.y).abs() < 0.01);
        assert!((r.w - 1920.0).abs() < 0.01);
        assert!((r.h - 1080.0).abs() < 0.01);
    }

    #[test]
    fn four_pane_grid() {
        let tree = LayoutTree::new(0);
        let (tree, p1) = tree.split(0, SplitDirection::Horizontal);
        let (tree, _p2) = tree.split(0, SplitDirection::Vertical);
        let (tree, _p3) = tree.split(p1, SplitDirection::Vertical);
        assert_eq!(tree.pane_count(), 4);
        let rects = tree.panes(800.0, 600.0);
        assert_eq!(rects.len(), 4);
        // All rects should sum to total area.
        let total_area: f32 = rects.iter().map(|(_, r)| r.w * r.h).sum();
        assert!((total_area - 800.0 * 600.0).abs() < 1.0);
    }

    // -- Focus --------------------------------------------------------------

    #[test]
    fn focus_changes() {
        let tree = LayoutTree::new(0);
        let (tree, p1) = tree.split(0, SplitDirection::Horizontal);
        assert_eq!(tree.focused(), p1);
        let tree = tree.focus(0);
        assert_eq!(tree.focused(), 0);
    }

    #[test]
    fn focus_nonexistent_pane_is_noop() {
        let tree = LayoutTree::new(0);
        let tree2 = tree.focus(999);
        assert_eq!(tree2.focused(), 0);
    }

    // -- Navigation ---------------------------------------------------------

    #[test]
    fn navigate_next_prev_wraps() {
        let tree = LayoutTree::new(0);
        let (tree, p1) = tree.split(0, SplitDirection::Horizontal);
        let (tree, p2) = tree.split(p1, SplitDirection::Horizontal);

        // Focus is on p2. Next should wrap to 0.
        let tree = tree.navigate(Direction::Next);
        assert_eq!(tree.focused(), 0);

        // Prev from 0 should wrap to p2.
        let tree = tree.navigate(Direction::Prev);
        assert_eq!(tree.focused(), p2);
    }

    #[test]
    fn navigate_spatial_left_right() {
        let tree = LayoutTree::new(0);
        let (tree, p1) = tree.split(0, SplitDirection::Horizontal);
        // p1 is right of 0
        let tree = tree.focus(0);
        let tree = tree.navigate(Direction::Right);
        assert_eq!(tree.focused(), p1);
        let tree = tree.navigate(Direction::Left);
        assert_eq!(tree.focused(), 0);
    }

    #[test]
    fn navigate_spatial_up_down() {
        let tree = LayoutTree::new(0);
        let (tree, p1) = tree.split(0, SplitDirection::Vertical);
        // p1 is below 0
        let tree = tree.focus(0);
        let tree = tree.navigate(Direction::Down);
        assert_eq!(tree.focused(), p1);
        let tree = tree.navigate(Direction::Up);
        assert_eq!(tree.focused(), 0);
    }

    #[test]
    fn navigate_no_pane_in_direction_stays() {
        let tree = LayoutTree::new(0);
        let (tree, _) = tree.split(0, SplitDirection::Horizontal);
        let tree = tree.focus(0);
        // No pane to the left of pane 0.
        let tree = tree.navigate(Direction::Left);
        assert_eq!(tree.focused(), 0);
    }

    #[test]
    fn navigate_single_pane_is_noop() {
        let tree = LayoutTree::new(0);
        let tree = tree.navigate(Direction::Right);
        assert_eq!(tree.focused(), 0);
    }

    // -- Resize -------------------------------------------------------------

    #[test]
    fn resize_horizontal_grows_pane() {
        let tree = LayoutTree::new(0);
        let (tree, p1) = tree.split(0, SplitDirection::Horizontal);
        // Grow pane 0 to the right by 100px (in a 1000px-wide space).
        let tree2 = tree.resize(0, SplitDirection::Horizontal, 100.0);
        let rects_before = tree.panes(1000.0, 1000.0);
        let rects_after = tree2.panes(1000.0, 1000.0);
        let w0_before = rects_before.iter().find(|(id, _)| *id == 0).unwrap().1.w;
        let w0_after = rects_after.iter().find(|(id, _)| *id == 0).unwrap().1.w;
        let w1_after = rects_after.iter().find(|(id, _)| *id == p1).unwrap().1.w;
        assert!(w0_after > w0_before);
        assert!((w0_after + w1_after - 1000.0).abs() < 1.0);
    }

    #[test]
    fn resize_respects_minimum() {
        let tree = LayoutTree::new(0);
        let (tree, _) = tree.split(0, SplitDirection::Horizontal);
        // Try to grow pane 0 by a huge amount.
        let tree2 = tree.resize(0, SplitDirection::Horizontal, 99999.0);
        let rects = tree2.panes(1000.0, 1000.0);
        let w1 = rects.iter().find(|(id, _)| *id == 1).unwrap().1.w;
        // The second pane should still be at least MIN_PANE_SIZE.
        assert!(w1 >= MIN_PANE_SIZE - 0.01);
    }

    #[test]
    fn resize_negative_shrinks_pane() {
        let tree = LayoutTree::new(0);
        let (tree, _) = tree.split(0, SplitDirection::Horizontal);
        let tree2 = tree.resize(0, SplitDirection::Horizontal, -100.0);
        let rects_before = tree.panes(1000.0, 1000.0);
        let rects_after = tree2.panes(1000.0, 1000.0);
        let w0_before = rects_before.iter().find(|(id, _)| *id == 0).unwrap().1.w;
        let w0_after = rects_after.iter().find(|(id, _)| *id == 0).unwrap().1.w;
        assert!(w0_after < w0_before);
    }

    // -- Zoom ---------------------------------------------------------------

    #[test]
    fn zoom_toggle() {
        let tree = LayoutTree::new(0);
        let (tree, _) = tree.split(0, SplitDirection::Horizontal);
        let tree = tree.focus(0);
        assert!(!tree.is_zoomed());

        let tree = tree.toggle_zoom();
        assert!(tree.is_zoomed());

        // When zoomed, panes() returns only the focused pane.
        let rects = tree.panes(800.0, 600.0);
        assert_eq!(rects.len(), 1);
        assert_eq!(rects[0].0, 0);
        assert!((rects[0].1.w - 800.0).abs() < 0.01);

        let tree = tree.toggle_zoom();
        assert!(!tree.is_zoomed());
        let rects = tree.panes(800.0, 600.0);
        assert_eq!(rects.len(), 2);
    }

    #[test]
    fn zoom_single_pane_noop() {
        let tree = LayoutTree::new(0);
        let tree = tree.toggle_zoom();
        assert!(!tree.is_zoomed());
    }

    #[test]
    fn split_unzooms() {
        let tree = LayoutTree::new(0);
        let (tree, _) = tree.split(0, SplitDirection::Horizontal);
        let tree = tree.toggle_zoom();
        assert!(tree.is_zoomed());
        let (tree, _) = tree.split(0, SplitDirection::Vertical);
        assert!(!tree.is_zoomed());
    }

    #[test]
    fn close_unzooms() {
        let tree = LayoutTree::new(0);
        let (tree, p1) = tree.split(0, SplitDirection::Horizontal);
        let (tree, _p2) = tree.split(p1, SplitDirection::Vertical);
        let tree = tree.focus(0).toggle_zoom();
        assert!(tree.is_zoomed());
        let tree = tree.close(p1).unwrap();
        assert!(!tree.is_zoomed());
    }

    // -- Immutability -------------------------------------------------------

    #[test]
    fn original_tree_unchanged_after_split() {
        let tree = LayoutTree::new(0);
        let (tree2, _) = tree.split(0, SplitDirection::Horizontal);
        assert_eq!(tree.pane_count(), 1);
        assert_eq!(tree2.pane_count(), 2);
    }

    #[test]
    fn original_tree_unchanged_after_close() {
        let tree = LayoutTree::new(0);
        let (tree, p1) = tree.split(0, SplitDirection::Horizontal);
        let tree2 = tree.close(p1).unwrap();
        assert_eq!(tree.pane_count(), 2);
        assert_eq!(tree2.pane_count(), 1);
    }

    // -- Contains -----------------------------------------------------------

    #[test]
    fn contains_works() {
        let tree = LayoutTree::new(0);
        assert!(tree.contains(0));
        assert!(!tree.contains(1));
        let (tree, p1) = tree.split(0, SplitDirection::Horizontal);
        assert!(tree.contains(0));
        assert!(tree.contains(p1));
        assert!(!tree.contains(999));
    }

    // -- Pane count ---------------------------------------------------------

    #[test]
    fn pane_count_tracks_splits_and_closes() {
        let tree = LayoutTree::new(0);
        assert_eq!(tree.pane_count(), 1);
        let (tree, p1) = tree.split(0, SplitDirection::Horizontal);
        assert_eq!(tree.pane_count(), 2);
        let (tree, _) = tree.split(p1, SplitDirection::Vertical);
        assert_eq!(tree.pane_count(), 3);
        let tree = tree.close(p1).unwrap();
        assert_eq!(tree.pane_count(), 2);
    }

    // -- Serde round-trip ---------------------------------------------------

    #[test]
    fn serde_roundtrip() {
        let tree = LayoutTree::new(0);
        let (tree, _) = tree.split(0, SplitDirection::Horizontal);
        let json = serde_json::to_string(&tree).unwrap();
        let tree2: LayoutTree = serde_json::from_str(&json).unwrap();
        assert_eq!(tree2.pane_count(), 2);
        assert_eq!(tree2.focused(), tree.focused());
    }

    // -- Complex scenario ---------------------------------------------------

    #[test]
    fn complex_workflow() {
        // Simulate a realistic workflow:
        // 1. Start with one pane.
        let tree = LayoutTree::new(0);
        // 2. Split horizontally -> [0 | 1]
        let (tree, p1) = tree.split(0, SplitDirection::Horizontal);
        assert_eq!(p1, 1);
        // 3. Split pane 1 vertically -> [0 | 1/2]
        let (tree, p2) = tree.split(p1, SplitDirection::Vertical);
        assert_eq!(p2, 2);
        assert_eq!(tree.pane_count(), 3);
        // 4. Focus pane 0, navigate right -> should land on 1 or 2
        let tree = tree.focus(0);
        let tree = tree.navigate(Direction::Right);
        assert!(tree.focused() == p1 || tree.focused() == p2);
        // 5. Close pane 2
        let tree = tree.close(p2).unwrap();
        assert_eq!(tree.pane_count(), 2);
        assert!(!tree.contains(p2));
        // 6. Zoom pane 1
        let tree = tree.focus(p1);
        let tree = tree.toggle_zoom();
        assert!(tree.is_zoomed());
        let rects = tree.panes(1000.0, 1000.0);
        assert_eq!(rects.len(), 1);
        assert_eq!(rects[0].0, p1);
        // 7. Unzoom
        let tree = tree.toggle_zoom();
        assert!(!tree.is_zoomed());
        let rects = tree.panes(1000.0, 1000.0);
        assert_eq!(rects.len(), 2);
    }

    // -- Resize edge cases --------------------------------------------------

    #[test]
    fn resize_vertical() {
        let tree = LayoutTree::new(0);
        let (tree, p1) = tree.split(0, SplitDirection::Vertical);
        let tree2 = tree.resize(0, SplitDirection::Vertical, 100.0);
        let rects = tree2.panes(1000.0, 1000.0);
        let h0 = rects.iter().find(|(id, _)| *id == 0).unwrap().1.h;
        let h1 = rects.iter().find(|(id, _)| *id == p1).unwrap().1.h;
        assert!(h0 > 500.0);
        assert!(h1 < 500.0);
        assert!((h0 + h1 - 1000.0).abs() < 1.0);
    }

    #[test]
    fn resize_wrong_direction_is_noop() {
        let tree = LayoutTree::new(0);
        let (tree, _) = tree.split(0, SplitDirection::Horizontal);
        // Resize in vertical direction when split is horizontal — nothing to do.
        let tree2 = tree.resize(0, SplitDirection::Vertical, 100.0);
        let rects1 = tree.panes(1000.0, 1000.0);
        let rects2 = tree2.panes(1000.0, 1000.0);
        let w1 = rects1.iter().find(|(id, _)| *id == 0).unwrap().1.w;
        let w2 = rects2.iter().find(|(id, _)| *id == 0).unwrap().1.w;
        assert!((w1 - w2).abs() < 0.01);
    }

    // -- Navigation in a grid -----------------------------------------------

    #[test]
    fn navigate_grid() {
        // Create a 2x2 grid:
        //  0 | 1
        //  -----
        //  2 | 3
        let tree = LayoutTree::new(0);
        let (tree, p1) = tree.split(0, SplitDirection::Horizontal);   // [0 | 1]
        let (tree, p2) = tree.split(0, SplitDirection::Vertical);     // [0/2 | 1]
        let (tree, p3) = tree.split(p1, SplitDirection::Vertical);    // [0/2 | 1/3]

        // Focus top-left (0), go right -> top-right (1)
        let tree = tree.focus(0);
        let t = tree.navigate(Direction::Right);
        assert_eq!(t.focused(), p1);

        // Focus top-left (0), go down -> bottom-left (2)
        let t = tree.navigate(Direction::Down);
        assert_eq!(t.focused(), p2);

        // Focus bottom-right (3), go up -> top-right (1)
        let tree = tree.focus(p3);
        let t = tree.navigate(Direction::Up);
        assert_eq!(t.focused(), p1);

        // Focus bottom-right (3), go left -> bottom-left (2)
        let t = tree.navigate(Direction::Left);
        assert_eq!(t.focused(), p2);
    }

    // -- Rect center --------------------------------------------------------

    #[test]
    fn rect_center() {
        let r = Rect {
            x: 10.0,
            y: 20.0,
            w: 100.0,
            h: 50.0,
        };
        let (cx, cy) = r.center();
        assert!((cx - 60.0).abs() < 0.01);
        assert!((cy - 45.0).abs() < 0.01);
    }

    // -- clamp_ratio --------------------------------------------------------

    #[test]
    fn clamp_ratio_basic() {
        // Normal case: 50/50 in 1000px -> min ratio = 0.05
        assert!((clamp_ratio(0.5, 1000.0, 50.0) - 0.5).abs() < 0.01);
        // Trying to go below min
        assert!((clamp_ratio(0.01, 1000.0, 50.0) - 0.05).abs() < 0.01);
        // Trying to go above max
        assert!((clamp_ratio(0.99, 1000.0, 50.0) - 0.95).abs() < 0.01);
    }

    #[test]
    fn clamp_ratio_tiny_total() {
        // Total smaller than 2 * min -> returns 0.5
        assert!((clamp_ratio(0.8, 80.0, 50.0) - 0.5).abs() < 0.01);
    }
}
