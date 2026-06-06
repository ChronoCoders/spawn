//! Scope tree nodes and the RAII `ScopeGuard` backing `profile_scope!`.

/// A node in a frame's hierarchical scope tree. Repeated entries of the same
/// `name` under the same parent in one frame merge into a single node.
pub struct ScopeNode {
    pub name: &'static str,
    pub duration: core::time::Duration,
    pub call_count: u32,
    pub children: Vec<ScopeNode>,
}

impl ScopeNode {
    pub(crate) fn new(name: &'static str) -> Self {
        Self {
            name,
            duration: core::time::Duration::ZERO,
            call_count: 0,
            children: Vec::new(),
        }
    }
}

/// RAII guard returned by `profile_scope!`. Not `Send`/`Sync`: a scope is valid
/// only on the thread owning the active `Profiler`. On drop (LIFO) it records the
/// elapsed time into the current frame's scope tree.
pub struct ScopeGuard {
    active: bool,
    // `*const ()` would need unsafe; instead a guard simply holds whether a
    // profiler was current at construction and pops via the thread-local on drop.
    _not_send: core::marker::PhantomData<*const ()>,
}

impl ScopeGuard {
    /// Construct a guard, pushing `name` onto the current profiler's scope stack
    /// if one is current on this thread. A no-op guard when none is current.
    pub fn enter(name: &'static str) -> Self {
        let active = super::push_scope(name);
        Self {
            active,
            _not_send: core::marker::PhantomData,
        }
    }
}

impl Drop for ScopeGuard {
    fn drop(&mut self) {
        if self.active {
            super::pop_scope();
        }
    }
}
