//! LayoutTree — the public API for the immutable split-pane tree.

use serde::{Deserialize, Serialize};

use crate::node::Node;
use crate::types::{Direction, PaneId, Rect, SplitDirection};

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
                zoomed: false,
            },
            new_id,
        )
    }

    /// Insert an existing pane next to `target` in the given direction.
    pub fn split_insert(
        &self,
        target: PaneId,
        direction: SplitDirection,
        insert_id: PaneId,
        insert_first: bool,
    ) -> Self {
        let (new_root, _found) = self
            .root
            .split_insert(target, direction, insert_id, insert_first);
        Self {
            root: new_root,
            next_id: self.next_id,
            focused: insert_id,
            zoomed: false,
        }
    }

    /// Close a pane. Returns `None` if it was the last pane.
    pub fn close(&self, pane: PaneId) -> Option<Self> {
        let new_root = self.root.close(pane)?;
        let new_focused = if self.focused == pane {
            *new_root.pane_ids().first().expect("at least one pane remains")
        } else {
            self.focused
        };
        Some(Self {
            root: new_root,
            next_id: self.next_id,
            focused: new_focused,
            zoomed: false,
        })
    }

    /// Resize the split boundary near `pane` in the given `direction` by
    /// `delta` pixels.
    pub fn resize(&self, pane: PaneId, direction: SplitDirection, delta: f32) -> Self {
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
                    None => self.focused,
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

    /// All pane IDs in tree order.
    pub fn pane_ids(&self) -> Vec<PaneId> {
        self.root.pane_ids()
    }

    /// Extract a pane from the tree.
    pub fn extract_pane(&self, pane: PaneId) -> Option<(Self, Self)> {
        if self.root.pane_count() <= 1 {
            return None;
        }
        if !self.root.contains(pane) {
            return None;
        }

        let remaining_root = self.root.close(pane)?;
        let new_focused = if self.focused == pane {
            *remaining_root
                .pane_ids()
                .first()
                .expect("at least one pane remains")
        } else {
            self.focused
        };
        let remaining = Self {
            root: remaining_root,
            next_id: self.next_id,
            focused: new_focused,
            zoomed: false,
        };

        let extracted = Self {
            root: Node::Leaf(pane),
            next_id: self.next_id,
            focused: pane,
            zoomed: false,
        };

        Some((remaining, extracted))
    }
}
