//! Error type for `spawn-debug` setup paths.
//!
//! `DebugError` surfaces only from explicit setup calls (`Logger::init`,
//! `FileSink::open`). Logging, profiling, and metrics hot paths are infallible
//! and never produce a `DebugError`.

use std::error::Error;
use std::fmt;

/// Errors returned by `spawn-debug` setup operations.
///
/// `#[non_exhaustive]`: later phases may add variants, so matches must include a
/// wildcard arm. Not `Clone`/`PartialEq` because `Io` wraps `std::io::Error`.
/// `context` is `&'static str`, so construction never allocates.
#[derive(Debug)]
#[non_exhaustive]
pub enum DebugError {
    AlreadyInitialized,
    NotInitialized,
    InvalidConfig { context: &'static str },
    Io(std::io::Error),
}

impl fmt::Display for DebugError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::AlreadyInitialized => write!(f, "logger already initialized"),
            Self::NotInitialized => write!(f, "logger not initialized"),
            Self::InvalidConfig { context } => write!(f, "invalid config: {context}"),
            Self::Io(err) => write!(f, "I/O error: {err}"),
        }
    }
}

impl Error for DebugError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Io(err) => Some(err),
            _ => None,
        }
    }
}

impl From<std::io::Error> for DebugError {
    fn from(err: std::io::Error) -> Self {
        Self::Io(err)
    }
}

/// Result alias for `spawn-debug` setup operations.
pub type DebugResult<T> = Result<T, DebugError>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_non_empty_for_every_variant() {
        let variants: [DebugError; 4] = [
            DebugError::AlreadyInitialized,
            DebugError::NotInitialized,
            DebugError::InvalidConfig { context: "c" },
            DebugError::Io(std::io::Error::other("x")),
        ];
        for v in variants {
            assert!(!format!("{v}").is_empty());
        }
    }

    #[test]
    fn io_source_present_others_absent() {
        let io = DebugError::Io(std::io::Error::other("x"));
        assert!(io.source().is_some());
        assert!(DebugError::AlreadyInitialized.source().is_none());
    }

    #[test]
    fn from_io() {
        let err: DebugError = std::io::Error::other("x").into();
        assert!(matches!(err, DebugError::Io(_)));
    }
}
