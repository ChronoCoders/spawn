//! Script error types. No `mlua::Error` is ever exposed; every Lua error that
//! crosses an internal boundary is classified into exactly one variant here.

use std::error::Error;
use std::fmt;

use spawn_core::SpawnError;

/// All failure modes of the scripting runtime.
///
/// Carries owned diagnostic strings (script names, tracebacks), so it is neither
/// `Clone` nor `PartialEq`. A faulting script is isolated, never panicking the
/// engine.
#[derive(Debug)]
#[non_exhaustive]
pub enum ScriptError {
    /// VM creation or one-time sandbox installation failed.
    Init { message: String },
    /// Compile/syntax error or a top-level runtime error during `load_script`.
    /// `line` is the attributed source line when mlua can supply one.
    Load {
        script: String,
        line: Option<u32>,
        message: String,
    },
    /// A lifecycle call raised an ordinary Lua runtime error; `traceback` is the
    /// captured Lua traceback.
    Runtime { script: String, traceback: String },
    /// The per-call instruction budget was exhausted (runaway script aborted).
    BudgetExceeded { script: String },
    /// The VM memory limit was hit during a call.
    MemoryExceeded { script: String },
    /// A value could not be converted across the Rust/Lua boundary.
    Conversion { context: &'static str },
    /// The `ScriptId` is not loaded in this engine.
    UnknownScript,
    /// The script is in the `Failed` state and is skipped until a successful
    /// reload.
    ScriptFailed { script: String },
    /// A registration or call argument was rejected (e.g. duplicate binding name).
    InvalidArgument { context: &'static str },
}

impl fmt::Display for ScriptError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Init { message } => write!(f, "script engine init failed: {message}"),
            Self::Load {
                script,
                line: Some(line),
                message,
            } => write!(f, "script '{script}' load error at line {line}: {message}"),
            Self::Load {
                script,
                line: None,
                message,
            } => write!(f, "script '{script}' load error: {message}"),
            Self::Runtime { script, traceback } => {
                write!(f, "script '{script}' runtime error: {traceback}")
            }
            Self::BudgetExceeded { script } => {
                write!(f, "script '{script}' exceeded its instruction budget")
            }
            Self::MemoryExceeded { script } => {
                write!(f, "script '{script}' exceeded its memory limit")
            }
            Self::Conversion { context } => write!(f, "script value conversion failed: {context}"),
            Self::UnknownScript => write!(f, "unknown script id"),
            Self::ScriptFailed { script } => {
                write!(
                    f,
                    "script '{script}' is in the failed state and was skipped"
                )
            }
            Self::InvalidArgument { context } => write!(f, "invalid script argument: {context}"),
        }
    }
}

impl Error for ScriptError {}

impl From<ScriptError> for SpawnError {
    fn from(err: ScriptError) -> Self {
        match err {
            ScriptError::Load { .. } | ScriptError::Conversion { .. } => SpawnError::Parse {
                context: "spawn-script",
            },
            ScriptError::InvalidArgument { context } => SpawnError::InvalidArgument { context },
            ScriptError::UnknownScript => SpawnError::NotFound {
                context: "spawn-script",
            },
            _ => SpawnError::InvalidState {
                context: "spawn-script",
            },
        }
    }
}

/// Result alias for the scripting runtime.
pub type ScriptResult<T> = Result<T, ScriptError>;
