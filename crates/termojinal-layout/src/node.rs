//! Internal tree node (immutable, cheaply cloneable via Box).

use serde::{Deserialize, Serialize};

use crate::types::{PaneId, Rect, SplitDirection};
use crate::{clamp_ratio, split_rect, MIN_PANE_SIZE};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) enum Node {
    Leaf(PaneId),
    Split {
        direction: SplitDirection,
        /// Fraction of space given to the *first* (left / top) child (0.0-1.0).
        ratio: f32,
        first: Box<Node>,
        second: Box<Node>,
    },
}

impl Node {
    /// Collect all pane IDs in tree order (left-to-right / top-to-bottom).
    pub(crate) fn pane_ids(&self) -> Vec<PaneId> {
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
    pub(crate) fn pane_count(&self) -> usize {
        match self {
            Node::Leaf(_) => 1,
            Node::Split { first, second, .. } => first.pane_count() + second.pane_count(),
        }
    }

    /// Does this subtree contain the given pane?
    pub(crate) fn contains(&self, pane: PaneId) -> bool {
        match self {
            Node::Leaf(id) => *id == pane,
            Node::Split { first, second, .. } => first.contains(pane) || second.contains(pane),
        }
    }

    /// Split a leaf pane, returning the new subtree and the newly created PaneId.
    pub(crate) fn split(&self, target: PaneId, dir: SplitDirection, new_id: PaneId) -> (Node, bool) {
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

    /// Insert an existing pane next to a target pane.
    pub(crate) fn split_insert(
        &self,
        target: PaneId,
        dir: SplitDirection,
        insert_id: PaneId,
        insert_first: bool,
    ) -> (Node, bool) {
        match self {
            Node::Leaf(id) if *id == target => {
                let (first, second) = if insert_first {
                    (Box::new(Node::Leaf(insert_id)), Box::new(Node::Leaf(*id)))
                } else {
                    (Box::new(Node::Leaf(*id)), Box::new(Node::Leaf(insert_id)))
                };
                let new_node = Node::Split {
                    direction: dir,
                    ratio: 0.5,
                    first,
                    second,
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
                let (new_first, found) = first.split_insert(target, dir, insert_id, insert_first);
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
                let (new_second, found) = second.split_insert(target, dir, insert_id, insert_first);
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

    /// Close a pane. Returns `None` if the pane was the only leaf at root level.
    pub(crate) fn close(&self, target: PaneId) -> Option<Node> {
        match self {
            Node::Leaf(id) => {
                if *id == target {
                    None
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
    pub(crate) fn layout(&self, rect: Rect) -> Vec<(PaneId, Rect)> {
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
    /// `direction` that contains `pane` on one side.
    pub(crate) fn resize(&self, pane: PaneId, dir: SplitDirection, delta: f32, rect: Rect) -> Node {
        match self {
            Node::Leaf(_) => self.clone(),
            Node::Split {
                direction,
                ratio,
                first,
                second,
            } => {
                if *direction == dir {
                    let in_first = first.contains(pane);
                    let in_second = second.contains(pane);
                    if in_first || in_second {
                        let (r1, r2) = split_rect(rect, *direction, *ratio);
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

                        let total = match dir {
                            SplitDirection::Horizontal => rect.w,
                            SplitDirection::Vertical => rect.h,
                        };
                        if total <= 0.0 {
                            return self.clone();
                        }
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
