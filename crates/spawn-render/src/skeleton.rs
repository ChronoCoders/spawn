//! Skeletal skinning data: a flat, topologically-ordered joint array with parent
//! indices and inverse-bind matrices, plus the pure skin-matrix composition that
//! turns a local pose into the per-joint matrices uploaded to the GPU.
//!
//! This hierarchy is internal to the skeleton, it is **not** the ECS
//! `Parent`/`Children` hierarchy. The engine attaches a skinned-mesh *entity*
//! under a parent via the 3a hierarchy; a skeleton's *bones* are this array.

use spawn_core::{Mat4, Transform3D, Vec4};

use crate::error::{RenderError, RenderResult};

/// Parent index marking a root joint (no parent).
pub const ROOT_JOINT: u16 = 0xFFFF;

/// One bone: its parent (or [`ROOT_JOINT`]) and inverse-bind matrix (the inverse
/// of the joint's bind-pose model transform).
#[derive(Debug, Clone, Copy)]
pub struct Joint {
    pub parent: u16,
    pub inverse_bind: Mat4,
}

/// A per-joint skinning matrix in GPU layout (`skin[j] = global[j] *
/// inverse_bind[j]`, column-major). `#[repr(C)]` + `Pod`; matches a WGSL
/// `mat4x4<f32>` storage element (64 bytes, std430).
#[repr(C)]
#[derive(Debug, Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
pub struct GpuJoint {
    pub model: [[f32; 4]; 4],
}

const _: () = assert!(std::mem::size_of::<GpuJoint>() == 64);

impl GpuJoint {
    fn from_mat4(m: Mat4) -> Self {
        let c = |v: Vec4| [v.x, v.y, v.z, v.w];
        Self {
            model: [c(m.cols[0]), c(m.cols[1]), c(m.cols[2]), c(m.cols[3])],
        }
    }
}

/// A flat skeleton: joints in parent-before-child order. The hierarchy lives in
/// the parent indices.
#[derive(Debug, Clone)]
pub struct Skeleton {
    joints: Vec<Joint>,
}

impl Skeleton {
    /// Validates and builds a skeleton. `Err(SkeletonInvalid)` if it is empty, a
    /// parent index is out of range, or a joint's parent does not precede it (the
    /// array must be topologically ordered so composition is a single forward pass).
    pub fn new(joints: Vec<Joint>) -> RenderResult<Self> {
        if joints.is_empty() {
            return Err(RenderError::SkeletonInvalid {
                context: "skeleton has no joints",
            });
        }
        for (i, joint) in joints.iter().enumerate() {
            if joint.parent == ROOT_JOINT {
                continue;
            }
            let parent = joint.parent as usize;
            if parent >= joints.len() {
                return Err(RenderError::SkeletonInvalid {
                    context: "joint parent index out of range",
                });
            }
            if parent >= i {
                return Err(RenderError::SkeletonInvalid {
                    context: "skeleton is not topologically ordered (parent must precede child)",
                });
            }
        }
        Ok(Self { joints })
    }

    pub fn joint_count(&self) -> usize {
        self.joints.len()
    }

    pub fn joints(&self) -> &[Joint] {
        &self.joints
    }

