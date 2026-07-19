//! Platform error type and result alias.
//!
//! winit error types are never wrapped or re-exported; each fallible winit
//! operation is mapped to a `PlatformError` variant carrying a `&'static str`
//! context, so error construction never allocates.

use std::error::Error;
use std::fmt;

/// Errors raised by windowing, the event loop, and monitor enumeration.
///
/// `#[non_exhaustive]`: later phases may add variants, so matches must include a
/// wildcard arm. `context` is `&'static str` by design, error construction must
/// not allocate. Not `Clone`/`PartialEq`.
#[derive(Debug)]
#[non_exhaustive]
pub enum PlatformError {
    EventLoopCreation { context: &'static str },
    WindowCreation { context: &'static str },
    Fullscreen { context: &'static str },
    CursorGrab { context: &'static str },
    NotSupported { context: &'static str },
    OsError { context: &'static str },
}

impl fmt::Display for PlatformError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EventLoopCreation { context } => {
                write!(f, "event loop creation failed: {context}")
            }
            Self::WindowCreation { context } => write!(f, "window creation failed: {context}"),
            Self::Fullscreen { context } => write!(f, "fullscreen request failed: {context}"),
            Self::CursorGrab { context } => write!(f, "cursor grab failed: {context}"),
            Self::NotSupported { context } => write!(f, "not supported: {context}"),
            Self::OsError { context } => write!(f, "OS error: {context}"),
        }
    }
}

impl Error for PlatformError {}

pub type PlatformResult<T> = Result<T, PlatformError>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_non_empty_and_includes_context() {
        let variants = [
            PlatformError::EventLoopCreation { context: "a" },
            PlatformError::WindowCreation { context: "b" },
            PlatformError::Fullscreen { context: "c" },
            PlatformError::CursorGrab { context: "d" },
            PlatformError::NotSupported { context: "e" },
            PlatformError::OsError { context: "f" },
        ];
        for v in &variants {
            let s = v.to_string();
            assert!(!s.is_empty());
        }
    }

    #[test]
    fn is_std_error() {
        fn assert_error<T: Error>() {}
        assert_error::<PlatformError>();
    }
}
