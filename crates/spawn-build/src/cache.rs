//! Line-based incremental build cache.

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::Path;

use spawn_asset::AssetId;

use crate::error::{BuildError, BuildResult};

/// One cached compile result.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CacheRecord {
    pub source_hash: u64,
    pub output_hash: u64,
}

/// Incremental build cache.
///
/// Invalidation rule: an asset is skipped on the next build iff its cached
/// `source_hash` equals the freshly discovered `content_hash` *and* the expected
/// output file still exists on disk (the on-disk check is enforced by the
/// pipeline). The cache file is sorted by `AssetId` so it is deterministic and
/// diffable.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct BuildCache {
    records: BTreeMap<u64, CacheRecord>,
}

impl BuildCache {
    /// Loads the cache. A non-existent file yields an empty cache (the first
    /// build is a full build, not a failure); other I/O errors surface.
    pub fn load(path: &Path) -> BuildResult<Self> {
        let text = match std::fs::read_to_string(path) {
            Ok(text) => text,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(Self::default()),
            Err(source) => {
                return Err(BuildError::Io {
                    path: path.to_path_buf(),
                    source,
                })
            }
        };
        let mut records = BTreeMap::new();
        for (idx, line) in text.lines().enumerate() {
            if line.trim().is_empty() {
                continue;
            }
            let line_no = idx + 1;
            let mut fields = line.split('\t');
            let parsed = (|| {
                let id = parse_hex_u64(fields.next()?)?;
                let source_hash = parse_hex_u64(fields.next()?)?;
                let output_hash = parse_hex_u64(fields.next()?)?;
                if fields.next().is_some() {
                    return None;
                }
                Some((
                    id,
                    CacheRecord {
                        source_hash,
                        output_hash,
                    },
                ))
            })();
            match parsed {
                Some((id, record)) => {
                    records.insert(id, record);
                }
                None => return Err(BuildError::CacheParse { line: line_no }),
            }
        }
        Ok(Self { records })
    }

    /// Writes the cache, one record per line, sorted by `AssetId`.
    pub fn save(&self, path: &Path) -> BuildResult<()> {
        let mut text = String::new();
        for (id, record) in &self.records {
            let _ = writeln!(
                text,
                "{id:016x}\t{:016x}\t{:016x}",
                record.source_hash, record.output_hash
            );
        }
        std::fs::write(path, text).map_err(|source| BuildError::Io {
            path: path.to_path_buf(),
            source,
        })
    }

    pub fn lookup(&self, id: AssetId) -> Option<CacheRecord> {
        self.records.get(&id.raw()).copied()
    }

    pub fn record(&mut self, id: AssetId, record: CacheRecord) {
        self.records.insert(id.raw(), record);
    }
}

fn parse_hex_u64(field: &str) -> Option<u64> {
    if field.len() != 16 || !field.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    u64::from_str_radix(field, 16).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_file_is_empty_cache() {
        let path = std::env::temp_dir().join("spawn-build-cache-missing-xyz.cache");
        let _ = std::fs::remove_file(&path);
        let cache = BuildCache::load(&path).unwrap();
        assert_eq!(cache, BuildCache::default());
    }

    #[test]
    fn round_trip_sorted() {
        let dir = std::env::temp_dir();
        let path = dir.join(format!("spawn-build-cache-rt-{}.cache", std::process::id()));
        let mut cache = BuildCache::default();
        cache.record(
            AssetId::from_raw(0xff),
            CacheRecord {
                source_hash: 1,
                output_hash: 2,
            },
        );
        cache.record(
            AssetId::from_raw(0x01),
            CacheRecord {
                source_hash: 3,
                output_hash: 4,
            },
        );
        cache.save(&path).unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.starts_with("0000000000000001"));
        let loaded = BuildCache::load(&path).unwrap();
        assert_eq!(loaded, cache);
        std::fs::remove_file(&path).unwrap();
    }

    #[test]
    fn malformed_line_reports_line() {
        let dir = std::env::temp_dir();
        let path = dir.join(format!(
            "spawn-build-cache-bad-{}.cache",
            std::process::id()
        ));
        std::fs::write(
            &path,
            "0000000000000001\t0000000000000002\t0000000000000003\nbroken\n",
        )
        .unwrap();
        match BuildCache::load(&path) {
            Err(BuildError::CacheParse { line }) => assert_eq!(line, 2),
            other => panic!("expected CacheParse, got {other:?}"),
        }
        std::fs::remove_file(&path).unwrap();
    }
}
