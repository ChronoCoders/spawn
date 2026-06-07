//! Phase 1 identity compile step.

use std::fs::File;
use std::io::{Read, Write};
use std::path::PathBuf;

use crate::discover::AssetEntry;
use crate::error::{BuildError, BuildResult};
use crate::hash::{hash_bytes, HASH_CHUNK_SIZE};
use crate::manifest::BuildManifest;

/// Result of compiling one asset.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompileOutput {
    pub output_path: PathBuf,
    pub output_hash: u64,
    pub output_len: u64,
}

/// Compiles one asset via the Phase 1 identity transform: source bytes are
/// copied verbatim to the content-addressed path `output_dir/data/<id-hex-16>`.
///
/// The `output_hash` is computed from the *written* bytes (re-read), not the
/// source, so when real compilers replace this identity copy in later phases no
/// other contract changes. The write is atomic: bytes go to a temporary sibling
/// and are renamed into place, so a crash never leaves a half-written
/// content-addressed file that a later cache hit would trust. Does not touch the
/// cache or report.
pub fn compile_asset(entry: &AssetEntry, manifest: &BuildManifest) -> BuildResult<CompileOutput> {
    let data_dir = manifest.output_dir.join("data");
    let id_hex = format!("{:016x}", entry.id.raw());
    let output_path = data_dir.join(&id_hex);
    let source_path = manifest.source_root.join(&entry.source_path);

    let bytes = read_all(&source_path)?;

    let tmp_path = data_dir.join(format!("{id_hex}.tmp"));
    {
        let mut tmp = File::create(&tmp_path).map_err(|source| BuildError::Io {
            path: tmp_path.clone(),
            source,
        })?;
        tmp.write_all(&bytes).map_err(|source| BuildError::Io {
            path: tmp_path.clone(),
            source,
        })?;
        tmp.sync_all().map_err(|source| BuildError::Io {
            path: tmp_path.clone(),
            source,
        })?;
    }
    std::fs::rename(&tmp_path, &output_path).map_err(|source| BuildError::Io {
        path: output_path.clone(),
        source,
    })?;

    let written = read_all(&output_path)?;
    let output_hash = hash_bytes(&written);
    let output_len = written.len() as u64;

    Ok(CompileOutput {
        output_path,
        output_hash,
        output_len,
    })
}

fn read_all(path: &std::path::Path) -> BuildResult<Vec<u8>> {
    let mut file = File::open(path).map_err(|source| BuildError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    let mut buf = Vec::new();
    let mut chunk = vec![0u8; HASH_CHUNK_SIZE];
    loop {
        let read = file.read(&mut chunk).map_err(|source| BuildError::Io {
            path: path.to_path_buf(),
            source,
        })?;
        if read == 0 {
            break;
        }
        buf.extend_from_slice(&chunk[..read]);
    }
    Ok(buf)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::glob::Pattern;
    use spawn_asset::AssetId;

    #[test]
    fn identity_copy_and_hash() {
        let root = std::env::temp_dir().join(format!(
            "spawn-build-compile-{}-{:?}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&root).unwrap();
        let out = root.join("out");
        std::fs::create_dir_all(out.join("data")).unwrap();
        std::fs::write(root.join("a.txt"), b"hello").unwrap();

        let manifest = BuildManifest {
            source_root: root.clone(),
            output_dir: out.clone(),
            include: vec![Pattern::compile("**").unwrap()],
            exclude: vec![],
        };
        let id = AssetId::from_canonical_path("a.txt");
        let entry = AssetEntry {
            id,
            source_path: "a.txt".into(),
            byte_len: 5,
            content_hash: hash_bytes(b"hello"),
        };
        let result = compile_asset(&entry, &manifest).unwrap();
        assert_eq!(result.output_len, 5);
        assert_eq!(result.output_hash, hash_bytes(b"hello"));
        assert_eq!(std::fs::read(&result.output_path).unwrap(), b"hello");
        assert_eq!(
            result.output_path,
            out.join("data").join(format!("{:016x}", id.raw()))
        );
        std::fs::remove_dir_all(&root).unwrap();
    }
}
