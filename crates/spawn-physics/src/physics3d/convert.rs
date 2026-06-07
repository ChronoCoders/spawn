//! Private conversions between spawn-core 3D math and nalgebra/Rapier types.
//!
//! No type defined here, and no nalgebra/Rapier type, escapes the `physics3d`
//! module boundary.

use rapier3d::na::{Isometry3, Point3, Translation3, UnitQuaternion, Vector3};
use rapier3d::prelude as rp;
use spawn_core::{Quat, Transform3D, Vec3};

use crate::handles::{ColliderHandle, JointHandle, RigidBodyHandle};

pub(crate) fn vec_to_na(v: Vec3) -> Vector3<f32> {
    Vector3::new(v.x, v.y, v.z)
}

pub(crate) fn na_to_vec(v: Vector3<f32>) -> Vec3 {
    Vec3::new(v.x, v.y, v.z)
}

pub(crate) fn point_to_vec(p: Point3<f32>) -> Vec3 {
    Vec3::new(p.x, p.y, p.z)
}

pub(crate) fn quat_to_na(q: Quat) -> UnitQuaternion<f32> {
    let raw = rapier3d::na::Quaternion::new(q.w, q.x, q.y, q.z);
    UnitQuaternion::from_quaternion(raw)
}

pub(crate) fn na_to_quat(q: &UnitQuaternion<f32>) -> Quat {
    let c = q.coords;
    Quat::from_xyzw(c.x, c.y, c.z, c.w)
}

pub(crate) fn transform_to_iso(t: Transform3D) -> Isometry3<f32> {
    Isometry3::from_parts(
        Translation3::new(t.translation.x, t.translation.y, t.translation.z),
        quat_to_na(t.rotation),
    )
}

pub(crate) fn iso_to_transform(iso: &Isometry3<f32>) -> Transform3D {
    Transform3D {
        translation: na_to_vec(iso.translation.vector),
        rotation: na_to_quat(&iso.rotation),
        scale: Vec3::ONE,
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
