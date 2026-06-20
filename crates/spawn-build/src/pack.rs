//! Byte-precise `index.spawnpack` writer/reader.

use std::io::{Read, Write};
use std::path::Path;

use spawn_asset::AssetId;

use crate::error::{BuildError, BuildResult};

/// Pack magic: ASCII `"SWPK"` written little-endian.
pub const PACK_MAGIC: u32 = 0x4B50_5753;
/// Pack format version.
pub const PACK_VERSION: u32 = 1;
/// `flags` bit 0: the asset is stored externally as `data/<id-hex>` (always set
/// in Phase 1, where each asset is a separate file at offset 0). All other bits
/// are reserved and zero.
pub const PACK_FLAG_EXTERNAL: u8 = 0x01;

const HEADER_LEN: usize = 16;

/// One pack-index entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PackEntry {
    pub id: AssetId,
    pub offset: u64,
    pub flags: u8,
    pub rel_path: String,
    pub content_hash: u64,
}

/// The full pack index. Entries are stored sorted by `id`.
///
/// Determinism guarantee: because entries are id-sorted, no padding is emitted,
/// and no timestamp or host path is written, two builds of identical inputs
/// produce a byte-identical `index.spawnpack`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PackIndex {
    pub entries: Vec<PackEntry>,
}

impl PackIndex {
    pub fn new(mut entries: Vec<PackEntry>) -> Self {
        entries.sort_by_key(|e| e.id.raw());
        Self { entries }
    }

    pub fn write(&self, path: &Path) -> BuildResult<()> {
        let mut buf = Vec::with_capacity(HEADER_LEN + self.entries.len() * 32);
        buf.extend_from_slice(&PACK_MAGIC.to_le_bytes());
        buf.extend_from_slice(&PACK_VERSION.to_le_bytes());
        buf.extend_from_slice(&(self.entries.len() as u32).to_le_bytes());
        buf.extend_from_slice(&0u32.to_le_bytes());
        for entry in &self.entries {
            buf.extend_from_slice(&entry.id.raw().to_le_bytes());
            buf.extend_from_slice(&entry.offset.to_le_bytes());
            buf.push(entry.flags);
            let path_bytes = entry.rel_path.as_bytes();
            buf.extend_from_slice(&(path_bytes.len() as u16).to_le_bytes());
            buf.extend_from_slice(path_bytes);
            buf.extend_from_slice(&entry.content_hash.to_le_bytes());
        }
        let mut file = std::fs::File::create(path).map_err(|source| BuildError::Io {
            path: path.to_path_buf(),
            source,
        })?;
        file.write_all(&buf).map_err(|source| BuildError::Io {
            path: path.to_path_buf(),
            source,
        })
    }

    pub fn read(path: &Path) -> BuildResult<Self> {
        let mut file = std::fs::File::open(path).map_err(|source| BuildError::Io {
            path: path.to_path_buf(),
            source,
        })?;
        let mut bytes = Vec::new();
        file.read_to_end(&mut bytes)
            .map_err(|source| BuildError::Io {
                path: path.to_path_buf(),
                source,
            })?;
        Self::parse(&bytes)
    }

    fn parse(bytes: &[u8]) -> BuildResult<Self> {
        let mut cursor = Cursor { bytes, pos: 0 };
        let magic = cursor.read_u32()?;
        if magic != PACK_MAGIC {
            return Err(BuildError::PackFormat {
                detail: format!("bad magic {magic:#010x}"),
            });
        }
        let version = cursor.read_u32()?;
        if version != PACK_VERSION {
            return Err(BuildError::PackFormat {
                detail: format!("unsupported version {version}"),
            });
        }
        let count = cursor.read_u32()? as usize;
        let reserved = cursor.read_u32()?;
        if reserved != 0 {
            return Err(BuildError::PackFormat {
                detail: format!("reserved field not zero: {reserved}"),
            });
        }
        let mut entries = Vec::with_capacity(count);
        for _ in 0..count {
            let id = AssetId::from_raw(cursor.read_u64()?);
            let offset = cursor.read_u64()?;
            let flags = cursor.read_u8()?;
            let path_len = cursor.read_u16()? as usize;
            let path_bytes = cursor.read_bytes(path_len)?;
            let rel_path =
                String::from_utf8(path_bytes.to_vec()).map_err(|_| BuildError::PackFormat {
                    detail: "path is not valid UTF-8".to_string(),
                })?;
            let content_hash = cursor.read_u64()?;
            entries.push(PackEntry {
                id,
                offset,
                flags,
                rel_path,
                content_hash,
            });
        }
        if cursor.pos != bytes.len() {
            return Err(BuildError::PackFormat {
                detail: "trailing bytes after last entry".to_string(),
            });
        }
        Ok(Self { entries })
    }
}