    /// Composes the local `pose` (one [`Transform3D`] per joint, in skeleton
    /// order) into the per-joint skinning matrices. Each joint's global transform
    /// is its parent's global times its local (roots use the local directly), and
    /// its skin matrix is that global times its inverse-bind. The bind pose (each
    /// local equal to the joint's bind transform) yields identity skin matrices.
    /// `Err(AnimationInvalid)` if `pose.len()` does not match the joint count.
    pub fn skin_matrices(&self, pose: &[Transform3D]) -> RenderResult<Vec<GpuJoint>> {
        if pose.len() != self.joints.len() {
            return Err(RenderError::AnimationInvalid {
                context: "pose joint count does not match skeleton",
            });
        }
        let mut globals: Vec<Mat4> = Vec::with_capacity(self.joints.len());
        let mut skins: Vec<GpuJoint> = Vec::with_capacity(self.joints.len());
        for (i, joint) in self.joints.iter().enumerate() {
            let local = pose[i].to_mat4();
            let global = if joint.parent == ROOT_JOINT {
                local
            } else {
                globals[joint.parent as usize] * local
            };
            globals.push(global);
            skins.push(GpuJoint::from_mat4(global * joint.inverse_bind));
        }
        Ok(skins)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use spawn_core::{Quat, Vec3};

    fn approx_identity(m: &[[f32; 4]; 4]) -> bool {
        let id = [
            [1.0, 0.0, 0.0, 0.0],
            [0.0, 1.0, 0.0, 0.0],
            [0.0, 0.0, 1.0, 0.0],
            [0.0, 0.0, 0.0, 1.0],
        ];
        m.iter()
            .flatten()
            .zip(id.iter().flatten())
            .all(|(a, b)| (a - b).abs() < 1e-5)
    }

    fn bind_chain() -> (Skeleton, Vec<Transform3D>) {
        // Root at origin, child translated +x by 1, grandchild +x by 1 more.
        let binds = [
            Transform3D::from_translation(Vec3::new(0.0, 0.0, 0.0)),
            Transform3D::from_translation(Vec3::new(1.0, 0.0, 0.0)),
            Transform3D::from_translation(Vec3::new(2.0, 0.0, 0.0)),
        ];
        let joints = binds
            .iter()
            .enumerate()
            .map(|(i, b)| Joint {
                parent: if i == 0 { ROOT_JOINT } else { (i - 1) as u16 },
                inverse_bind: b.inverse().unwrap().to_mat4(),
            })
            .collect();
        // Local pose that reproduces the bind pose: each joint's local transform is
        // its bind relative to its parent's bind (here, +x by 1 for children).
        let pose = vec![
            Transform3D::from_translation(Vec3::new(0.0, 0.0, 0.0)),
            Transform3D::from_translation(Vec3::new(1.0, 0.0, 0.0)),
            Transform3D::from_translation(Vec3::new(1.0, 0.0, 0.0)),
        ];
        (Skeleton::new(joints).unwrap(), pose)
    }

    #[test]
    fn bind_pose_yields_identity_skin_matrices() {
        let (skel, pose) = bind_chain();
        let skins = skel.skin_matrices(&pose).unwrap();
        assert_eq!(skins.len(), 3);
        for s in &skins {
            assert!(approx_identity(&s.model), "bind pose must be identity skin");
        }
    }

    #[test]
    fn animated_joint_moves_its_subtree() {
        let (skel, mut pose) = bind_chain();
        // Rotate the root 90° about Z; the grandchild's skin should displace.
        pose[0].rotation = Quat::from_rotation_z(std::f32::consts::FRAC_PI_2);
        let skins = skel.skin_matrices(&pose).unwrap();
        assert!(
            !approx_identity(&skins[2].model),
            "rotating the root must move the grandchild off the bind pose"
        );
    }

    #[test]
    fn empty_skeleton_is_rejected() {
        assert!(matches!(
            Skeleton::new(Vec::new()),
            Err(RenderError::SkeletonInvalid { .. })
        ));
    }

    #[test]
    fn forward_parent_reference_is_rejected() {
        let joints = vec![
            Joint {
                parent: 1,
                inverse_bind: Mat4::IDENTITY,
            },
            Joint {
                parent: ROOT_JOINT,
                inverse_bind: Mat4::IDENTITY,
            },
        ];
        assert!(matches!(
            Skeleton::new(joints),
            Err(RenderError::SkeletonInvalid { .. })
        ));
    }

    #[test]
    fn out_of_range_parent_is_rejected() {
        let joints = vec![Joint {
            parent: 9,
            inverse_bind: Mat4::IDENTITY,
        }];
        assert!(matches!(
            Skeleton::new(joints),
            Err(RenderError::SkeletonInvalid { .. })
        ));
    }

    #[test]
    fn pose_length_mismatch_is_rejected() {
        let (skel, _) = bind_chain();
        assert!(matches!(
            skel.skin_matrices(&[Transform3D::IDENTITY]),
            Err(RenderError::AnimationInvalid { .. })
        ));
    }
}
