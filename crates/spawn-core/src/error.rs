//! Error types shared across the engine.

use std::error::Error;
use std::fmt;

/// The shared engine error type.
///
/// `#[non_exhaustive]`: downstream crates gain variants in later phases, so
/// matches must include a wildcard arm. `context` is `&'static str` so error
/// construction never allocates.
#[derive(Debug)]
#[non_exhaustive]
pub enum SpawnError {
    InvalidArgument { context: &'static str },
    NotFound { context: &'static str },
    InvalidState { context: &'static str },
    Io(std::io::Error),
    Parse { context: &'static str },
    Unsupported { context: &'static str },
}

impl fmt::Display for SpawnError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidArgument { context } => write!(f, "invalid argument: {context}"),
            Self::NotFound { context } => write!(f, "not found: {context}"),
            Self::InvalidState { context } => write!(f, "invalid state: {context}"),
            Self::Io(err) => write!(f, "I/O error: {err}"),
            Self::Parse { context } => write!(f, "parse error: {context}"),
            Self::Unsupported { context } => write!(f, "unsupported: {context}"),
        }
    }
}

impl Error for SpawnError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Io(err) => Some(err),
            _ => None,
        }
    }
}

impl From<std::io::Error> for SpawnError {
    fn from(err: std::io::Error) -> Self {
        Self::Io(err)
    }
}

pub type SpawnResult<T> = Result<T, SpawnError>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_non_empty_for_every_variant() {
        let variants = [
            SpawnError::InvalidArgument { context: "a" },
            SpawnError::NotFound { context: "b" },
            SpawnError::InvalidState { context: "c" },
            SpawnError::Io(std::io::Error::other("x")),
            SpawnError::Parse { context: "d" },
            SpawnError::Unsupported { context: "e" },
        ];
        for v in &variants {
            assert!(!v.to_string().is_empty());
        }
    }

    #[test]
    fn source_some_for_io_none_otherwise() {
        let io = SpawnError::Io(std::io::Error::new(std::io::ErrorKind::NotFound, "x"));
        assert!(io.source().is_some());

        assert!(SpawnError::InvalidArgument { context: "a" }
            .source()
            .is_none());
        assert!(SpawnError::NotFound { context: "b" }.source().is_none());
        assert!(SpawnError::InvalidState { context: "c" }.source().is_none());
        assert!(SpawnError::Parse { context: "d" }.source().is_none());
        assert!(SpawnError::Unsupported { context: "e" }.source().is_none());
    }

    #[test]
    fn from_io_error() {
        let io = std::io::Error::new(std::io::ErrorKind::PermissionDenied, "denied");
        let err: SpawnError = io.into();
        assert!(matches!(err, SpawnError::Io(_)));
        assert!(err.source().is_some());
    }
}
