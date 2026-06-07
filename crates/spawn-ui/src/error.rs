//! Error and result types for the UI layer.
//!
//! Fallible operations return [`UiResult`]. Invalid [`NodeId`](crate::NodeId)s
//! never panic: queries return `Option` and mutations return
//! [`UiError::InvalidNode`]. `context` is `&'static str` so error construction
//! never allocates.

use std::error::Error;
use std::fmt;

/// Errors produced by the UI layer.
///
/// `#[non_exhaustive]`: later phases may add variants, so downstream matches
/// must include a wildcard arm.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum UiError {
    /// A node handle was stale (freed slot or mismatched generation) or never
    /// existed in this tree.
    InvalidNode,
    /// `remove_node` targeted the root, which exists for the tree's lifetime.
    CannotRemoveRoot,
    /// An argument violated a precondition; `context` names the failing
    /// parameter or rule.
    InvalidArgument { context: &'static str },
    /// An operation was issued in a state that forbids it (e.g. reading cached
    /// layout while the tree is dirty); `context` names the violated invariant.
    InvalidState { context: &'static str },
}

impl fmt::Display for UiError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidNode => write!(f, "invalid node handle"),
            Self::CannotRemoveRoot => write!(f, "cannot remove the root node"),
            Self::InvalidArgument { context } => write!(f, "invalid argument: {context}"),
            Self::InvalidState { context } => write!(f, "invalid state: {context}"),
        }
    }
}

impl Error for UiError {}

/// Result alias for fallible UI operations.
pub type UiResult<T> = Result<T, UiError>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_non_empty_for_every_variant() {
        let variants = [
            UiError::InvalidNode,
            UiError::CannotRemoveRoot,
            UiError::InvalidArgument { context: "x" },
            UiError::InvalidState { context: "y" },
        ];
        for v in &variants {
            assert!(!v.to_string().is_empty());
        }
    }

    #[test]
    fn is_std_error() {
        let err = UiError::InvalidNode;
        let _: &dyn Error = &err;
        assert!(err.source().is_none());
    }
}
