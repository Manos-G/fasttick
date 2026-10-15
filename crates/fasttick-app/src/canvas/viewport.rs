//! Viewport maths: the map from world coordinates (the layout) to screen
//! coordinates (the canvas widget). All functions are pure.
//!
//! `screen = origin + pan + world * zoom`, where `origin` is the top-left
//! corner of the canvas rectangle on the screen.

use egui::{Pos2, Rect, Vec2, pos2, vec2};

/// The smallest zoom factor.
pub const MIN_ZOOM: f32 = 0.01;
/// The largest zoom factor.
pub const MAX_ZOOM: f32 = 4.0;
/// The factor of one zoom step (menu, keyboard).
pub const ZOOM_STEP: f32 = 1.25;
/// The largest zoom that "fit to view" uses: a small tree does not grow
/// past 100 %.
pub const FIT_MAX_ZOOM: f32 = 1.0;
/// The margin around the tree for "fit to view", in screen points.
pub const FIT_MARGIN: f32 = 32.0;

/// The pan and zoom of a canvas.
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Viewport {
    /// The screen offset of the world origin from the canvas origin.
    pub pan: Vec2,
    /// The scale from world units to screen points.
    pub zoom: f32,
}

impl Default for Viewport {
    fn default() -> Self {
        Viewport {
            pan: Vec2::ZERO,
            zoom: 1.0,
        }
    }
}

impl Viewport {
    /// A viewport with this pan and zoom. The zoom is clamped.
    pub fn new(pan: Vec2, zoom: f32) -> Self {
        Viewport {
            pan,
            zoom: clamp_zoom(zoom),
        }
    }

    /// Converts a world point to a screen point.
    pub fn to_screen(&self, origin: Pos2, world: Pos2) -> Pos2 {
        origin + self.pan + world.to_vec2() * self.zoom
    }

    /// Converts a screen point to a world point.
    pub fn to_world(&self, origin: Pos2, screen: Pos2) -> Pos2 {
        ((screen - origin - self.pan) / self.zoom).to_pos2()
    }

    /// Converts a world rectangle to a screen rectangle.
    pub fn rect_to_screen(&self, origin: Pos2, world: Rect) -> Rect {
        Rect::from_min_max(
            self.to_screen(origin, world.min),
            self.to_screen(origin, world.max),
        )
    }

    /// Converts a screen rectangle to a world rectangle.
    pub fn rect_to_world(&self, origin: Pos2, screen: Rect) -> Rect {
        Rect::from_min_max(
            self.to_world(origin, screen.min),
            self.to_world(origin, screen.max),
        )
    }

    /// The part of the world that a canvas rectangle shows.
    pub fn visible_world(&self, canvas: Rect) -> Rect {
        self.rect_to_world(canvas.min, canvas)
    }

    /// Moves the view by a screen distance.
    pub fn pan_by(&mut self, delta: Vec2) {
        self.pan += delta;
    }

    /// Multiplies the zoom by `factor` and keeps the world point under the
    /// screen point `anchor` in place. The zoom is clamped.
    pub fn zoom_around(&mut self, origin: Pos2, anchor: Pos2, factor: f32) {
        if !factor.is_finite() || factor <= 0.0 {
            return;
        }
        let world = self.to_world(origin, anchor);
        self.zoom = clamp_zoom(self.zoom * factor);
        // Solve anchor = origin + pan + world * zoom for pan.
        self.pan = anchor - origin - world.to_vec2() * self.zoom;
    }

    /// Sets the zoom and keeps the world point under `anchor` in place.
    pub fn set_zoom_around(&mut self, origin: Pos2, anchor: Pos2, zoom: f32) {
        let factor = clamp_zoom(zoom) / self.zoom;
        self.zoom_around(origin, anchor, factor);
    }

    /// The viewport that shows all of `bounds` in a canvas of `size`, with
    /// a margin, centred. The zoom is at most `max_zoom`.
    pub fn fit(bounds: Rect, size: Vec2, margin: f32, max_zoom: f32) -> Viewport {
        if !is_usable(bounds) || size.x <= 0.0 || size.y <= 0.0 {
            return Viewport::default();
        }
        let available = vec2(
            (size.x - 2.0 * margin).max(1.0),
            (size.y - 2.0 * margin).max(1.0),
        );
        let width = bounds.width().max(1.0);
        let height = bounds.height().max(1.0);
        let zoom = clamp_zoom(
            (available.x / width)
                .min(available.y / height)
                .min(max_zoom),
        );
        Viewport::centered_on(bounds.center(), size, zoom)
    }