struct Cursor<'a> {
    bytes: &'a [u8],
    pos: usize,
}

impl Cursor<'_> {
    fn read_bytes(&mut self, len: usize) -> BuildResult<&[u8]> {
        let end = self
            .pos
            .checked_add(len)
            .ok_or_else(|| BuildError::PackFormat {
                detail: "length overflow".to_string(),
            })?;
        if end > self.bytes.len() {
            return Err(BuildError::PackFormat {
                detail: "truncated".to_string(),
            });
        }
        let slice = &self.bytes[self.pos..end];
        self.pos = end;
        Ok(slice)
    }

    fn read_u8(&mut self) -> BuildResult<u8> {
        Ok(self.read_bytes(1)?[0])
    }

    fn read_u16(&mut self) -> BuildResult<u16> {
        let b = self.read_bytes(2)?;
        Ok(u16::from_le_bytes([b[0], b[1]]))
    }

    fn read_u32(&mut self) -> BuildResult<u32> {
        let b = self.read_bytes(4)?;
        Ok(u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    }

    fn read_u64(&mut self) -> BuildResult<u64> {
        let b = self.read_bytes(8)?;
        Ok(u64::from_le_bytes([
            b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7],
        ]))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> PackIndex {
        PackIndex::new(vec![
            PackEntry {
                id: AssetId::from_raw(0x10),
                offset: 0,
                flags: PACK_FLAG_EXTERNAL,
                rel_path: "b/c.png".into(),
                content_hash: 0xdead_beef,
            },
            PackEntry {
                id: AssetId::from_raw(0x02),
                offset: 0,
                flags: PACK_FLAG_EXTERNAL,
                rel_path: "a.txt".into(),
                content_hash: 0x1234,
            },
        ])
    }

    #[test]
    fn new_sorts_by_id() {
        let idx = sample();
        assert_eq!(idx.entries[0].id.raw(), 0x02);
        assert_eq!(idx.entries[1].id.raw(), 0x10);
    }

    #[test]
    fn round_trip() {
        let dir = std::env::temp_dir();
        let path = dir.join(format!(
            "spawn-build-pack-rt-{}.spawnpack",
            std::process::id()
        ));
        let idx = sample();
        idx.write(&path).unwrap();
        let read = PackIndex::read(&path).unwrap();
        assert_eq!(read, idx);
        std::fs::remove_file(&path).unwrap();
    }

    #[test]
    fn header_layout() {
        let dir = std::env::temp_dir();
        let path = dir.join(format!(
            "spawn-build-pack-hdr-{}.spawnpack",
            std::process::id()
        ));
        sample().write(&path).unwrap();
        let bytes = std::fs::read(&path).unwrap();
        assert_eq!(&bytes[0..4], b"SWPK");
        assert_eq!(
            u32::from_le_bytes([bytes[4], bytes[5], bytes[6], bytes[7]]),
            1
        );
        assert_eq!(
            u32::from_le_bytes([bytes[8], bytes[9], bytes[10], bytes[11]]),
            2
        );
        assert_eq!(&bytes[12..16], &[0, 0, 0, 0]);
        std::fs::remove_file(&path).unwrap();
    }

    #[test]
    fn bad_magic_rejected() {
        let bytes = [0u8; 16];
        assert!(matches!(
            PackIndex::parse(&bytes),
            Err(BuildError::PackFormat { .. })
        ));
    }

    #[test]
    fn truncated_rejected() {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&PACK_MAGIC.to_le_bytes());
        bytes.extend_from_slice(&PACK_VERSION.to_le_bytes());
        bytes.extend_from_slice(&1u32.to_le_bytes());
        bytes.extend_from_slice(&0u32.to_le_bytes());
        bytes.extend_from_slice(&0u64.to_le_bytes());
        assert!(matches!(
            PackIndex::parse(&bytes),
            Err(BuildError::PackFormat { .. })
        ));
    }
}
