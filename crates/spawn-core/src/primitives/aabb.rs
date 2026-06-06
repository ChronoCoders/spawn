//! Axis-aligned bounding boxes in 2D and 3D collision space.

use crate::math::{Vec2, Vec3};
use crate::primitives::Rect;

/// Axis-aligned bounding box in 2D collision space. Unlike [`Rect`], all bounds
/// are inclusive: a point lying exactly on `min` or `max` is contained, and
/// overlap tests treat touching faces as intersecting. The `min ≤ max`
/// invariant holds by convention.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct AABB2 {
    pub min: Vec2,
    pub max: Vec2,
}

impl AABB2 {
    /// The caller upholds `min ≤ max`.
    pub const fn new(min: Vec2, max: Vec2) -> Self {
        Self { min, max }
    }

    /// `None` if the slice is empty.
    pub fn from_points(points: &[Vec2]) -> Option<Self> {
        let mut iter = points.iter();
        let first = *iter.next()?;
        let mut min = first;
        let mut max = first;
        for &p in iter {
            min = min.min(p);
            max = max.max(p);
        }
        Some(Self { min, max })
    }

    pub fn from_center_half_extents(center: Vec2, half: Vec2) -> Self {
        Self {
            min: center - half,
            max: center + half,
        }
    }

    pub fn center(self) -> Vec2 {
        (self.min + self.max) * 0.5
    }

    pub fn half_extents(self) -> Vec2 {
        (self.max - self.min) * 0.5
    }

    /// Inclusive on all bounds.
    pub fn contains_point(self, p: Vec2) -> bool {
        p.x >= self.min.x && p.x <= self.max.x && p.y >= self.min.y && p.y <= self.max.y
    }

    /// Inclusive.
    pub fn contains_aabb(self, rhs: Self) -> bool {
        rhs.min.x >= self.min.x
            && rhs.max.x <= self.max.x
            && rhs.min.y >= self.min.y
            && rhs.max.y <= self.max.y
    }

    /// Inclusive: touching faces count as intersecting.
    pub fn intersects(self, rhs: Self) -> bool {
        self.min.x <= rhs.max.x
            && self.max.x >= rhs.min.x
            && self.min.y <= rhs.max.y
            && self.max.y >= rhs.min.y
    }

    /// `None` if the boxes are disjoint.
    pub fn intersection(self, rhs: Self) -> Option<Self> {
        let min = self.min.max(rhs.min);
        let max = self.max.min(rhs.max);
        if min.x <= max.x && min.y <= max.y {
            Some(Self { min, max })
        } else {
            None
        }
    }

    pub fn union(self, rhs: Self) -> Self {
        Self {
            min: self.min.min(rhs.min),
            max: self.max.max(rhs.max),
        }
    }

    pub fn union_point(self, p: Vec2) -> Self {
        Self {
            min: self.min.min(p),
            max: self.max.max(p),
        }
    }

    /// Clamps `p` to the box; returns `p` itself when already inside.
    pub fn closest_point(self, p: Vec2) -> Vec2 {
        p.clamp(self.min, self.max)
    }

    /// Negative `amount` shrinks.
    pub fn expand(self, amount: f32) -> Self {
        let d = Vec2::splat(amount);
        Self {
            min: self.min - d,
            max: self.max + d,
        }
    }
}

/// Axis-aligned bounding box in 3D collision space. All bounds are inclusive,
/// matching [`AABB2`]. The `min ≤ max` invariant holds by convention.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct AABB3 {
    pub min: Vec3,
    pub max: Vec3,
}

impl AABB3 {
    /// The caller upholds `min ≤ max`.
    pub const fn new(min: Vec3, max: Vec3) -> Self {
        Self { min, max }
    }

    /// `None` if the slice is empty.
    pub fn from_points(points: &[Vec3]) -> Option<Self> {
        let mut iter = points.iter();
        let first = *iter.next()?;
        let mut min = first;
        let mut max = first;
        for &p in iter {
            min = min.min(p);
            max = max.max(p);
        }
        Some(Self { min, max })
    }

    pub fn from_center_half_extents(center: Vec3, half: Vec3) -> Self {
        Self {
            min: center - half,
            max: center + half,
        }
    }

    pub fn center(self) -> Vec3 {
        (self.min + self.max) * 0.5
    }

    pub fn half_extents(self) -> Vec3 {
        (self.max - self.min) * 0.5
    }

    /// Inclusive on all bounds.
    pub fn contains_point(self, p: Vec3) -> bool {
        p.x >= self.min.x
            && p.x <= self.max.x
            && p.y >= self.min.y
            && p.y <= self.max.y
            && p.z >= self.min.z
            && p.z <= self.max.z
    }

