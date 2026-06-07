//! Private conversions between spawn-core 2D math and nalgebra/Rapier types.
//!
//! No type defined here, and no nalgebra/Rapier type, escapes the `physics2d`
//! module boundary.

use rapier2d::na::{Isometry2, Point2, Translation2, UnitComplex, Vector2};
use rapier2d::prelude as rp;
use spawn_core::{Transform2D, Vec2};

use crate::handles::{ColliderHandle, JointHandle, RigidBodyHandle};

pub(crate) fn vec_to_na(v: Vec2) -> Vector2<f32> {
    Vector2::new(v.x, v.y)
}

pub(crate) fn na_to_vec(v: Vector2<f32>) -> Vec2 {
    Vec2::new(v.x, v.y)
}

pub(crate) fn point_to_vec(p: Point2<f32>) -> Vec2 {
    Vec2::new(p.x, p.y)
}

pub(crate) fn transform_to_iso(t: Transform2D) -> Isometry2<f32> {
    Isometry2::from_parts(
        Translation2::new(t.translation.x, t.translation.y),
        UnitComplex::new(t.rotation),
    )
}

pub(crate) fn iso_to_transform(iso: &Isometry2<f32>) -> Transform2D {
    Transform2D {
        translation: na_to_vec(iso.translation.vector),
        rotation: iso.rotation.angle(),
        scale: Vec2::ONE,
    }
}

pub(crate) fn rb_to_handle(h: rp::RigidBodyHandle) -> RigidBodyHandle {
    let (index, generation) = h.into_raw_parts();
    RigidBodyHandle { index, generation }
}

pub(crate) fn handle_to_rb(h: RigidBodyHandle) -> rp::RigidBodyHandle {
    rp::RigidBodyHandle::from_raw_parts(h.index, h.generation)
}

pub(crate) fn col_to_handle(h: rp::ColliderHandle) -> ColliderHandle {
    let (index, generation) = h.into_raw_parts();
    ColliderHandle { index, generation }
}

pub(crate) fn handle_to_col(h: ColliderHandle) -> rp::ColliderHandle {
    rp::ColliderHandle::from_raw_parts(h.index, h.generation)
}

pub(crate) fn joint_to_handle(h: rp::ImpulseJointHandle) -> JointHandle {
    let (index, generation) = h.into_raw_parts();
    JointHandle { index, generation }
}

pub(crate) fn handle_to_joint(h: JointHandle) -> rp::ImpulseJointHandle {
    rp::ImpulseJointHandle::from_raw_parts(h.index, h.generation)
}