    /// The viewport that shows the world point `center` in the middle of a
    /// canvas of `size`.
    pub fn centered_on(center: Pos2, size: Vec2, zoom: f32) -> Viewport {
        let zoom = clamp_zoom(zoom);
        Viewport {
            pan: size * 0.5 - center.to_vec2() * zoom,
            zoom,
        }
    }

    /// The world point in the middle of a canvas of `size`.
    pub fn center(&self, size: Vec2) -> Pos2 {
        ((size * 0.5 - self.pan) / self.zoom).to_pos2()
    }

    /// The smallest pan change that makes `world` fully visible in a
    /// canvas of `size`, with a margin. Returns the viewport unchanged if
    /// `world` is visible already. If `world` is larger than the canvas,
    /// the result shows its top-left part.
    pub fn reveal(&self, world: Rect, size: Vec2, margin: f32) -> Viewport {
        let screen = Rect::from_min_max(
            (self.pan + world.min.to_vec2() * self.zoom).to_pos2(),
            (self.pan + world.max.to_vec2() * self.zoom).to_pos2(),
        );
        let inner = Rect::from_min_size(Pos2::ZERO, size).shrink(margin);
        let mut shift = Vec2::ZERO;
        for axis in 0..2 {
            let (lo, hi, inner_lo, inner_hi) = (
                screen.min[axis],
                screen.max[axis],
                inner.min[axis],
                inner.max[axis],
            );
            shift[axis] = if hi - lo > inner_hi - inner_lo || lo < inner_lo {
                inner_lo - lo
            } else if hi > inner_hi {
                inner_hi - hi
            } else {
                0.0
            };
        }
        Viewport {
            pan: self.pan + shift,
            zoom: self.zoom,
        }
    }

    /// Interpolates between two viewports for an animation. The zoom
    /// changes on a log scale and the centre of the view moves on a straight
    /// line, so the motion looks even. `t` is from 0 to 1.
    pub fn lerp(from: Viewport, to: Viewport, size: Vec2, t: f32) -> Viewport {
        let t = t.clamp(0.0, 1.0);
        let zoom = (from.zoom.ln() + (to.zoom.ln() - from.zoom.ln()) * t).exp();
        let a = from.center(size);
        let b = to.center(size);
        let center = a + (b - a) * t;
        Viewport::centered_on(center, size, zoom)
    }

    /// True if the values are finite and the zoom is in range.
    pub fn is_valid(&self) -> bool {
        self.pan.x.is_finite()
            && self.pan.y.is_finite()
            && self.zoom.is_finite()
            && (MIN_ZOOM..=MAX_ZOOM).contains(&self.zoom)
    }
}

/// Clamps a zoom factor to [`MIN_ZOOM`]..=[`MAX_ZOOM`]. A value that is not
/// finite becomes 1.
pub fn clamp_zoom(zoom: f32) -> f32 {
    if zoom.is_finite() {
        zoom.clamp(MIN_ZOOM, MAX_ZOOM)
    } else {
        1.0
    }
}

/// An ease-out curve for animations (cubic).
pub fn ease_out_cubic(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    1.0 - (1.0 - t).powi(3)
}

fn is_usable(rect: Rect) -> bool {
    rect.min.x.is_finite()
        && rect.min.y.is_finite()
        && rect.max.x.is_finite()
        && rect.max.y.is_finite()
        && rect.min.x <= rect.max.x
        && rect.min.y <= rect.max.y
}

/// The number of text buckets in one doubling of the zoom.
pub const BUCKETS_PER_DOUBLING: f32 = 48.0;

/// The zoom "bucket" for cached text: zoom values in one bucket use the
/// same font size. A bucket is 1/48 of a doubling (about 1.5 %), so the
/// change of size between two buckets is too small to see.
pub fn zoom_bucket(zoom: f32) -> i32 {
    (clamp_zoom(zoom).log2() * BUCKETS_PER_DOUBLING).floor() as i32
}

