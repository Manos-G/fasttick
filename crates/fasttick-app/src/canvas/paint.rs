//! Drawing primitives of the canvas: category glyphs, status badges,
//! markers and edges. All icons are shapes (no symbol glyphs: the bundled
//! font has few symbols).
//!
//! The functions push shapes into a buffer. The canvas sends the buffer to
//! the painter in one call, which is faster than one call for each shape.

use egui::epaint::{CubicBezierShape, PathShape, PathStroke};
use egui::{Color32, CornerRadius, Pos2, Rect, Shape, Stroke, StrokeKind, Vec2, pos2, vec2};
use fasttick_core::status::NodeStatus;

use super::model::{BreakpointMarker, NodeCategory, ProblemMarker};
use crate::theme::CanvasPalette;

/// Pushes a filled convex polygon given in unit coordinates (-1..1) around
/// `center`, scaled by `radius`.
fn unit_polygon(
    out: &mut Vec<Shape>,
    center: Pos2,
    radius: f32,
    points: &[(f32, f32)],
    fill: Color32,
) {
    let points = points
        .iter()
        .map(|(x, y)| center + vec2(*x, *y) * radius)
        .collect();
    out.push(Shape::convex_polygon(points, fill, Stroke::NONE));
}

/// Pushes an open polyline given in unit coordinates.
fn unit_line(
    out: &mut Vec<Shape>,
    center: Pos2,
    radius: f32,
    points: &[(f32, f32)],
    stroke: Stroke,
) {
    let points: Vec<Pos2> = points
        .iter()
        .map(|(x, y)| center + vec2(*x, *y) * radius)
        .collect();
    out.push(Shape::Path(PathShape::line(points, stroke)));
}

/// The glyph of a category, drawn in `color` inside a box of `size`.
pub fn category_glyph(
    out: &mut Vec<Shape>,
    center: Pos2,
    size: f32,
    category: NodeCategory,
    color: Color32,
) {
    let r = size * 0.36;
    match category {
        NodeCategory::Action => {
            // A lightning bolt from two triangles.
            unit_polygon(
                out,
                center,
                r,
                &[(0.32, -1.05), (-0.62, 0.18), (0.12, 0.18)],
                color,
            );
            unit_polygon(
                out,
                center,
                r,
                &[(-0.12, -0.18), (0.62, -0.18), (-0.32, 1.05)],
                color,
            );
        }
        NodeCategory::Condition => {
            // A decision diamond.
            unit_polygon(
                out,
                center,
                r * 1.05,
                &[(0.0, -1.0), (1.0, 0.0), (0.0, 1.0), (-1.0, 0.0)],
                color,
            );
        }
        NodeCategory::Control => {
            // A fork: a stem that splits into three branches.
            let stroke = Stroke::new((r * 0.26).max(0.8), color);
            unit_line(out, center, r, &[(0.0, -1.0), (0.0, -0.15)], stroke);
            unit_line(
                out,
                center,
                r,
                &[(-0.8, 0.55), (-0.8, -0.15), (0.8, -0.15), (0.8, 0.55)],
                stroke,
            );
            unit_line(out, center, r, &[(0.0, -0.15), (0.0, 0.55)], stroke);
            for x in [-0.8, 0.0, 0.8] {
                out.push(Shape::circle_filled(
                    center + vec2(x, 0.72) * r,
                    r * 0.26,
                    color,
                ));
            }
        }
        NodeCategory::Decorator => {
            // A ring around a dot: the node wraps one child.
            out.push(Shape::circle_stroke(
                center,
                r * 0.86,
                Stroke::new((r * 0.24).max(0.8), color),
            ));
            out.push(Shape::circle_filled(center, r * 0.34, color));
        }
        NodeCategory::SubTree => {
            // Two stacked boxes: a tree inside a tree.
            let side = r * 1.3;
            let back = Rect::from_center_size(center + vec2(-0.32, -0.32) * r, Vec2::splat(side));
            let front = Rect::from_center_size(center + vec2(0.32, 0.32) * r, Vec2::splat(side));
            let radius = CornerRadius::same((r * 0.25).round().clamp(0.0, 255.0) as u8);
            out.push(Shape::rect_stroke(
                back,
                radius,
                Stroke::new((r * 0.2).max(0.8), color),
                StrokeKind::Middle,
            ));
            out.push(Shape::rect_filled(front, radius, color));
        }
        NodeCategory::Unknown => {
            out.push(Shape::circle_stroke(
                center,
                r * 0.8,
                Stroke::new((r * 0.2).max(0.8), color),
            ));
        }
    }
}

