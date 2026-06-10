//! Editor-shell error type and result alias.
//!
//! Wraps the underlying subsystem errors so the editor never panics on a stale
//! id, a missing component, a degenerate camera, a reflection mismatch, or a
//! surface error. `&'static str` contexts keep construction allocation-free.

use std::error::Error;
use std::fmt;

use spawn_editor::EditorError;
use spawn_input::InputError;
use spawn_platform::PlatformError;
use spawn_render::RenderError;
use spawn_ui::UiError;

/// An editor-shell failure.
#[derive(Debug)]
#[non_exhaustive]
pub enum ShellError {
    /// An editor command/transaction/selection operation failed.
    Editor(EditorError),
    /// A rendering operation failed.
    Render(RenderError),
    /// A UI-tree operation failed.
    Ui(UiError),
    /// Window/platform setup failed.
    Platform(PlatformError),
    /// Input initialization failed.
    Input(InputError),
    /// An editor-internal invariant was violated (e.g. the renderer was used
    /// before the window existed).
    InvalidState {
        /// Failure-class context.
        context: &'static str,
    },
}

impl fmt::Display for ShellError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Editor(e) => write!(f, "editor error: {e}"),
            Self::Render(e) => write!(f, "render error: {e}"),
            Self::Ui(e) => write!(f, "ui error: {e}"),
            Self::Platform(e) => write!(f, "platform error: {e}"),
            Self::Input(e) => write!(f, "input error: {e}"),
            Self::InvalidState { context } => write!(f, "invalid editor state: {context}"),
        }
    }
}

impl Error for ShellError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Editor(e) => Some(e),
            Self::Render(e) => Some(e),
            Self::Ui(e) => Some(e),
            Self::Platform(e) => Some(e),
            Self::Input(e) => Some(e),
            Self::InvalidState { .. } => None,
        }
    }
}

impl From<EditorError> for ShellError {
    fn from(e: EditorError) -> Self {
        Self::Editor(e)
    }
}
impl From<RenderError> for ShellError {
    fn from(e: RenderError) -> Self {
        Self::Render(e)
    }
}
impl From<UiError> for ShellError {
    fn from(e: UiError) -> Self {
        Self::Ui(e)
    }
}
impl From<PlatformError> for ShellError {
    fn from(e: PlatformError) -> Self {
        Self::Platform(e)
    }
}
impl From<InputError> for ShellError {
    fn from(e: InputError) -> Self {
        Self::Input(e)
    }
}

/// Result alias for fallible editor-shell operations.
pub type ShellResult<T> = Result<T, ShellError>;