/// The zoom of a bucket (the lower bound, so that text never grows larger
/// than its box).
pub fn bucket_zoom(bucket: i32) -> f32 {
    (bucket as f32 / BUCKETS_PER_DOUBLING).exp2()
}

/// A point in the middle of two points.
pub fn midpoint(a: Pos2, b: Pos2) -> Pos2 {
    pos2((a.x + b.x) * 0.5, (a.y + b.y) * 0.5)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: f32, b: f32) -> bool {
        (a - b).abs() < 1e-3
    }

    fn close_pos(a: Pos2, b: Pos2) -> bool {
        close(a.x, b.x) && close(a.y, b.y)
    }

    #[test]
    fn round_trip() {
        let view = Viewport::new(vec2(30.0, -12.0), 1.7);
        let origin = pos2(100.0, 50.0);
        for p in [pos2(0.0, 0.0), pos2(-40.0, 33.5), pos2(1000.0, -200.0)] {
            assert!(close_pos(
                view.to_world(origin, view.to_screen(origin, p)),
                p
            ));
        }
        let rect = Rect::from_min_max(pos2(-5.0, 2.0), pos2(20.0, 40.0));
        let back = view.rect_to_world(origin, view.rect_to_screen(origin, rect));
        assert!(close_pos(back.min, rect.min) && close_pos(back.max, rect.max));
    }

    #[test]
    fn zoom_keeps_anchor_in_place() {
        let mut view = Viewport::new(vec2(10.0, 20.0), 1.0);
        let origin = pos2(5.0, 5.0);
        let anchor = pos2(300.0, 200.0);
        let world = view.to_world(origin, anchor);
        view.zoom_around(origin, anchor, 2.0);
        assert!(close(view.zoom, 2.0));
        assert!(close_pos(view.to_screen(origin, world), anchor));
        view.set_zoom_around(origin, anchor, 0.5);
        assert!(close(view.zoom, 0.5));
        assert!(close_pos(view.to_screen(origin, world), anchor));
    }

    #[test]
    fn zoom_limits() {
        let mut view = Viewport::default();
        let origin = Pos2::ZERO;
        view.zoom_around(origin, Pos2::ZERO, 1000.0);
        assert_eq!(view.zoom, MAX_ZOOM);
        view.zoom_around(origin, Pos2::ZERO, 1e-6);
        assert_eq!(view.zoom, MIN_ZOOM);
        let before = view;
        view.zoom_around(origin, Pos2::ZERO, f32::NAN);
        view.zoom_around(origin, Pos2::ZERO, -1.0);
        assert_eq!(view, before);
        assert_eq!(clamp_zoom(f32::INFINITY), 1.0);
        assert!(view.is_valid());
        assert!(
            !Viewport {
                pan: Vec2::ZERO,
                zoom: 99.0
            }
            .is_valid()
        );
    }

    #[test]
    fn fit_centres_and_limits_zoom() {
        let size = vec2(800.0, 600.0);
        // A small tree is not enlarged past FIT_MAX_ZOOM.
        let small = Rect::from_min_size(pos2(100.0, 100.0), vec2(100.0, 50.0));
        let view = Viewport::fit(small, size, FIT_MARGIN, FIT_MAX_ZOOM);
        assert!(close(view.zoom, 1.0));
        assert!(close_pos(
            view.to_screen(Pos2::ZERO, small.center()),
            pos2(400.0, 300.0)
        ));

        // A wide tree fits the width.
        let wide = Rect::from_min_size(pos2(-1000.0, 0.0), vec2(4000.0, 200.0));
        let view = Viewport::fit(wide, size, FIT_MARGIN, FIT_MAX_ZOOM);
        assert!(close(view.zoom, (800.0 - 2.0 * FIT_MARGIN) / 4000.0));
        let screen = view.rect_to_screen(Pos2::ZERO, wide);
        assert!(close(screen.min.x, FIT_MARGIN));
        assert!(close(screen.max.x, 800.0 - FIT_MARGIN));
        assert!(close(screen.center().y, 300.0));

        // Bad input gives the default view.
        let nan = Rect::from_min_max(pos2(f32::NAN, 0.0), pos2(1.0, 1.0));
        assert_eq!(Viewport::fit(nan, size, 0.0, 1.0), Viewport::default());
        assert_eq!(
            Viewport::fit(small, Vec2::ZERO, 0.0, 1.0),
            Viewport::default()
        );
        // A huge tree uses the smallest zoom.
        let huge = Rect::from_min_size(Pos2::ZERO, vec2(1e9, 1e9));
        assert_eq!(Viewport::fit(huge, size, 0.0, 1.0).zoom, MIN_ZOOM);
    }

    #[test]
    fn visible_world_and_center() {
        let view = Viewport::new(vec2(-100.0, -50.0), 2.0);
        let canvas = Rect::from_min_size(pos2(10.0, 10.0), vec2(400.0, 200.0));
        let world = view.visible_world(canvas);
        assert!(close_pos(world.min, pos2(50.0, 25.0)));
        assert!(close_pos(world.max, pos2(250.0, 125.0)));
        assert!(close_pos(view.center(canvas.size()), world.center()));
        let centered = Viewport::centered_on(pos2(7.0, 9.0), canvas.size(), 0.5);
        assert!(close_pos(centered.center(canvas.size()), pos2(7.0, 9.0)));
    }

    #[test]
    fn reveal_moves_only_when_needed() {
        let size = vec2(400.0, 300.0);
        let view = Viewport::new(Vec2::ZERO, 1.0);
        let inside = Rect::from_min_size(pos2(50.0, 50.0), vec2(40.0, 20.0));
        assert_eq!(view.reveal(inside, size, 10.0), view);
        let right = Rect::from_min_size(pos2(500.0, 50.0), vec2(40.0, 20.0));
        let moved = view.reveal(right, size, 10.0);
        let screen = moved.rect_to_screen(Pos2::ZERO, right);
        assert!(close(screen.max.x, 390.0));
        assert!(close(moved.pan.y, 0.0));
        let above = Rect::from_min_size(pos2(50.0, -100.0), vec2(40.0, 20.0));
        let moved = view.reveal(above, size, 10.0);
        assert!(close(moved.rect_to_screen(Pos2::ZERO, above).min.y, 10.0));
        // Larger than the canvas: show the top-left part.
        let big = Rect::from_min_size(pos2(1000.0, 1000.0), vec2(2000.0, 2000.0));
        let moved = view.reveal(big, size, 10.0);
        assert!(close_pos(
            moved.rect_to_screen(Pos2::ZERO, big).min,
            pos2(10.0, 10.0)
        ));
    }

    #[test]
    fn lerp_ends_and_middle() {
        let size = vec2(800.0, 600.0);
        let a = Viewport::centered_on(pos2(0.0, 0.0), size, 0.5);
        let b = Viewport::centered_on(pos2(1000.0, 500.0), size, 2.0);
        let start = Viewport::lerp(a, b, size, 0.0);
        let end = Viewport::lerp(a, b, size, 1.0);
        assert!(close(start.zoom, 0.5) && close_pos(start.center(size), pos2(0.0, 0.0)));
        assert!(close(end.zoom, 2.0) && close_pos(end.center(size), pos2(1000.0, 500.0)));
        let mid = Viewport::lerp(a, b, size, 0.5);
        assert!(close(mid.zoom, 1.0));
        assert!(close_pos(mid.center(size), pos2(500.0, 250.0)));
        assert!(close(ease_out_cubic(0.0), 0.0));
        assert!(close(ease_out_cubic(1.0), 1.0));
        assert!(ease_out_cubic(0.5) > 0.5);
    }

    #[test]
    fn buckets() {
        assert_eq!(zoom_bucket(1.0), 0);
        assert!(bucket_zoom(zoom_bucket(1.3)) <= 1.3);
        assert!(bucket_zoom(zoom_bucket(1.3)) > 1.3 / 1.02);
        assert!(bucket_zoom(zoom_bucket(0.07)) <= 0.07);
        assert_eq!(zoom_bucket(0.5), -48);
        assert!(close(bucket_zoom(-48), 0.5));
        assert_eq!(midpoint(pos2(0.0, 0.0), pos2(2.0, 4.0)), pos2(1.0, 2.0));
    }
}