/// The glyph of a status (check, cross, play, skip) in `color`. IDLE has
/// no glyph.
pub fn status_glyph(
    out: &mut Vec<Shape>,
    center: Pos2,
    radius: f32,
    status: NodeStatus,
    color: Color32,
) {
    let width = (radius * 0.34).max(1.0);
    match status {
        NodeStatus::Idle => {}
        NodeStatus::Running => {
            unit_polygon(
                out,
                center,
                radius,
                &[(-0.42, -0.72), (0.78, 0.0), (-0.42, 0.72)],
                color,
            );
        }
        NodeStatus::Success => {
            unit_line(
                out,
                center,
                radius,
                &[(-0.68, 0.02), (-0.2, 0.5), (0.7, -0.5)],
                Stroke::new(width, color),
            );
        }
        NodeStatus::Failure => {
            let stroke = Stroke::new(width, color);
            unit_line(out, center, radius, &[(-0.55, -0.55), (0.55, 0.55)], stroke);
            unit_line(out, center, radius, &[(-0.55, 0.55), (0.55, -0.55)], stroke);
        }
        NodeStatus::Skipped => {
            let stroke = Stroke::new(width * 0.9, color);
            unit_line(
                out,
                center,
                radius,
                &[(-0.62, -0.55), (-0.1, 0.0), (-0.62, 0.55)],
                stroke,
            );
            unit_line(
                out,
                center,
                radius,
                &[(0.05, -0.55), (0.57, 0.0), (0.05, 0.55)],
                stroke,
            );
        }
    }
}

/// A status badge: a solid circle with a white glyph, or, for a previous
/// status (`faded`), a ring with a glyph in the status colour.
pub fn status_badge(
    out: &mut Vec<Shape>,
    center: Pos2,
    diameter: f32,
    status: NodeStatus,
    faded: bool,
    palette: &CanvasPalette,
) {
    let Some(color) = palette.status(status) else {
        return;
    };
    let radius = diameter * 0.5;
    if faded {
        let color = color.gamma_multiply(0.75);
        out.push(Shape::circle_stroke(
            center,
            radius - 0.6,
            Stroke::new((diameter * 0.09).max(1.0), color),
        ));
        status_glyph(out, center, radius * 0.6, status, color);
    } else {
        out.push(Shape::circle_filled(center, radius, color));
        status_glyph(out, center, radius * 0.6, status, palette.on_color);
    }
}

/// A breakpoint marker: a solid circle (enabled), a ring (disabled), or a
/// circle with a halo and a "current position" arrow (hit).
pub fn breakpoint_marker(
    out: &mut Vec<Shape>,
    center: Pos2,
    radius: f32,
    marker: BreakpointMarker,
    palette: &CanvasPalette,
) {
    let color = palette.breakpoint;
    match marker {
        BreakpointMarker::Enabled => {
            out.push(Shape::circle_filled(center, radius, color));
            out.push(Shape::circle_stroke(
                center,
                radius,
                Stroke::new((radius * 0.18).max(0.8), palette.card),
            ));
        }
        BreakpointMarker::Disabled => {
            out.push(Shape::circle_stroke(
                center,
                radius * 0.85,
                Stroke::new((radius * 0.3).max(1.0), color.gamma_multiply(0.8)),
            ));
        }
        BreakpointMarker::Hit => {
            out.push(Shape::circle_filled(
                center,
                radius * 1.7,
                palette.running.gamma_multiply(0.35),
            ));
            out.push(Shape::circle_filled(center, radius, color));
            // A pause sign: the executor waits here.
            let bar = vec2(radius * 0.22, radius * 0.95);
            for dx in [-0.32, 0.32] {
                out.push(Shape::rect_filled(
                    Rect::from_center_size(center + vec2(dx * radius, 0.0), bar),
                    CornerRadius::ZERO,
                    palette.on_color,
                ));
            }
        }
    }
}

/// A problem marker: a red circle with "!" (error) or an amber triangle
/// with "!" (warning).
pub fn problem_marker(
    out: &mut Vec<Shape>,
    center: Pos2,
    radius: f32,
    marker: ProblemMarker,
    palette: &CanvasPalette,
) {
    let (fill, mark, offset) = match marker {
        ProblemMarker::Error => {
            out.push(Shape::circle_filled(center, radius, palette.error));
            (palette.error, palette.on_color, 0.0)
        }
        ProblemMarker::Warning => {
            unit_polygon(
                out,
                center,
                radius * 1.15,
                &[(0.0, -1.0), (1.0, 0.78), (-1.0, 0.78)],
                palette.warning,
            );
            (
                palette.warning,
                Color32::from_rgb(0x2B, 0x22, 0x10),
                radius * 0.15,
            )
        }
    };
    let _ = fill;
    let width = (radius * 0.26).max(1.0);
    let top = center + vec2(0.0, -radius * 0.52 + offset);
    let bottom = center + vec2(0.0, radius * 0.12 + offset);
    out.push(Shape::line_segment([top, bottom], Stroke::new(width, mark)));
    out.push(Shape::circle_filled(
        center + vec2(0.0, radius * 0.45 + offset),
        width * 0.62,
        mark,
    ));
}

