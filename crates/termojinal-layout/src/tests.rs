#[cfg(test)]
mod tests {
    use crate::*;

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
        let (_, r0) = rects.iter().find(|(id, _)| *id == 0).expect("pane 0");
        let (_, r1) = rects.iter().find(|(id, _)| *id == 1).expect("pane 1");
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
        let (_, r0) = rects.iter().find(|(id, _)| *id == 0).expect("pane 0");
        let (_, r1) = rects.iter().find(|(id, _)| *id == 1).expect("pane 1");
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
        assert_eq!(tree2.pane_count(), 1);
        assert_eq!(new_id, 1);
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
        let tree = tree.close(p1).expect("should close");
        assert_eq!(tree.pane_count(), 1);
        assert!(tree.contains(0));
        assert!(!tree.contains(p1));
    }

    #[test]
    fn close_refocuses_when_focused_pane_closed() {
        let tree = LayoutTree::new(0);
        let (tree, p1) = tree.split(0, SplitDirection::Horizontal);
        assert_eq!(tree.focused(), p1);
        let tree = tree.close(p1).expect("should close");
        assert_eq!(tree.focused(), 0);
    }

    #[test]
    fn close_preserves_focus_when_other_pane_closed() {
        let tree = LayoutTree::new(0);
        let (tree, p1) = tree.split(0, SplitDirection::Horizontal);
        let tree = tree.focus(0);
        let tree = tree.close(p1).expect("should close");
        assert_eq!(tree.focused(), 0);
    }

    #[test]
    fn close_nonexistent_pane_returns_unchanged() {
        let tree = LayoutTree::new(0);
        let (tree, _) = tree.split(0, SplitDirection::Horizontal);
        let tree2 = tree.close(999);
        assert!(tree2.is_some());
        assert_eq!(tree2.expect("exists").pane_count(), 2);
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
        let total_area: f32 = rects.iter().map(|(_, r)| r.w * r.h).sum();
        assert!((total_area - 800.0 * 600.0).abs() < 1.0);
    }

    // -- Focus / Navigation / Resize / Zoom tests ---------------------------

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

    #[test]
    fn navigate_next_prev_wraps() {
        let tree = LayoutTree::new(0);
        let (tree, p1) = tree.split(0, SplitDirection::Horizontal);
        let (tree, p2) = tree.split(p1, SplitDirection::Horizontal);
        let tree = tree.navigate(Direction::Next);
        assert_eq!(tree.focused(), 0);
        let tree = tree.navigate(Direction::Prev);
        assert_eq!(tree.focused(), p2);
    }

    #[test]
    fn navigate_spatial_left_right() {
        let tree = LayoutTree::new(0);
        let (tree, p1) = tree.split(0, SplitDirection::Horizontal);
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
        let tree = tree.navigate(Direction::Left);
        assert_eq!(tree.focused(), 0);
    }

    #[test]
    fn navigate_single_pane_is_noop() {
        let tree = LayoutTree::new(0);
        let tree = tree.navigate(Direction::Right);
        assert_eq!(tree.focused(), 0);
    }

    #[test]
    fn resize_horizontal_grows_pane() {
        let tree = LayoutTree::new(0);
        let (tree, p1) = tree.split(0, SplitDirection::Horizontal);
        let tree2 = tree.resize(0, SplitDirection::Horizontal, 100.0);
        let rects_before = tree.panes(1000.0, 1000.0);
        let rects_after = tree2.panes(1000.0, 1000.0);
        let w0_before = rects_before.iter().find(|(id, _)| *id == 0).expect("pane 0").1.w;
        let w0_after = rects_after.iter().find(|(id, _)| *id == 0).expect("pane 0").1.w;
        let w1_after = rects_after.iter().find(|(id, _)| *id == p1).expect("pane 1").1.w;
        assert!(w0_after > w0_before);
        assert!((w0_after + w1_after - 1000.0).abs() < 1.0);
    }

    #[test]
    fn resize_respects_minimum() {
        let tree = LayoutTree::new(0);
        let (tree, _) = tree.split(0, SplitDirection::Horizontal);
        let tree2 = tree.resize(0, SplitDirection::Horizontal, 99999.0);
        let rects = tree2.panes(1000.0, 1000.0);
        let w1 = rects.iter().find(|(id, _)| *id == 1).expect("pane 1").1.w;
        assert!(w1 >= MIN_PANE_SIZE - 0.01);
    }

    #[test]
    fn resize_negative_shrinks_pane() {
        let tree = LayoutTree::new(0);
        let (tree, _) = tree.split(0, SplitDirection::Horizontal);
        let tree2 = tree.resize(0, SplitDirection::Horizontal, -100.0);
        let rects_before = tree.panes(1000.0, 1000.0);
        let rects_after = tree2.panes(1000.0, 1000.0);
        let w0_before = rects_before.iter().find(|(id, _)| *id == 0).expect("pane 0").1.w;
        let w0_after = rects_after.iter().find(|(id, _)| *id == 0).expect("pane 0").1.w;
        assert!(w0_after < w0_before);
    }

    #[test]
    fn zoom_toggle() {
        let tree = LayoutTree::new(0);
        let (tree, _) = tree.split(0, SplitDirection::Horizontal);
        let tree = tree.focus(0);
        assert!(!tree.is_zoomed());
        let tree = tree.toggle_zoom();
        assert!(tree.is_zoomed());
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
        let tree = tree.close(p1).expect("should close");
        assert!(!tree.is_zoomed());
    }

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
        let tree2 = tree.close(p1).expect("should close");
        assert_eq!(tree.pane_count(), 2);
        assert_eq!(tree2.pane_count(), 1);
    }

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

    #[test]
    fn pane_count_tracks_splits_and_closes() {
        let tree = LayoutTree::new(0);
        assert_eq!(tree.pane_count(), 1);
        let (tree, p1) = tree.split(0, SplitDirection::Horizontal);
        assert_eq!(tree.pane_count(), 2);
        let (tree, _) = tree.split(p1, SplitDirection::Vertical);
        assert_eq!(tree.pane_count(), 3);
        let tree = tree.close(p1).expect("should close");
        assert_eq!(tree.pane_count(), 2);
    }

    #[test]
    fn serde_roundtrip() {
        let tree = LayoutTree::new(0);
        let (tree, _) = tree.split(0, SplitDirection::Horizontal);
        let json = serde_json::to_string(&tree).expect("serialize");
        let tree2: LayoutTree = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(tree2.pane_count(), 2);
        assert_eq!(tree2.focused(), tree.focused());
    }

    #[test]
    fn complex_workflow() {
        let tree = LayoutTree::new(0);
        let (tree, p1) = tree.split(0, SplitDirection::Horizontal);
        assert_eq!(p1, 1);
        let (tree, p2) = tree.split(p1, SplitDirection::Vertical);
        assert_eq!(p2, 2);
        assert_eq!(tree.pane_count(), 3);
        let tree = tree.focus(0);
        let tree = tree.navigate(Direction::Right);
        assert!(tree.focused() == p1 || tree.focused() == p2);
        let tree = tree.close(p2).expect("should close");
        assert_eq!(tree.pane_count(), 2);
        assert!(!tree.contains(p2));
        let tree = tree.focus(p1);
        let tree = tree.toggle_zoom();
        assert!(tree.is_zoomed());
        let rects = tree.panes(1000.0, 1000.0);
        assert_eq!(rects.len(), 1);
        assert_eq!(rects[0].0, p1);
        let tree = tree.toggle_zoom();
        assert!(!tree.is_zoomed());
        let rects = tree.panes(1000.0, 1000.0);
        assert_eq!(rects.len(), 2);
    }

    #[test]
    fn resize_vertical() {
        let tree = LayoutTree::new(0);
        let (tree, p1) = tree.split(0, SplitDirection::Vertical);
        let tree2 = tree.resize(0, SplitDirection::Vertical, 100.0);
        let rects = tree2.panes(1000.0, 1000.0);
        let h0 = rects.iter().find(|(id, _)| *id == 0).expect("pane 0").1.h;
        let h1 = rects.iter().find(|(id, _)| *id == p1).expect("pane 1").1.h;
        assert!(h0 > 500.0);
        assert!(h1 < 500.0);
        assert!((h0 + h1 - 1000.0).abs() < 1.0);
    }

    #[test]
    fn resize_wrong_direction_is_noop() {
        let tree = LayoutTree::new(0);
        let (tree, _) = tree.split(0, SplitDirection::Horizontal);
        let tree2 = tree.resize(0, SplitDirection::Vertical, 100.0);
        let rects1 = tree.panes(1000.0, 1000.0);
        let rects2 = tree2.panes(1000.0, 1000.0);
        let w1 = rects1.iter().find(|(id, _)| *id == 0).expect("pane 0").1.w;
        let w2 = rects2.iter().find(|(id, _)| *id == 0).expect("pane 0").1.w;
        assert!((w1 - w2).abs() < 0.01);
    }

    #[test]
    fn navigate_grid() {
        let tree = LayoutTree::new(0);
        let (tree, p1) = tree.split(0, SplitDirection::Horizontal);
        let (tree, p2) = tree.split(0, SplitDirection::Vertical);
        let (tree, p3) = tree.split(p1, SplitDirection::Vertical);

        let tree = tree.focus(0);
        let t = tree.navigate(Direction::Right);
        assert_eq!(t.focused(), p1);
        let t = tree.navigate(Direction::Down);
        assert_eq!(t.focused(), p2);
        let tree = tree.focus(p3);
        let t = tree.navigate(Direction::Up);
        assert_eq!(t.focused(), p1);
        let t = tree.navigate(Direction::Left);
        assert_eq!(t.focused(), p2);
    }

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

    #[test]
    fn clamp_ratio_basic() {
        assert!((clamp_ratio(0.5, 1000.0, 50.0) - 0.5).abs() < 0.01);
        assert!((clamp_ratio(0.01, 1000.0, 50.0) - 0.05).abs() < 0.01);
        assert!((clamp_ratio(0.99, 1000.0, 50.0) - 0.95).abs() < 0.01);
    }

    #[test]
    fn clamp_ratio_tiny_total() {
        assert!((clamp_ratio(0.8, 80.0, 50.0) - 0.5).abs() < 0.01);
    }

    #[test]
    fn split_insert_places_pane_first() {
        let tree = LayoutTree::new(0);
        let tree = tree.split_insert(0, SplitDirection::Horizontal, 10, true);
        assert_eq!(tree.pane_count(), 2);
        assert!(tree.contains(0));
        assert!(tree.contains(10));
        assert_eq!(tree.focused(), 10);
        let rects = tree.panes(800.0, 600.0);
        let r10 = rects.iter().find(|(id, _)| *id == 10).expect("pane 10").1;
        let r0 = rects.iter().find(|(id, _)| *id == 0).expect("pane 0").1;
        assert!(r10.x < r0.x, "inserted pane should be to the left");
    }

    #[test]
    fn split_insert_places_pane_second() {
        let tree = LayoutTree::new(0);
        let tree = tree.split_insert(0, SplitDirection::Horizontal, 10, false);
        assert_eq!(tree.pane_count(), 2);
        let rects = tree.panes(800.0, 600.0);
        let r10 = rects.iter().find(|(id, _)| *id == 10).expect("pane 10").1;
        let r0 = rects.iter().find(|(id, _)| *id == 0).expect("pane 0").1;
        assert!(r10.x > r0.x, "inserted pane should be to the right");
    }

    #[test]
    fn split_insert_vertical() {
        let tree = LayoutTree::new(0);
        let tree = tree.split_insert(0, SplitDirection::Vertical, 10, true);
        let rects = tree.panes(800.0, 600.0);
        let r10 = rects.iter().find(|(id, _)| *id == 10).expect("pane 10").1;
        let r0 = rects.iter().find(|(id, _)| *id == 0).expect("pane 0").1;
        assert!(r10.y < r0.y, "inserted pane should be above");
    }

    #[test]
    fn extract_pane_from_two_pane_tree() {
        let tree = LayoutTree::new(0);
        let (tree, pid) = tree.split(0, SplitDirection::Horizontal);
        assert_eq!(tree.pane_count(), 2);
        let (remaining, extracted) = tree.extract_pane(pid).expect("should extract");
        assert_eq!(remaining.pane_count(), 1);
        assert!(remaining.contains(0));
        assert!(!remaining.contains(pid));
        assert_eq!(extracted.pane_count(), 1);
        assert!(extracted.contains(pid));
        assert_eq!(extracted.focused(), pid);
    }

    #[test]
    fn extract_only_pane_returns_none() {
        let tree = LayoutTree::new(0);
        assert!(tree.extract_pane(0).is_none());
    }
}
