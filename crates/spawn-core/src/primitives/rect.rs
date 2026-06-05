//! Axis-aligned screen/UI-space rectangle with half-open containment.

use crate::math::Vec2;

/// Axis-aligned rectangle in screen/UI space. The `min ≤ max` invariant holds
/// by convention; [`Rect::new`] trusts the caller while [`Rect::from_points`]
/// normalizes. Containment and overlap use half-open semantics
/// (min-inclusive, max-exclusive), matching pixel/cell conventions.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Rect {
    /// Minimum corner (top-left in a y-down convention).
    pub min: Vec2,
    /// Maximum corner (exclusive bound).
    pub max: Vec2,
}

impl Rect {
    /// Creates a rectangle from explicit corners. The caller upholds the
    /// `min ≤ max` invariant.
    pub const fn new(min: Vec2, max: Vec2) -> Self {
        Self { min, max }
    }

    /// Creates a rectangle spanning two arbitrary points, normalizing so that
    /// `min ≤ max` componentwise.
    pub fn from_points(a: Vec2, b: Vec2) -> Self {
        Self {
            min: a.min(b),
            max: a.max(b),
        }
    }

    /// Creates a rectangle from a center and full size.
    pub fn from_center_size(center: Vec2, size: Vec2) -> Self {
        let half = size * 0.5;
        Self {
            min: center - half,
            max: center + half,
        }
    }

    /// Returns the width (extent along x).
    pub fn width(self) -> f32 {
        self.max.x - self.min.x
    }

    /// Returns the height (extent along y).
    pub fn height(self) -> f32 {
        self.max.y - self.min.y
    }

    /// Returns the size as a vector `(width, height)`.
    pub fn size(self) -> Vec2 {
        self.max - self.min
    }

    /// Returns the center point.
    pub fn center(self) -> Vec2 {
        (self.min + self.max) * 0.5
    }

    /// Returns the area (`width * height`).
    pub fn area(self) -> f32 {
        self.width() * self.height()
    }

    /// Returns `true` if the point lies inside the rectangle. Containment is
    /// min-inclusive and max-exclusive: a point on the `min` edge is contained,
    /// a point on the `max` edge is not.
    pub fn contains_point(self, p: Vec2) -> bool {
        p.x >= self.min.x && p.x < self.max.x && p.y >= self.min.y && p.y < self.max.y
    }

    /// Returns `true` if the two rectangles overlap with positive area. Edges
    /// that merely touch (zero overlap) do not count, consistent with the
    /// half-open convention.
    pub fn intersects(self, rhs: Self) -> bool {
        self.min.x < rhs.max.x
            && self.max.x > rhs.min.x
            && self.min.y < rhs.max.y
            && self.max.y > rhs.min.y
    }

    /// Returns the overlapping rectangle, or `None` if the rectangles do not
    /// overlap with positive area.
    pub fn intersection(self, rhs: Self) -> Option<Self> {
        let min = self.min.max(rhs.min);
        let max = self.max.min(rhs.max);
        if min.x < max.x && min.y < max.y {
            Some(Self { min, max })
        } else {
            None
        }
    }

    /// Returns the smallest rectangle covering both inputs.
    pub fn union(self, rhs: Self) -> Self {
        Self {
            min: self.min.min(rhs.min),
            max: self.max.max(rhs.max),
        }
    }

    /// Returns the rectangle grown outward by `amount` on every side. Negative
    /// values shrink it.
    pub fn expand(self, amount: f32) -> Self {
        let d = Vec2::splat(amount);
        Self {
            min: self.min - d,
            max: self.max + d,
        }
    }

    /// Returns the rectangle translated by `offset`.
    pub fn translate(self, offset: Vec2) -> Self {
        Self {
            min: self.min + offset,
            max: self.max + offset,
        }
    }
}

const _: () = assert!(core::mem::size_of::<Rect>() == 16);

#[cfg(test)]
mod tests {
    use super::*;
    use crate::traits::ApproxEq;

    fn r(x0: f32, y0: f32, x1: f32, y1: f32) -> Rect {
        Rect::new(Vec2::new(x0, y0), Vec2::new(x1, y1))
    }

    #[test]
    fn new() {
        let rect = r(0.0, 1.0, 2.0, 3.0);
        assert_eq!(rect.min, Vec2::new(0.0, 1.0));
        assert_eq!(rect.max, Vec2::new(2.0, 3.0));
    }