/// The edge style.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Hash, Default, serde::Serialize, serde::Deserialize,
)]
pub enum EdgeStyle {
    /// Straight segments with rounded corners: down from the parent, along
    /// a shared bus, down to the child.
    #[default]
    Orthogonal,
    /// One smooth curve from the parent to each child.
    Curved,
}

impl EdgeStyle {
    /// The display name.
    pub fn name(self) -> &'static str {
        match self {
            EdgeStyle::Orthogonal => "Orthogonal",
            EdgeStyle::Curved => "Curved",
        }
    }
}

/// The points of an orthogonal edge in screen space. `start` is on the
/// parent, `end` on the child, `bus` the coordinate of the turn on the
/// depth axis, `vertical` true for a top-down tree. `radius` rounds the two
/// corners. Writes into `points` (cleared first).
pub fn orthogonal_edge_points(
    start: Pos2,
    end: Pos2,
    bus: f32,
    vertical: bool,
    radius: f32,
    points: &mut Vec<Pos2>,
) {
    points.clear();
    // Work in (breadth, depth) coordinates, then map back.
    let (sb, sd, eb, ed) = if vertical {
        (start.x, start.y, end.x, end.y)
    } else {
        (start.y, start.x, end.y, end.x)
    };
    let map = |b: f32, d: f32| if vertical { pos2(b, d) } else { pos2(d, b) };
    let delta = eb - sb;
    if delta.abs() < 0.5 {
        points.push(map(sb, sd));
        points.push(map(eb, ed));
        return;
    }
    // Keep the bus between the two ends. (Not `f32::clamp`: it panics on
    // NaN bounds.)
    let (low, high) = (sd.min(ed), sd.max(ed));
    let bus = if bus < low {
        low
    } else if bus > high {
        high
    } else {
        bus
    };
    let r = radius
        .min(delta.abs() * 0.5)
        .min((bus - sd).abs())
        .min((ed - bus).abs());
    let sign = delta.signum();
    points.push(map(sb, sd));
    if r < 0.5 {
        points.push(map(sb, bus));
        points.push(map(eb, bus));
    } else {
        // First corner: from going "down" to going "across".
        corner(
            points,
            map(sb, bus - r),
            map(sb, bus),
            map(sb + sign * r, bus),
        );
        // Second corner: from "across" to "down".
        corner(
            points,
            map(eb - sign * r, bus),
            map(eb, bus),
            map(eb, bus + r),
        );
    }
    points.push(map(eb, ed));
}

/// Adds a rounded corner from `a` to `c` with the control point `b`
/// (a quadratic curve with four segments).
fn corner(points: &mut Vec<Pos2>, a: Pos2, b: Pos2, c: Pos2) {
    for i in 0..=4 {
        let t = i as f32 / 4.0;
        let u = 1.0 - t;
        let p = a.to_vec2() * (u * u) + b.to_vec2() * (2.0 * u * t) + c.to_vec2() * (t * t);
        points.push(p.to_pos2());
    }
}

/// A curved edge from `start` to `end`.
pub fn curved_edge(start: Pos2, end: Pos2, vertical: bool, stroke: Stroke) -> Shape {
    let (c1, c2) = if vertical {
        let dy = (end.y - start.y) * 0.5;
        (start + vec2(0.0, dy), end - vec2(0.0, dy))
    } else {
        let dx = (end.x - start.x) * 0.5;
        (start + vec2(dx, 0.0), end - vec2(dx, 0.0))
    };
    Shape::CubicBezier(CubicBezierShape::from_points_stroke(
        [start, c1, c2, end],
        false,
        Color32::TRANSPARENT,
        PathStroke::new(stroke.width, stroke.color),
    ))
}

/// A dashed outline of a rectangle (square corners).
pub fn dashed_rect(out: &mut Vec<Shape>, rect: Rect, stroke: Stroke, dash: f32, gap: f32) {
    let path = [
        rect.left_top(),
        rect.right_top(),
        rect.right_bottom(),
        rect.left_bottom(),
        rect.left_top(),
    ];
    out.extend(Shape::dashed_line(&path, stroke, dash, gap));
}

