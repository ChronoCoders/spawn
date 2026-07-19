//! Asset identity canonicalization and FNV-1a 64-bit content hashing.

use std::io::Read;
use std::path::Path;

use crate::error::{BuildError, BuildResult};

/// FNV-1a 64-bit offset basis. Fixed by the spec because `std` provides no
/// hasher guaranteed stable across runs/versions/platforms; pinning the
/// constants makes `content_hash` reproducible everywhere.
pub const FNV_OFFSET_BASIS_64: u64 = 0xcbf2_9ce4_8422_2325;
/// FNV-1a 64-bit prime (see [`FNV_OFFSET_BASIS_64`] for the stability rationale).
pub const FNV_PRIME_64: u64 = 0x0000_0100_0000_01b3;

/// Default streaming chunk size: large assets are hashed with bounded memory
/// rather than slurped whole.
pub const HASH_CHUNK_SIZE: usize = 64 * 1024;

/// Incremental FNV-1a 64-bit hasher.
///
/// The byte order and per-byte XOR-then-`wrapping_mul` are part of the stable
/// on-disk contract: identical bytes must always produce the same hash. This is
/// a non-cryptographic hash used only for change detection and content
/// addressing; it must never be relied upon against adversarial collisions.
#[derive(Debug, Clone)]
pub struct Fnv1a64 {
    state: u64,
}

impl Fnv1a64 {
    pub const fn new() -> Self {
        Self {
            state: FNV_OFFSET_BASIS_64,
        }
    }

    pub fn write(&mut self, bytes: &[u8]) {
        for byte in bytes {
            self.state ^= u64::from(*byte);
            self.state = self.state.wrapping_mul(FNV_PRIME_64);
        }
    }

    pub const fn finish(&self) -> u64 {
        self.state
    }
}

impl Default for Fnv1a64 {
    fn default() -> Self {
        Self::new()
    }
}

/// One-shot FNV-1a 64-bit hash over a byte slice.
pub fn hash_bytes(bytes: &[u8]) -> u64 {
    let mut hasher = Fnv1a64::new();
    hasher.write(bytes);
    hasher.finish()
}

/// Streaming FNV-1a 64-bit hash. Consumes `reader` in fixed-size reads through
/// the caller-provided `chunk` buffer, so memory use is bounded by `chunk.len()`
/// regardless of input size. Equivalent to [`hash_bytes`] over the same bytes.
pub fn hash_reader<R: Read>(reader: &mut R, chunk: &mut [u8]) -> BuildResult<u64> {
    let mut hasher = Fnv1a64::new();
    loop {
        let read = reader.read(chunk).map_err(|source| BuildError::Io {
            path: std::path::PathBuf::new(),
            source,
        })?;
        if read == 0 {
            break;
        }
        hasher.write(&chunk[..read]);
    }
    Ok(hasher.finish())
}

/// Canonical relative path string for `path` inside `source_root`.
///
/// Strips the `source_root` prefix, then delegates to spawn-asset's
/// [`spawn_asset::id::canonicalize`], the SAME function the runtime uses,
/// so a path baked here and a path loaded by spawn-asset yield the same
/// `AssetId` by construction, not by mirrored reimplementation. Returns
/// [`BuildError::PathEscapesRoot`] if the path lies outside `source_root` or
/// canonicalizes to a root-escaping (`..`-leading) form, which the build
/// rejects even though the runtime canonicalizer preserves it.
pub fn canonical_relative_path(source_root: &Path, path: &Path) -> BuildResult<String> {
    let relative = path
        .strip_prefix(source_root)
        .map_err(|_| BuildError::PathEscapesRoot {
            path: path.to_path_buf(),
        })?;
    let canonical = spawn_asset::id::canonicalize(&relative.to_string_lossy());
    if canonical == ".." || canonical.starts_with("../") {
        return Err(BuildError::PathEscapesRoot {
            path: path.to_path_buf(),
        });
    }
    Ok(canonical)
}

#[cfg(test)]
mod tests {
    use super::*;
    use spawn_asset::AssetId;
    use std::io::Cursor;
    use std::path::PathBuf;

    #[test]
    fn empty_string_is_offset_basis() {
        assert_eq!(hash_bytes(b""), FNV_OFFSET_BASIS_64);
    }

    #[test]
    fn known_answer_vectors() {
        assert_eq!(hash_bytes(b"a"), 0xaf63_dc4c_8601_ec8c);
        assert_eq!(hash_bytes(b"foobar"), 0x85944171f73967e8);
    }

    #[test]
    fn streaming_equals_one_shot() {
        let data = b"the quick brown fox jumps over the lazy dog, repeatedly!!!";
        let expected = hash_bytes(data);
        for chunk_size in [1usize, 7, HASH_CHUNK_SIZE] {
            let mut buf = vec![0u8; chunk_size];
            let mut cursor = Cursor::new(data.to_vec());
            let got = hash_reader(&mut cursor, &mut buf).unwrap();
            assert_eq!(got, expected, "chunk size {chunk_size}");
        }
    }

    #[test]
    fn canonicalization_collapses_dot_segments() {
        let root = PathBuf::from("/root");
        let a = canonical_relative_path(&root, &PathBuf::from("/root/a/./b.txt")).unwrap();
        let b = canonical_relative_path(&root, &PathBuf::from("/root/a/b.txt")).unwrap();
        assert_eq!(a, "a/b.txt");
        assert_eq!(a, b);
        assert_eq!(
            AssetId::from_canonical_path(&a),
            AssetId::from_canonical_path(&b)
        );
    }

    #[test]
    fn escaping_dotdot_is_error() {
        let root = PathBuf::from("/root");
        let err = canonical_relative_path(&root, &PathBuf::from("/root/../x.txt"));
        assert!(matches!(err, Err(BuildError::PathEscapesRoot { .. })));
    }

    #[test]
    fn out_of_root_prefix_is_error() {
        let root = PathBuf::from("/root");
        let err = canonical_relative_path(&root, &PathBuf::from("/other/x.txt"));
        assert!(matches!(err, Err(BuildError::PathEscapesRoot { .. })));
    }
}
