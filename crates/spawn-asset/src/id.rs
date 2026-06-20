//! Stable content-independent asset identity.
//!
//! Identity is the FNV-1a 64-bit hash of the *canonical* asset path, not of the
//! file bytes, so the same logical path yields the same `AssetId` across runs
//! and machines. Canonicalization (separator unification, lexical `.`/`..`
//! resolution, root-relative form) is performed by [`canonicalize`].

const FNV_OFFSET_BASIS: u64 = 0xcbf2_9ce4_8422_2325;
const FNV_PRIME: u64 = 0x0000_0100_0000_01b3;

#[repr(transparent)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct AssetId(u64);

impl AssetId {
    /// Hashes a path the caller guarantees is already in canonical form (see
    /// [`canonicalize`]). Passing a non-canonical path yields a different id and
    /// breaks dedup; callers inside this crate always canonicalize first.
    pub fn from_canonical_path(path: &str) -> Self {
        let mut hash = FNV_OFFSET_BASIS;
        for byte in path.as_bytes() {
            hash ^= u64::from(*byte);
            hash = hash.wrapping_mul(FNV_PRIME);
        }
        Self(hash)
    }

    pub const fn from_raw(value: u64) -> Self {
        Self(value)
    }

    pub const fn raw(self) -> u64 {
        self.0
    }
}

/// Normalizes `path` into the crate's canonical form: backslashes become `/`,
/// repeated/trailing separators collapse, `.` segments drop, and `..` segments
/// pop the previous segment lexically (never touching the filesystem). Leading
/// `..` that would escape the root are preserved verbatim so distinct escaping
/// paths stay distinct. The result is the identity basis for [`AssetId`].
pub fn canonicalize(path: &str) -> String {
    let unified = path.replace('\\', "/");
    let mut segments: Vec<&str> = Vec::new();
    for segment in unified.split('/') {
        match segment {
            "" | "." => {}
            ".." => {
                if matches!(segments.last(), Some(&last) if last != "..") {
                    segments.pop();
                } else {
                    segments.push("..");
                }
            }
            other => segments.push(other),
        }
    }
    segments.join("/")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fnv_is_stable_and_deterministic() {
        assert_eq!(AssetId::from_canonical_path("").raw(), FNV_OFFSET_BASIS);
        let a = AssetId::from_canonical_path("a/b.txt");
        let b = AssetId::from_canonical_path("a/b.txt");
        assert_eq!(a, b);
        assert_ne!(a, AssetId::from_canonical_path("a/c.txt"));
    }

    #[test]
    fn raw_roundtrip() {
        let id = AssetId::from_raw(12345);
        assert_eq!(id.raw(), 12345);
    }

    #[test]
    fn canonicalize_resolves_dot_segments() {
        assert_eq!(canonicalize("a/./b.txt"), "a/b.txt");
        assert_eq!(canonicalize("a/b/../c.txt"), "a/c.txt");
        assert_eq!(canonicalize("a\\b.txt"), "a/b.txt");
        assert_eq!(canonicalize("a//b.txt"), "a/b.txt");
        assert_eq!(canonicalize("./a/b.txt"), "a/b.txt");
        assert_eq!(canonicalize("../a.txt"), "../a.txt");
    }

    #[test]
    fn equivalent_paths_share_id() {
        assert_eq!(
            AssetId::from_canonical_path(&canonicalize("a/./b.txt")),
            AssetId::from_canonical_path(&canonicalize("a/b.txt")),
        );
    }
}
