//! Small geometry types of the layout: [`Vec2`] and [`Rect`].
//!
//! The layout does not use GUI types. These types convert to and from
//! `(f32, f32)` and `[f32; 2]`, so that the GUI can map them to its own
//! types.

use std::ops::{Add, AddAssign, Neg, Sub, SubAssign};

/// A point or a size in world units. The x axis points right and the y
/// axis points down.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Vec2 {
    /// The horizontal component.
    pub x: f32,
    /// The vertical component.
    pub y: f32,
}

impl Vec2 {
    /// The zero vector.
    pub const ZERO: Vec2 = Vec2 { x: 0.0, y: 0.0 };

    /// Makes a vector.
    pub const fn new(x: f32, y: f32) -> Vec2 {
        Vec2 { x, y }
    }

    /// The vector with x and y swapped.
    pub const fn swapped(self) -> Vec2 {
        Vec2 {
            x: self.y,
            y: self.x,
        }
    }

    /// True if both components are finite.
    pub fn is_finite(self) -> bool {
        self.x.is_finite() && self.y.is_finite()
    }
}

impl From<(f32, f32)> for Vec2 {
    fn from((x, y): (f32, f32)) -> Vec2 {
        Vec2 { x, y }
    }
}

impl From<[f32; 2]> for Vec2 {
    fn from([x, y]: [f32; 2]) -> Vec2 {
        Vec2 { x, y }
    }
}

impl From<Vec2> for (f32, f32) {
    fn from(v: Vec2) -> (f32, f32) {
        (v.x, v.y)
    }
}

impl From<Vec2> for [f32; 2] {
    fn from(v: Vec2) -> [f32; 2] {
        [v.x, v.y]
    }
}

impl Add for Vec2 {
    type Output = Vec2;
    fn add(self, other: Vec2) -> Vec2 {
        Vec2::new(self.x + other.x, self.y + other.y)
    }
}

impl AddAssign for Vec2 {
    fn add_assign(&mut self, other: Vec2) {
        *self = *self + other;
    }
}

impl Sub for Vec2 {
    type Output = Vec2;
    fn sub(self, other: Vec2) -> Vec2 {
        Vec2::new(self.x - other.x, self.y - other.y)
    }
}

impl SubAssign for Vec2 {
    fn sub_assign(&mut self, other: Vec2) {
        *self = *self - other;
    }
}

impl Neg for Vec2 {
    type Output = Vec2;
    fn neg(self) -> Vec2 {
        Vec2::new(-self.x, -self.y)
    }
}

/// An axis-aligned rectangle: the corner with the smallest coordinates
/// (`min`) and the corner with the largest coordinates (`max`).
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Rect {
    /// The top-left corner.
    pub min: Vec2,
    /// The bottom-right corner.
    pub max: Vec2,
}

impl Rect {
    /// The empty rectangle at the origin.
    pub const ZERO: Rect = Rect {
        min: Vec2::ZERO,
        max: Vec2::ZERO,
    };

    /// Makes a rectangle from two corners.
    pub const fn from_min_max(min: Vec2, max: Vec2) -> Rect {
        Rect { min, max }
    }

    /// Makes a rectangle from the top-left corner and the size.
    pub fn from_min_size(min: Vec2, size: Vec2) -> Rect {
        Rect {
            min,
            max: min + size,
        }
    }

    /// The width.
    pub fn width(&self) -> f32 {
        self.max.x - self.min.x
    }

    /// The height.
    pub fn height(&self) -> f32 {
        self.max.y - self.min.y
    }

    /// The size.
    pub fn size(&self) -> Vec2 {
        self.max - self.min
    }

    /// The centre point.
    pub fn center(&self) -> Vec2 {
        Vec2::new(
            (self.min.x + self.max.x) * 0.5,
            (self.min.y + self.max.y) * 0.5,
        )
    }

    /// The smallest rectangle that contains both rectangles.
    pub fn union(&self, other: Rect) -> Rect {
        Rect {
            min: Vec2::new(self.min.x.min(other.min.x), self.min.y.min(other.min.y)),
            max: Vec2::new(self.max.x.max(other.max.x), self.max.y.max(other.max.y)),
        }
    }

    /// True if the interiors of the two rectangles have a common point.
    /// Rectangles that only touch at an edge do not overlap.
    pub fn overlaps(&self, other: Rect) -> bool {
        self.min.x < other.max.x
            && other.min.x < self.max.x
            && self.min.y < other.max.y
            && other.min.y < self.max.y
    }

    /// True if `point` is in the rectangle or on its edge.
    pub fn contains(&self, point: Vec2) -> bool {
        self.min.x <= point.x
            && point.x <= self.max.x
            && self.min.y <= point.y
            && point.y <= self.max.y
    }

    /// True if `other` is completely in this rectangle.
    pub fn contains_rect(&self, other: Rect) -> bool {
        self.contains(other.min) && self.contains(other.max)
    }

    /// The rectangle moved by `offset`.
    pub fn translated(&self, offset: Vec2) -> Rect {
        Rect {
            min: self.min + offset,
            max: self.max + offset,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vec2_conversions_and_operators() {
        let v = Vec2::from((1.0, 2.0));
        assert_eq!(v, Vec2::new(1.0, 2.0));
        assert_eq!(Vec2::from([3.0, 4.0]), Vec2::new(3.0, 4.0));
        assert_eq!(<(f32, f32)>::from(v), (1.0, 2.0));
        assert_eq!(<[f32; 2]>::from(v), [1.0, 2.0]);
        assert_eq!(v + Vec2::new(1.0, 1.0), Vec2::new(2.0, 3.0));
        assert_eq!(v - Vec2::new(1.0, 1.0), Vec2::new(0.0, 1.0));
        assert_eq!(-v, Vec2::new(-1.0, -2.0));
        assert_eq!(v.swapped(), Vec2::new(2.0, 1.0));
        let mut w = v;
        w += v;
        w -= Vec2::new(0.5, 0.5);
        assert_eq!(w, Vec2::new(1.5, 3.5));
        assert!(v.is_finite());
        assert!(!Vec2::new(f32::NAN, 0.0).is_finite());
    }

    #[test]
    fn rect_operations() {
        let a = Rect::from_min_size(Vec2::new(0.0, 0.0), Vec2::new(10.0, 5.0));
        assert_eq!(a.width(), 10.0);
        assert_eq!(a.height(), 5.0);
        assert_eq!(a.size(), Vec2::new(10.0, 5.0));
        assert_eq!(a.center(), Vec2::new(5.0, 2.5));
        let b = Rect::from_min_max(Vec2::new(10.0, 0.0), Vec2::new(20.0, 5.0));
        // Touching edges do not overlap.
        assert!(!a.overlaps(b));
        let c = Rect::from_min_max(Vec2::new(9.0, 4.0), Vec2::new(12.0, 8.0));
        assert!(a.overlaps(c));
        assert!(c.overlaps(b));
        let u = a.union(b);
        assert_eq!(u, Rect::from_min_max(Vec2::ZERO, Vec2::new(20.0, 5.0)));
        assert!(u.contains_rect(a));
        assert!(u.contains_rect(b));
        assert!(!u.contains_rect(c));
        assert_eq!(
            a.translated(Vec2::new(1.0, 2.0)),
            Rect::from_min_max(Vec2::new(1.0, 2.0), Vec2::new(11.0, 7.0))
        );
        assert_eq!(Rect::ZERO.size(), Vec2::ZERO);
    }
}