    /// Inclusive.
    pub fn contains_aabb(self, rhs: Self) -> bool {
        rhs.min.x >= self.min.x
            && rhs.max.x <= self.max.x
            && rhs.min.y >= self.min.y
            && rhs.max.y <= self.max.y
            && rhs.min.z >= self.min.z
            && rhs.max.z <= self.max.z
    }

    /// Inclusive: touching faces count as intersecting.
    pub fn intersects(self, rhs: Self) -> bool {
        self.min.x <= rhs.max.x
            && self.max.x >= rhs.min.x
            && self.min.y <= rhs.max.y
            && self.max.y >= rhs.min.y
            && self.min.z <= rhs.max.z
            && self.max.z >= rhs.min.z
    }

    /// `None` if the boxes are disjoint.
    pub fn intersection(self, rhs: Self) -> Option<Self> {
        let min = self.min.max(rhs.min);
        let max = self.max.min(rhs.max);
        if min.x <= max.x && min.y <= max.y && min.z <= max.z {
            Some(Self { min, max })
        } else {
            None
        }
    }

    pub fn union(self, rhs: Self) -> Self {
        Self {
            min: self.min.min(rhs.min),
            max: self.max.max(rhs.max),
        }
    }

    pub fn union_point(self, p: Vec3) -> Self {
        Self {
            min: self.min.min(p),
            max: self.max.max(p),
        }
    }

    /// Clamps `p` to the box; returns `p` itself when already inside.
    pub fn closest_point(self, p: Vec3) -> Vec3 {
        p.clamp(self.min, self.max)
    }

    /// Negative `amount` shrinks.
    pub fn expand(self, amount: f32) -> Self {
        let d = Vec3::splat(amount);
        Self {
            min: self.min - d,
            max: self.max + d,
        }
    }

    pub fn surface_area(self) -> f32 {
        let d = self.max - self.min;
        2.0 * (d.x * d.y + d.y * d.z + d.z * d.x)
    }

    pub fn volume(self) -> f32 {
        let d = self.max - self.min;
        d.x * d.y * d.z
    }
}

impl From<Rect> for AABB2 {
    fn from(r: Rect) -> Self {
        Self {
            min: r.min,
            max: r.max,
        }
    }
}

impl From<AABB2> for Rect {
    fn from(b: AABB2) -> Self {
        Rect::new(b.min, b.max)
    }
}

const _: () = assert!(core::mem::size_of::<AABB2>() == 16);
const _: () = assert!(core::mem::size_of::<AABB3>() == 24);

#[cfg(test)]
mod tests {
    use super::*;
    use crate::traits::ApproxEq;

    fn b2(x0: f32, y0: f32, x1: f32, y1: f32) -> AABB2 {
        AABB2::new(Vec2::new(x0, y0), Vec2::new(x1, y1))
    }

    fn b3(x0: f32, y0: f32, z0: f32, x1: f32, y1: f32, z1: f32) -> AABB3 {
        AABB3::new(Vec3::new(x0, y0, z0), Vec3::new(x1, y1, z1))
    }

    #[test]
    fn aabb2_new_and_default() {
        let b = b2(0.0, 1.0, 2.0, 3.0);
        assert_eq!(b.min, Vec2::new(0.0, 1.0));
        assert_eq!(b.max, Vec2::new(2.0, 3.0));
        assert_eq!(AABB2::default(), AABB2::new(Vec2::ZERO, Vec2::ZERO));
    }

    #[test]
    fn aabb2_from_points() {
        assert!(AABB2::from_points(&[]).is_none());
        let b = AABB2::from_points(&[
            Vec2::new(1.0, 5.0),
            Vec2::new(-2.0, 3.0),
            Vec2::new(4.0, -1.0),
        ])
        .unwrap();
        assert!(b.min.approx_eq_default(Vec2::new(-2.0, -1.0)));
        assert!(b.max.approx_eq_default(Vec2::new(4.0, 5.0)));
    }

    #[test]
    fn aabb2_center_half_extents() {
        let b = AABB2::from_center_half_extents(Vec2::new(1.0, 2.0), Vec2::new(3.0, 4.0));
        assert!(b.min.approx_eq_default(Vec2::new(-2.0, -2.0)));
        assert!(b.max.approx_eq_default(Vec2::new(4.0, 6.0)));
        assert!(b.center().approx_eq_default(Vec2::new(1.0, 2.0)));
        assert!(b.half_extents().approx_eq_default(Vec2::new(3.0, 4.0)));
    }

