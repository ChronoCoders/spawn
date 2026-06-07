//! Engine configuration and the opaque script handle.

/// Runtime limits applied to the Lua VM.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ScriptConfig {
    /// Maximum VM instructions a single lifecycle call or load body may run
    /// before the budget hook aborts it with `BudgetExceeded`. `0` means
    /// unlimited and is unsafe for untrusted scripts.
    pub instruction_budget: u64,
    /// Maximum bytes the Lua VM may allocate, enforced via mlua's native memory
    /// limit. `0` means unlimited.
    pub memory_limit: usize,
}

impl Default for ScriptConfig {
    fn default() -> Self {
        Self {
            instruction_budget: 1_000_000,
            memory_limit: 64 * 1024 * 1024,
        }
    }
}

/// Opaque handle to a loaded script.
///
/// Generated from a monotonic per-engine counter; never reused within an engine
/// instance even across unload. A `ScriptId` from one engine must not be used
/// with another (not enforced).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ScriptId(u64);

impl ScriptId {
    pub(crate) const fn new(raw: u64) -> Self {
        Self(raw)
    }

    /// The underlying counter value; for debug/logging only.
    pub fn index(self) -> u64 {
        self.0
    }
}