/// A small pill with text, centred at `center`. Returns the pill rectangle.
pub fn pill(
    out: &mut Vec<Shape>,
    center: Pos2,
    galley: std::sync::Arc<egui::Galley>,
    fill: Color32,
    text: Color32,
    padding: Vec2,
) -> Rect {
    let size = galley.size() + padding * 2.0;
    let rect = Rect::from_center_size(center, size);
    let radius = CornerRadius::same((size.y * 0.5).round().clamp(0.0, 255.0) as u8);
    out.push(Shape::rect_filled(rect, radius, fill));
    out.push(Shape::galley(rect.min + padding, galley, text));
    rect
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn straight_edge_when_aligned() {
        let mut points = Vec::new();
        orthogonal_edge_points(
            pos2(10.0, 0.0),
            pos2(10.0, 100.0),
            50.0,
            true,
            6.0,
            &mut points,
        );
        assert_eq!(points, vec![pos2(10.0, 0.0), pos2(10.0, 100.0)]);
    }

    #[test]
    fn orthogonal_edge_turns_at_the_bus() {
        let mut points = Vec::new();
        orthogonal_edge_points(
            pos2(0.0, 0.0),
            pos2(100.0, 80.0),
            40.0,
            true,
            6.0,
            &mut points,
        );
        assert_eq!(points.first(), Some(&pos2(0.0, 0.0)));
        assert_eq!(points.last(), Some(&pos2(100.0, 80.0)));
        // All points are on the three legs of the route.
        for p in &points {
            let on_trunk = p.x.abs() < 6.01 && p.y <= 40.0 + 1e-3;
            let on_bus = (p.y - 40.0).abs() < 6.01;
            let on_drop = (p.x - 100.0).abs() < 6.01 && p.y >= 40.0 - 1e-3;
            assert!(on_trunk || on_bus || on_drop, "{p:?}");
        }
        // Left-right: the same route with the axes swapped.
        let mut swapped = Vec::new();
        orthogonal_edge_points(
            pos2(0.0, 0.0),
            pos2(80.0, 100.0),
            40.0,
            false,
            6.0,
            &mut swapped,
        );
        assert_eq!(swapped.len(), points.len());
        for (a, b) in points.iter().zip(&swapped) {
            assert!((a.x - b.y).abs() < 1e-3 && (a.y - b.x).abs() < 1e-3);
        }
        // A tiny radius gives sharp corners.
        orthogonal_edge_points(
            pos2(0.0, 0.0),
            pos2(-50.0, 80.0),
            40.0,
            true,
            0.0,
            &mut points,
        );
        assert_eq!(
            points,
            vec![
                pos2(0.0, 0.0),
                pos2(0.0, 40.0),
                pos2(-50.0, 40.0),
                pos2(-50.0, 80.0)
            ]
        );
    }

    #[test]
    fn edge_points_survive_bad_input() {
        let mut points = Vec::new();
        orthogonal_edge_points(
            pos2(f32::NAN, f32::NAN),
            pos2(f32::NAN, f32::NAN),
            f32::NAN,
            true,
            6.0,
            &mut points,
        );
        orthogonal_edge_points(
            pos2(0.0, 0.0),
            pos2(50.0, 10.0),
            f32::INFINITY,
            false,
            6.0,
            &mut points,
        );
        assert!(!points.is_empty());
    }

    #[test]
    fn glyphs_and_markers_make_shapes() {
        let palette = CanvasPalette::light();
        let mut out = Vec::new();
        for category in NodeCategory::ALL {
            out.clear();
            category_glyph(&mut out, pos2(0.0, 0.0), 18.0, category, Color32::RED);
            assert!(!out.is_empty(), "{category:?}");
        }
        for status in NodeStatus::ALL {
            out.clear();
            status_badge(&mut out, pos2(0.0, 0.0), 14.0, status, false, &palette);
            assert_eq!(out.is_empty(), status == NodeStatus::Idle);
            out.clear();
            status_badge(&mut out, pos2(0.0, 0.0), 14.0, status, true, &palette);
            assert_eq!(out.is_empty(), status == NodeStatus::Idle);
        }
        for marker in [
            BreakpointMarker::Enabled,
            BreakpointMarker::Disabled,
            BreakpointMarker::Hit,
        ] {
            out.clear();
            breakpoint_marker(&mut out, pos2(0.0, 0.0), 6.0, marker, &palette);
            assert!(!out.is_empty());
        }
        for marker in [ProblemMarker::Error, ProblemMarker::Warning] {
            out.clear();
            problem_marker(&mut out, pos2(0.0, 0.0), 7.0, marker, &palette);
            assert!(out.len() >= 3);
        }
        out.clear();
        dashed_rect(
            &mut out,
            Rect::from_min_size(Pos2::ZERO, vec2(50.0, 20.0)),
            Stroke::new(1.0, Color32::RED),
            4.0,
            3.0,
        );
        assert!(out.len() > 4);
        let curve = curved_edge(
            pos2(0.0, 0.0),
            pos2(10.0, 50.0),
            true,
            Stroke::new(1.0, Color32::RED),
        );
        assert!(matches!(curve, Shape::CubicBezier(_)));
    }
}
