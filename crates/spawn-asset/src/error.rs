//! Asset error layer.
//!
//! Errors are `Clone` so a `Failed` slot can hand out copies of its retained
//! error and `ReloadEvent` consumers can inspect failures. `Io` stores a
//! `std::io::ErrorKind` (which is `Copy`) plus the path rather than the
//! non-`Clone` `std::io::Error`.

use std::fmt;

#[derive(Debug, Clone)]
#[non_exhaustive]
pub enum AssetError {
    NotFound {
        path: String,
    },
    Io {
        path: String,
        kind: std::io::ErrorKind,
    },
    NoLoader {
        extension: String,
    },
    DuplicateLoader {
        extension: String,
    },
    InvalidUtf8 {
        path: String,
        offset: usize,
    },
    Parse {
        path: String,
        detail: String,
    },
    IdCollision {
        path_a: String,
        path_b: String,
    },
    QueueClosed,
    WatcherInit {
        detail: String,
    },
}

impl fmt::Display for AssetError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotFound { path } => write!(f, "asset not found: {path}"),
            Self::Io { path, kind } => write!(f, "I/O error for {path}: {kind:?}"),
            Self::NoLoader { extension } => {
                write!(f, "no loader registered for extension '{extension}'")
            }
            Self::DuplicateLoader { extension } => {
                write!(f, "loader already registered for extension '{extension}'")
            }
            Self::InvalidUtf8 { path, offset } => {
                write!(f, "invalid UTF-8 in {path} at byte offset {offset}")
            }
            Self::Parse { path, detail } => write!(f, "parse error in {path}: {detail}"),
            Self::IdCollision { path_a, path_b } => {
                write!(f, "asset id collision between '{path_a}' and '{path_b}'")
            }
            Self::QueueClosed => write!(f, "asset load queue is closed"),
            Self::WatcherInit { detail } => write!(f, "hot-reload watcher init failed: {detail}"),
        }
    }
}

impl std::error::Error for AssetError {}

impl From<spawn_core::SpawnError> for AssetError {
    fn from(err: spawn_core::SpawnError) -> Self {
        match err {
            spawn_core::SpawnError::NotFound { context } => Self::NotFound {
                path: context.to_string(),
            },
            spawn_core::SpawnError::Io(e) => Self::Io {
                path: String::new(),
                kind: e.kind(),
            },
            spawn_core::SpawnError::Parse { context } => Self::Parse {
                path: String::new(),
                detail: context.to_string(),
            },
            other => Self::Parse {
                path: String::new(),
                detail: other.to_string(),
            },
        }
    }
}

impl From<AssetError> for spawn_core::SpawnError {
    fn from(err: AssetError) -> Self {
        match err {
            AssetError::NotFound { .. } => Self::NotFound {
                context: "asset not found",
            },
            AssetError::Io { kind, .. } => Self::Io(std::io::Error::from(kind)),
            AssetError::NoLoader { .. } => Self::Unsupported {
                context: "no asset loader for extension",
            },
            AssetError::InvalidUtf8 { .. } | AssetError::Parse { .. } => Self::Parse {
                context: "asset parse error",
            },
            AssetError::DuplicateLoader { .. } | AssetError::IdCollision { .. } => {
                Self::InvalidState {
                    context: "asset registration conflict",
                }
            }
            AssetError::QueueClosed | AssetError::WatcherInit { .. } => Self::InvalidState {
                context: "asset subsystem failure",
            },
        }
    }
}

pub type AssetResult<T> = Result<T, AssetError>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_non_empty_for_every_variant() {
        let variants = [
            AssetError::NotFound { path: "a".into() },
            AssetError::Io {
                path: "a".into(),
                kind: std::io::ErrorKind::NotFound,
            },
            AssetError::NoLoader {
                extension: "x".into(),
            },
            AssetError::DuplicateLoader {
                extension: "x".into(),
            },
            AssetError::InvalidUtf8 {
                path: "a".into(),
                offset: 3,
            },
            AssetError::Parse {
                path: "a".into(),
                detail: "d".into(),
            },
            AssetError::IdCollision {
                path_a: "a".into(),
                path_b: "b".into(),
            },
            AssetError::QueueClosed,
            AssetError::WatcherInit { detail: "d".into() },
        ];
        for v in &variants {
            assert!(!v.to_string().is_empty());
        }
    }

    #[test]
    fn roundtrip_into_spawn_error() {
        let e: spawn_core::SpawnError = AssetError::NotFound { path: "a".into() }.into();
        assert!(matches!(e, spawn_core::SpawnError::NotFound { .. }));
        let e: spawn_core::SpawnError = AssetError::Io {
            path: "a".into(),
            kind: std::io::ErrorKind::PermissionDenied,
        }
        .into();
        assert!(matches!(e, spawn_core::SpawnError::Io(_)));
    }

    #[test]
    fn from_spawn_error() {
        let e: AssetError = spawn_core::SpawnError::NotFound { context: "missing" }.into();
        assert!(matches!(e, AssetError::NotFound { .. }));
    }
}
