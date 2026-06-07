//! Small boolean-vector helpers used by rotation/translation lock flags.
//!
//! These live in this crate (not spawn-core) for Phase 1; they exist only to
//! express per-axis degree-of-freedom locks.

/// Per-axis boolean vector (3D). A `true` component marks that axis as locked.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct BVec3 {
    pub x: bool,
    pub y: bool,
    pub z: bool,
}

/// Per-axis boolean vector (2D). A `true` component marks that axis as locked.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct BVec2 {
    pub x: bool,
    pub y: bool,
}

const _: () = assert!(std::mem::size_of::<BVec3>() == 3);
const _: () = assert!(std::mem::size_of::<BVec2>() == 2);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_and_repr() {
        assert_eq!(
            BVec3::default(),
            BVec3 {
                x: false,
                y: false,
                z: false,
            }
        );
        assert_eq!(BVec2::default(), BVec2 { x: false, y: false });
    }
}