    #[test]
    fn aabb2_contains_point_inclusive() {
        let b = b2(0.0, 0.0, 10.0, 10.0);
        assert!(b.contains_point(Vec2::new(5.0, 5.0)));
        assert!(b.contains_point(Vec2::new(0.0, 0.0)));
        // Max corner is contained (inclusive).
        assert!(b.contains_point(Vec2::new(10.0, 10.0)));
        assert!(!b.contains_point(Vec2::new(10.1, 5.0)));
    }

    #[test]
    fn aabb2_contains_aabb() {
        let b = b2(0.0, 0.0, 10.0, 10.0);
        assert!(b.contains_aabb(b2(2.0, 2.0, 8.0, 8.0)));
        assert!(b.contains_aabb(b2(0.0, 0.0, 10.0, 10.0)));
        assert!(!b.contains_aabb(b2(-1.0, 0.0, 5.0, 5.0)));
    }

    #[test]
    fn aabb2_intersects_inclusive() {
        let a = b2(0.0, 0.0, 10.0, 10.0);
        assert!(a.intersects(b2(5.0, 5.0, 15.0, 15.0)));
        // Touching faces intersect (inclusive).
        assert!(a.intersects(b2(10.0, 0.0, 20.0, 10.0)));
        assert!(!a.intersects(b2(11.0, 0.0, 20.0, 10.0)));
    }

    #[test]
    fn aabb2_intersection() {
        let a = b2(0.0, 0.0, 10.0, 10.0);
        let i = a.intersection(b2(5.0, 5.0, 15.0, 15.0)).unwrap();
        assert!(i.min.approx_eq_default(Vec2::new(5.0, 5.0)));
        assert!(i.max.approx_eq_default(Vec2::new(10.0, 10.0)));
        // Touching faces -> zero-extent box (inclusive).
        let t = a.intersection(b2(10.0, 0.0, 20.0, 10.0)).unwrap();
        assert!(t.min.approx_eq_default(Vec2::new(10.0, 0.0)));
        assert!(t.max.approx_eq_default(Vec2::new(10.0, 10.0)));
        assert!(a.intersection(b2(11.0, 0.0, 20.0, 10.0)).is_none());
    }

    #[test]
    fn aabb2_union_and_point() {
        let a = b2(0.0, 0.0, 5.0, 5.0);
        let u = a.union(b2(3.0, 3.0, 10.0, 8.0));
        assert!(u.min.approx_eq_default(Vec2::new(0.0, 0.0)));
        assert!(u.max.approx_eq_default(Vec2::new(10.0, 8.0)));
        let up = a.union_point(Vec2::new(-1.0, 7.0));
        assert!(up.min.approx_eq_default(Vec2::new(-1.0, 0.0)));
        assert!(up.max.approx_eq_default(Vec2::new(5.0, 7.0)));
    }

    #[test]
    fn aabb2_closest_point() {
        let b = b2(0.0, 0.0, 10.0, 10.0);
        // Inside returns the point itself.
        assert!(b
            .closest_point(Vec2::new(5.0, 5.0))
            .approx_eq_default(Vec2::new(5.0, 5.0)));
        // Outside clamps.
        assert!(b
            .closest_point(Vec2::new(-5.0, 15.0))
            .approx_eq_default(Vec2::new(0.0, 10.0)));
    }

    #[test]
    fn aabb2_expand() {
        let b = b2(0.0, 0.0, 10.0, 10.0).expand(2.0);
        assert!(b.min.approx_eq_default(Vec2::new(-2.0, -2.0)));
        assert!(b.max.approx_eq_default(Vec2::new(12.0, 12.0)));
    }

    #[test]
    fn aabb3_new_and_default() {
        let b = b3(0.0, 1.0, 2.0, 3.0, 4.0, 5.0);
        assert_eq!(b.min, Vec3::new(0.0, 1.0, 2.0));
        assert_eq!(b.max, Vec3::new(3.0, 4.0, 5.0));
        assert_eq!(AABB3::default(), AABB3::new(Vec3::ZERO, Vec3::ZERO));
    }

    #[test]
    fn aabb3_from_points() {
        assert!(AABB3::from_points(&[]).is_none());
        let b = AABB3::from_points(&[
            Vec3::new(1.0, 5.0, 0.0),
            Vec3::new(-2.0, 3.0, 9.0),
            Vec3::new(4.0, -1.0, -3.0),
        ])
        .unwrap();
        assert!(b.min.approx_eq_default(Vec3::new(-2.0, -1.0, -3.0)));
        assert!(b.max.approx_eq_default(Vec3::new(4.0, 5.0, 9.0)));
    }

