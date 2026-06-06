//! Error and result types for the input layer.
//!
//! Fallible operations return [`InputResult`]; no public API leaks the `gilrs`
//! backend error type. Backend initialization failures are mapped into
//! [`InputError::BackendInit`] at the boundary.

use std::error::Error;
use std::fmt;

use crate::context::ContextId;
use crate::state::GamepadId;

/// Errors produced by the input layer.
///
/// `#[non_exhaustive]`: later phases may add variants, so downstream matches
/// must include a wildcard arm. `context` is `&'static str` so error
/// construction never allocates.
#[derive(Debug)]
#[non_exhaustive]
pub enum InputError {
    /// The gamepad backend (`gilrs`) failed to initialize. `context` describes
    /// the failing boundary; the underlying backend error is not exposed.
    BackendInit { context: &'static str },
    /// A `replace`/`remove` targeted a binding that is not present.
    BindingNotFound,
    /// An operation referenced a gamepad id that is not currently connected.
    GamepadNotConnected(GamepadId),
    /// An operation referenced a context id that is not on the stack.
    ContextNotFound(ContextId),
}

impl fmt::Display for InputError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::BackendInit { context } => write!(f, "input backend init failed: {context}"),
            Self::BindingNotFound => write!(f, "binding not found"),
            Self::GamepadNotConnected(id) => write!(f, "gamepad not connected: {id:?}"),
            Self::ContextNotFound(id) => write!(f, "context not found: {id:?}"),
        }
    }
}

impl Error for InputError {}

/// Result alias for fallible input operations.
pub type InputResult<T> = Result<T, InputError>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_non_empty_for_every_variant() {
        let variants = [
            InputError::BackendInit { context: "x" },
            InputError::BindingNotFound,
            InputError::GamepadNotConnected(GamepadId::from_index(0)),
            InputError::ContextNotFound(ContextId(3)),
        ];
        for v in &variants {
            assert!(!v.to_string().is_empty());
        }
    }

    #[test]
    fn is_std_error() {
        let err = InputError::BindingNotFound;
        let _: &dyn Error = &err;
        assert!(err.source().is_none());
    }
}
