//! Error layer for the build pipeline.

use std::fmt;
use std::path::PathBuf;

use spawn_asset::AssetId;
use spawn_core::SpawnError;

/// Build-pipeline error.
///
/// `Io` always carries the offending `path`; `ManifestParse`/`CacheParse`
/// always carry the 1-based source line. There is deliberately no
/// `From<std::io::Error>`: a bare I/O error without a path is disallowed by
/// design, so every fallible filesystem call attaches its path explicitly.
#[derive(Debug)]
#[non_exhaustive]
pub enum BuildError {
    Io {
        path: PathBuf,
        source: std::io::Error,
    },
    ManifestParse {
        line: usize,
        detail: &'static str,
    },
    ManifestMissingKey {
        key: &'static str,
    },
    InvalidPattern {
        pattern: String,
        detail: &'static str,
    },
    PathEscapesRoot {
        path: PathBuf,
    },
    DuplicateAssetId {
        id: AssetId,
        path_a: String,
        path_b: String,
    },
    CacheParse {
        line: usize,
    },
    HashMismatch {
        id: AssetId,
        expected: u64,
        actual: u64,
    },
    PackFormat {
        detail: String,
    },
    SourceRootMissing {
        path: PathBuf,
    },
    /// Hook for the future *strict* unknown-extension policy. Phase 1 is
    /// permissive (every matched file is a copyable asset), so the default
    /// pipeline never raises this; the variant exists now so a later policy
    /// flip reports through a stable error rather than a new breaking variant.
    UnknownExtension {
        extension: String,
    },
}

impl fmt::Display for BuildError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io { path, source } => {
                write!(f, "I/O error at {}: {source}", path.display())
            }
            Self::ManifestParse { line, detail } => {
                write!(f, "manifest parse error on line {line}: {detail}")
            }
            Self::ManifestMissingKey { key } => {
                write!(f, "manifest missing required key: {key}")
            }
            Self::InvalidPattern { pattern, detail } => {
                write!(f, "invalid pattern {pattern:?}: {detail}")
            }
            Self::PathEscapesRoot { path } => {
                write!(f, "path escapes source root: {}", path.display())
            }
            Self::DuplicateAssetId { id, path_a, path_b } => write!(
                f,
                "duplicate asset id {:#018x} for {path_a:?} and {path_b:?}",
                id.raw()
            ),
            Self::CacheParse { line } => {
                write!(f, "cache parse error on line {line}")
            }
            Self::HashMismatch {
                id,
                expected,
                actual,
            } => write!(
                f,
                "hash mismatch for asset {:#018x}: expected {expected:#018x}, got {actual:#018x}",
                id.raw()
            ),
            Self::PackFormat { detail } => write!(f, "pack format error: {detail}"),
            Self::SourceRootMissing { path } => {
                write!(
                    f,
                    "source root missing or not a directory: {}",
                    path.display()
                )
            }
            Self::UnknownExtension { extension } => {
                write!(f, "unknown asset extension: {extension}")
            }
        }
    }
}

impl std::error::Error for BuildError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io { source, .. } => Some(source),
            _ => None,
        }
    }
}

impl From<BuildError> for SpawnError {
    fn from(err: BuildError) -> Self {
        match err {
            BuildError::Io { source, .. } => SpawnError::Io(source),
            BuildError::ManifestParse { .. } | BuildError::CacheParse { .. } => SpawnError::Parse {
                context: "spawn-build parse",
            },
            BuildError::InvalidPattern { .. } => SpawnError::Parse {
                context: "spawn-build pattern",
            },
            BuildError::ManifestMissingKey { .. } => SpawnError::Parse {
                context: "spawn-build manifest key",
            },
            BuildError::PathEscapesRoot { .. } => SpawnError::Parse {
                context: "spawn-build path escapes root",
            },
            BuildError::DuplicateAssetId { .. } => SpawnError::InvalidState {
                context: "spawn-build duplicate asset id",
            },
            BuildError::HashMismatch { .. } => SpawnError::InvalidState {
                context: "spawn-build hash mismatch",
            },
            BuildError::PackFormat { .. } => SpawnError::Parse {
                context: "spawn-build pack format",
            },
            BuildError::SourceRootMissing { .. } => SpawnError::InvalidState {
                context: "spawn-build source root",
            },
            BuildError::UnknownExtension { .. } => SpawnError::Unsupported {
                context: "spawn-build unknown extension",
            },
        }
    }
}

/// Result alias for the build pipeline.
pub type BuildResult<T> = Result<T, BuildError>;
