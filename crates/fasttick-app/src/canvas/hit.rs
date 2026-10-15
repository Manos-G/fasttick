//! Hit testing and culling on the world rectangles of a layout.
//!
//! The functions scan the rectangles in a straight loop. At 5,000 nodes a
//! scan takes a few microseconds, which is less than any spatial index
//! would save.

use egui::{Pos2, Rect};

use super::model::CanvasNodeId;

/// The node under a world point. `slop` enlarges each rectangle (in world
/// units), so that thin borders are easy to hit. If rectangles overlap,
/// the last one (drawn on top) wins.
pub fn hit_node(rects: &[Rect], world: Pos2, slop: f32) -> Option<CanvasNodeId> {
    rects
        .iter()
        .rposition(|rect| rect.expand(slop).contains(world))
        .map(|index| CanvasNodeId(index as u32))
}

/// The nodes whose rectangles intersect a world rectangle (for rubber-band
/// selection).
pub fn nodes_in_rect(rects: &[Rect], world: Rect) -> Vec<CanvasNodeId> {
    rects
        .iter()
        .enumerate()
        .filter(|(_, rect)| rect.intersects(world))
        .map(|(index, _)| CanvasNodeId(index as u32))
        .collect()
}

/// Writes the indices of the rectangles that intersect `view` to `out`.
/// `out` is cleared first. The caller keeps `out` from frame to frame, so
/// that culling does not allocate.
pub fn visible_into(rects: &[Rect], view: Rect, out: &mut Vec<u32>) {
    out.clear();
    out.extend(
        rects
            .iter()
            .enumerate()
            .filter(|(_, rect)| rect.intersects(view))
            .map(|(index, _)| index as u32),
    );
}

/// A rectangle from two corner points in any order.
pub fn rect_from_corners(a: Pos2, b: Pos2) -> Rect {
    Rect::from_two_pos(a, b)
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui::{pos2, vec2};

    fn rects() -> Vec<Rect> {
        vec![
            Rect::from_min_size(pos2(0.0, 0.0), vec2(100.0, 40.0)),
            Rect::from_min_size(pos2(150.0, 0.0), vec2(100.0, 40.0)),
            Rect::from_min_size(pos2(0.0, 100.0), vec2(100.0, 40.0)),
            // Overlaps node 0.
            Rect::from_min_size(pos2(50.0, 20.0), vec2(30.0, 30.0)),
        ]
    }

    #[test]
    fn hits() {
        let r = rects();
        assert_eq!(hit_node(&r, pos2(10.0, 10.0), 0.0), Some(CanvasNodeId(0)));
        assert_eq!(hit_node(&r, pos2(160.0, 39.0), 0.0), Some(CanvasNodeId(1)));
        assert_eq!(hit_node(&r, pos2(120.0, 10.0), 0.0), None);
        // The top-most rectangle wins.
        assert_eq!(hit_node(&r, pos2(60.0, 30.0), 0.0), Some(CanvasNodeId(3)));
        // Slop.
        assert_eq!(hit_node(&r, pos2(102.0, 10.0), 0.0), None);
        assert_eq!(hit_node(&r, pos2(102.0, 10.0), 3.0), Some(CanvasNodeId(0)));
        assert_eq!(hit_node(&[], pos2(0.0, 0.0), 1.0), None);
    }

    #[test]
    fn rubber_band() {
        let r = rects();
        let band = rect_from_corners(pos2(120.0, 60.0), pos2(-10.0, -10.0));
        assert_eq!(
            nodes_in_rect(&r, band),
            vec![CanvasNodeId(0), CanvasNodeId(3)]
        );
        let all = Rect::from_min_max(pos2(-1e6, -1e6), pos2(1e6, 1e6));
        assert_eq!(nodes_in_rect(&r, all).len(), 4);
        assert!(nodes_in_rect(&r, Rect::NOTHING).is_empty());
    }

    #[test]
    fn culling_reuses_the_buffer() {
        let r = rects();
        let mut out = vec![99, 98];
        visible_into(&r, Rect::from_min_size(pos2(140.0, -5.0), vec2(20.0, 20.0)), &mut out);
        assert_eq!(out, vec![1]);
        let capacity = out.capacity();
        visible_into(&r, Rect::from_min_size(pos2(0.0, 0.0), vec2(1.0, 1.0)), &mut out);
        assert_eq!(out, vec![0]);
        assert_eq!(out.capacity(), capacity);
    }
}