    #[test]
    fn aabb3_center_half_extents() {
        let b = AABB3::from_center_half_extents(Vec3::new(1.0, 2.0, 3.0), Vec3::new(1.0, 2.0, 3.0));
        assert!(b.center().approx_eq_default(Vec3::new(1.0, 2.0, 3.0)));
        assert!(b.half_extents().approx_eq_default(Vec3::new(1.0, 2.0, 3.0)));
    }

    #[test]
    fn aabb3_contains_point_inclusive() {
        let b = b3(0.0, 0.0, 0.0, 10.0, 10.0, 10.0);
        assert!(b.contains_point(Vec3::new(5.0, 5.0, 5.0)));
        // Max corner contained.
        assert!(b.contains_point(Vec3::new(10.0, 10.0, 10.0)));
        assert!(!b.contains_point(Vec3::new(5.0, 5.0, 10.1)));
    }

    #[test]
    fn aabb3_contains_aabb() {
        let b = b3(0.0, 0.0, 0.0, 10.0, 10.0, 10.0);
        assert!(b.contains_aabb(b3(2.0, 2.0, 2.0, 8.0, 8.0, 8.0)));
        assert!(!b.contains_aabb(b3(2.0, 2.0, 2.0, 8.0, 8.0, 11.0)));
    }

    #[test]
    fn aabb3_intersects_inclusive() {
        let a = b3(0.0, 0.0, 0.0, 10.0, 10.0, 10.0);
        assert!(a.intersects(b3(5.0, 5.0, 5.0, 15.0, 15.0, 15.0)));
        assert!(a.intersects(b3(10.0, 0.0, 0.0, 20.0, 10.0, 10.0)));
        assert!(!a.intersects(b3(11.0, 0.0, 0.0, 20.0, 10.0, 10.0)));
    }

    #[test]
    fn aabb3_intersection() {
        let a = b3(0.0, 0.0, 0.0, 10.0, 10.0, 10.0);
        let i = a.intersection(b3(5.0, 5.0, 5.0, 15.0, 15.0, 15.0)).unwrap();
        assert!(i.min.approx_eq_default(Vec3::new(5.0, 5.0, 5.0)));
        assert!(i.max.approx_eq_default(Vec3::new(10.0, 10.0, 10.0)));
        assert!(a
            .intersection(b3(11.0, 0.0, 0.0, 20.0, 10.0, 10.0))
            .is_none());
    }

    #[test]
    fn aabb3_union_and_point() {
        let a = b3(0.0, 0.0, 0.0, 5.0, 5.0, 5.0);
        let u = a.union(b3(3.0, 3.0, 3.0, 10.0, 8.0, 6.0));
        assert!(u.min.approx_eq_default(Vec3::new(0.0, 0.0, 0.0)));
        assert!(u.max.approx_eq_default(Vec3::new(10.0, 8.0, 6.0)));
        let up = a.union_point(Vec3::new(-1.0, 7.0, 2.0));
        assert!(up.min.approx_eq_default(Vec3::new(-1.0, 0.0, 0.0)));
        assert!(up.max.approx_eq_default(Vec3::new(5.0, 7.0, 5.0)));
    }

    #[test]
    fn aabb3_closest_point() {
        let b = b3(0.0, 0.0, 0.0, 10.0, 10.0, 10.0);
        assert!(b
            .closest_point(Vec3::new(5.0, 5.0, 5.0))
            .approx_eq_default(Vec3::new(5.0, 5.0, 5.0)));
        assert!(b
            .closest_point(Vec3::new(-5.0, 15.0, 5.0))
            .approx_eq_default(Vec3::new(0.0, 10.0, 5.0)));
    }

    #[test]
    fn aabb3_expand() {
        let b = b3(0.0, 0.0, 0.0, 10.0, 10.0, 10.0).expand(1.0);
        assert!(b.min.approx_eq_default(Vec3::new(-1.0, -1.0, -1.0)));
        assert!(b.max.approx_eq_default(Vec3::new(11.0, 11.0, 11.0)));
    }

    #[test]
    fn aabb3_surface_area_and_volume() {
        let b = b3(0.0, 0.0, 0.0, 2.0, 3.0, 4.0);
        // 2*(2*3 + 3*4 + 4*2) = 2*(6+12+8) = 52
        assert!(b.surface_area().approx_eq_default(52.0));
        assert!(b.volume().approx_eq_default(24.0));
    }

    #[test]
    fn rect_aabb2_conversions() {
        let r = Rect::new(Vec2::new(1.0, 2.0), Vec2::new(3.0, 4.0));
        let b: AABB2 = r.into();
        assert!(b.min.approx_eq_default(r.min));
        assert!(b.max.approx_eq_default(r.max));
        let back: Rect = b.into();
        assert_eq!(back, r);
    }
}
