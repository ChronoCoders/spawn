//! Deterministic filesystem asset discovery.

use std::collections::HashMap;
use std::fs::File;
use std::path::{Path, PathBuf};

use spawn_asset::AssetId;

use crate::error::{BuildError, BuildResult};
use crate::hash::{canonical_relative_path, hash_reader, HASH_CHUNK_SIZE};
use crate::manifest::BuildManifest;

/// A discovered source asset.
///
/// `id` is path-derived and content-independent (editing bytes changes only
/// `content_hash`, never `id`). `source_path` is canonical, relative to
/// `source_root`, forward-slash separated.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AssetEntry {
    pub id: AssetId,
    pub source_path: String,
    pub byte_len: u64,
    pub content_hash: u64,
}

/// Discovers all assets selected by `manifest`.
///
/// Recursively walks `source_root` via `std::fs::read_dir`, skipping the
/// `assets.manifest` file, the `output_dir` (if nested), and symlinks (not
/// followed, to keep the walk acyclic and deterministic). A file is included iff
/// it matches at least one `include` pattern and no `exclude` pattern (exclude
/// wins). The returned vector is sorted ascending by `id`; filesystem iteration
/// order is never observable. A duplicate `AssetId` is a hard error, never a
/// silent overwrite.
pub fn discover(manifest: &BuildManifest) -> BuildResult<Vec<AssetEntry>> {
    let source_root = &manifest.source_root;
    let manifest_file = source_root.join("assets.manifest");
    let output_dir = manifest.output_dir.canonicalize().ok();

    let mut files: Vec<PathBuf> = Vec::new();
    walk(
        source_root,
        &manifest_file,
        output_dir.as_deref(),
        &mut files,
    )?;

    let mut by_id: HashMap<AssetId, AssetEntry> = HashMap::new();
    let mut chunk = vec![0u8; HASH_CHUNK_SIZE];

    for path in files {
        let canonical = canonical_relative_path(source_root, &path)?;
        if !is_selected(manifest, &canonical) {
            continue;
        }
        let id = AssetId::from_canonical_path(&canonical);
        let metadata = std::fs::metadata(&path).map_err(|source| BuildError::Io {
            path: path.clone(),
            source,
        })?;
        let byte_len = metadata.len();
        let content_hash = {
            let mut file = File::open(&path).map_err(|source| BuildError::Io {
                path: path.clone(),
                source,
            })?;
            hash_reader(&mut file, &mut chunk).map_err(|err| with_path(err, &path))?
        };

        let entry = AssetEntry {
            id,
            source_path: canonical.clone(),
            byte_len,
            content_hash,
        };
        if let Some(existing) = by_id.insert(id, entry) {
            return Err(BuildError::DuplicateAssetId {
                id,
                path_a: existing.source_path,
                path_b: canonical,
            });
        }
    }

    let mut entries: Vec<AssetEntry> = by_id.into_values().collect();
    entries.sort_by_key(|e| e.id.raw());
    Ok(entries)
}

/// Attaches the offending path to an [`BuildError::Io`] raised by a helper that
/// had no path context of its own (e.g. [`hash_reader`]).
fn with_path(err: BuildError, path: &Path) -> BuildError {
    match err {
        BuildError::Io { source, .. } => BuildError::Io {
            path: path.to_path_buf(),
            source,
        },
        other => other,
    }
}

fn is_selected(manifest: &BuildManifest, canonical: &str) -> bool {
    let included = manifest.include.iter().any(|p| p.matches(canonical));
    if !included {
        return false;
    }
    !manifest.exclude.iter().any(|p| p.matches(canonical))
}

fn walk(
    dir: &Path,
    manifest_file: &Path,
    output_dir: Option<&Path>,
    out: &mut Vec<PathBuf>,
) -> BuildResult<()> {
    let read = std::fs::read_dir(dir).map_err(|source| BuildError::Io {
        path: dir.to_path_buf(),
        source,
    })?;
    for entry in read {
        let entry = entry.map_err(|source| BuildError::Io {
            path: dir.to_path_buf(),
            source,
        })?;
        let path = entry.path();
        let file_type = entry.file_type().map_err(|source| BuildError::Io {
            path: path.clone(),
            source,
        })?;
        if file_type.is_symlink() {
            continue;
        }
        if let Some(out_dir) = output_dir {
            if path.canonicalize().ok().as_deref() == Some(out_dir) {
                continue;
            }
        }
        if file_type.is_dir() {
            walk(&path, manifest_file, output_dir, out)?;
        } else if file_type.is_file() {
            if path == *manifest_file {
                continue;
            }
            out.push(path);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::glob::Pattern;

    fn manifest(root: &Path, out: &Path, include: &[&str], exclude: &[&str]) -> BuildManifest {
        BuildManifest {
            source_root: root.to_path_buf(),
            output_dir: out.to_path_buf(),
            include: include
                .iter()
                .map(|p| Pattern::compile(p).unwrap())
                .collect(),
            exclude: exclude
                .iter()
                .map(|p| Pattern::compile(p).unwrap())
                .collect(),
        }
    }

    struct TempTree {
        root: PathBuf,
    }
    impl TempTree {
        fn new(tag: &str) -> Self {
            let root = std::env::temp_dir().join(format!(
                "spawn-build-discover-{tag}-{}-{:?}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            ));
            std::fs::create_dir_all(&root).unwrap();
            Self { root }
        }
        fn file(&self, rel: &str, bytes: &[u8]) {
            let p = self.root.join(rel);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, bytes).unwrap();
        }
    }
    impl Drop for TempTree {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.root);
        }
    }

    #[test]
    fn discovers_sorted_and_filtered() {
        let tree = TempTree::new("filter");
        tree.file("z.png", b"z");
        tree.file("a/b.png", b"b");
        tree.file("a/skip.tmp", b"t");
        tree.file("notes.txt", b"n");
        let out = tree.root.join("out");
        let m = manifest(&tree.root, &out, &["**/*.png"], &["**/*.tmp"]);
        let entries = discover(&m).unwrap();
        let paths: Vec<&str> = entries.iter().map(|e| e.source_path.as_str()).collect();
        assert!(paths.contains(&"z.png"));
        assert!(paths.contains(&"a/b.png"));
        assert!(!paths.contains(&"notes.txt"));
        assert!(!paths.iter().any(|p| p.ends_with(".tmp")));
        // sorted by id
        let ids: Vec<u64> = entries.iter().map(|e| e.id.raw()).collect();
        let mut sorted = ids.clone();
        sorted.sort_unstable();
        assert_eq!(ids, sorted);
    }

    #[test]
    fn exclude_wins_over_include() {
        let tree = TempTree::new("excl");
        tree.file("keep.png", b"k");
        tree.file("drop.png", b"d");
        let out = tree.root.join("out");
        let m = manifest(&tree.root, &out, &["**"], &["drop.png"]);
        let entries = discover(&m).unwrap();
        let paths: Vec<&str> = entries.iter().map(|e| e.source_path.as_str()).collect();
        assert_eq!(paths, vec!["keep.png"]);
    }

    #[test]
    fn skips_manifest_file() {
        let tree = TempTree::new("manifest");
        tree.file("assets.manifest", b"output_dir = out\n");
        tree.file("a.png", b"a");
        let out = tree.root.join("out");
        let m = manifest(&tree.root, &out, &["**"], &[]);
        let entries = discover(&m).unwrap();
        let paths: Vec<&str> = entries.iter().map(|e| e.source_path.as_str()).collect();
        assert_eq!(paths, vec!["a.png"]);
    }
}