    #[test]
    fn from_points_normalizes() {
        let rect = Rect::from_points(Vec2::new(3.0, 4.0), Vec2::new(1.0, 2.0));
        assert_eq!(rect.min, Vec2::new(1.0, 2.0));
        assert_eq!(rect.max, Vec2::new(3.0, 4.0));
    }

    #[test]
    fn from_center_size() {
        let rect = Rect::from_center_size(Vec2::new(1.0, 1.0), Vec2::new(2.0, 4.0));
        assert!(rect.min.approx_eq_default(Vec2::new(0.0, -1.0)));
        assert!(rect.max.approx_eq_default(Vec2::new(2.0, 3.0)));
    }

    #[test]
    fn dimensions() {
        let rect = r(0.0, 0.0, 2.0, 3.0);
        assert!(rect.width().approx_eq_default(2.0));
        assert!(rect.height().approx_eq_default(3.0));
        assert!(rect.size().approx_eq_default(Vec2::new(2.0, 3.0)));
        assert!(rect.center().approx_eq_default(Vec2::new(1.0, 1.5)));
        assert!(rect.area().approx_eq_default(6.0));
    }

    #[test]
    fn contains_point_half_open() {
        let rect = r(0.0, 0.0, 10.0, 10.0);
        assert!(rect.contains_point(Vec2::new(5.0, 5.0)));
        // Min edge inclusive.
        assert!(rect.contains_point(Vec2::new(0.0, 0.0)));
        assert!(rect.contains_point(Vec2::new(0.0, 5.0)));
        // Max edge exclusive.
        assert!(!rect.contains_point(Vec2::new(10.0, 5.0)));
        assert!(!rect.contains_point(Vec2::new(5.0, 10.0)));
        assert!(!rect.contains_point(Vec2::new(10.0, 10.0)));
        assert!(!rect.contains_point(Vec2::new(-1.0, 5.0)));
    }

    #[test]
    fn intersects() {
        let a = r(0.0, 0.0, 10.0, 10.0);
        assert!(a.intersects(r(5.0, 5.0, 15.0, 15.0)));
        // Touching edges do not intersect (half-open).
        assert!(!a.intersects(r(10.0, 0.0, 20.0, 10.0)));
        assert!(!a.intersects(r(20.0, 20.0, 30.0, 30.0)));
    }

    #[test]
    fn intersection() {
        let a = r(0.0, 0.0, 10.0, 10.0);
        let b = r(5.0, 5.0, 15.0, 15.0);
        let i = a.intersection(b).unwrap();
        assert!(i.min.approx_eq_default(Vec2::new(5.0, 5.0)));
        assert!(i.max.approx_eq_default(Vec2::new(10.0, 10.0)));
        // Touching edges yield zero area -> None.
        assert!(a.intersection(r(10.0, 0.0, 20.0, 10.0)).is_none());
        // Disjoint -> None.
        assert!(a.intersection(r(20.0, 20.0, 30.0, 30.0)).is_none());
    }

    #[test]
    fn union() {
        let a = r(0.0, 0.0, 5.0, 5.0);
        let b = r(3.0, 3.0, 10.0, 8.0);
        let u = a.union(b);
        assert!(u.min.approx_eq_default(Vec2::new(0.0, 0.0)));
        assert!(u.max.approx_eq_default(Vec2::new(10.0, 8.0)));
    }

    #[test]
    fn expand() {
        let rect = r(0.0, 0.0, 10.0, 10.0).expand(2.0);
        assert!(rect.min.approx_eq_default(Vec2::new(-2.0, -2.0)));
        assert!(rect.max.approx_eq_default(Vec2::new(12.0, 12.0)));
    }

    #[test]
    fn translate() {
        let rect = r(0.0, 0.0, 10.0, 10.0).translate(Vec2::new(1.0, 2.0));
        assert!(rect.min.approx_eq_default(Vec2::new(1.0, 2.0)));
        assert!(rect.max.approx_eq_default(Vec2::new(11.0, 12.0)));
    }

    #[test]
    fn default_is_zero() {
        assert_eq!(Rect::default(), Rect::new(Vec2::ZERO, Vec2::ZERO));
    }
}
