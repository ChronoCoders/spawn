//! The `Script` ECS component and its per-entity lifecycle/state.

use spawn_ecs::Component;

use crate::config::ScriptId;

/// Per-entity lifecycle phase of a [`Script`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScriptLifecycle {
    /// Attached but `on_init` not yet run.
    Pending,
    /// `on_init` succeeded; `on_update` runs each frame.
    Active,
    /// A lifecycle call errored; the entity is skipped thereafter.
    Failed,
}

/// Opaque per-entity script state.
///
/// Carries the lifecycle flag. The persistent Lua `state` table is owned by the
/// [`ScriptEngine`](crate::ScriptEngine) keyed by `(script, entity)` (not by the
/// component, so the component stays `Send + Sync`); that table persists across
/// frames and is preserved across hot-reload. Opaque to gameplay code in
/// Phase 1; no serialization.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ScriptState {
    lifecycle: ScriptLifecycle,
}

impl ScriptState {
    /// The current per-entity lifecycle phase. Read-only introspection; the
    /// `state` table itself stays opaque in Phase 1.
    pub fn lifecycle(self) -> ScriptLifecycle {
        self.lifecycle
    }

    pub(crate) fn set_lifecycle(&mut self, lifecycle: ScriptLifecycle) {
        self.lifecycle = lifecycle;
    }
}

/// A script attached to an entity.
///
/// Attaching schedules `on_init` on the next runner pass; removal or entity
/// destruction schedules `on_destroy`.
pub struct Script {
    pub script: ScriptId,
    pub state: ScriptState,
}

impl Script {
    /// Constructs a `Script` referencing `script` with `lifecycle = Pending`.
    pub fn new(script: ScriptId) -> Self {
        Self {
            script,
            state: ScriptState {
                lifecycle: ScriptLifecycle::Pending,
            },
        }
    }
}

impl Component for Script {}
