//! Frame profiler: scoped RAII timers, a hierarchical scope tree, frame reports,
//! rolling per-scope stats, and a history ring.
//!
//! Threading: Phase 1 is main-thread-only. `Profiler` is not `Sync`;
//! `profile_scope!` is valid only on the thread that owns the active profiler.
//! The active frame's scope state lives in a `thread_local!` so `profile_scope!`
//! finds it without raw pointers or `unsafe`. When no profiler is active on the
//! thread, `profile_scope!` is a no-op.

pub mod report;
pub mod scope;
pub mod stats;

pub use report::FrameReport;
pub use scope::{ScopeGuard, ScopeNode};
pub use stats::RollingStats;

use std::cell::RefCell;
use std::collections::HashMap;
use std::time::Instant;

/// Open a profiling scope bound to a local `ScopeGuard`. `name` must be a
/// `&'static str` (string literal or `&'static` expression) — no per-scope
/// allocation. The scope closes when the guard drops (RAII, LIFO). A no-op when
/// no `Profiler` has an active frame on the calling thread.
#[macro_export]
macro_rules! profile_scope {
    ($name:expr) => {
        let _spawn_scope_guard = $crate::profile::ScopeGuard::enter($name);
    };
}

/// Profiler configuration.
pub struct ProfilerConfig {
    pub history_len: usize,
    pub stats_window: usize,
}

impl Default for ProfilerConfig {
    fn default() -> Self {
        Self {
            history_len: 240,
            stats_window: 120,
        }
    }
}

/// A node in the in-progress frame's scope stack, before it is folded into a
/// `ScopeNode` tree. Children are merged by name on close.
struct LiveNode {
    name: &'static str,
    start: Instant,
    duration: core::time::Duration,
    call_count: u32,
    children: Vec<LiveNode>,
}

impl LiveNode {
    fn new(name: &'static str) -> Self {
        Self {
            name,
            start: Instant::now(),
            duration: core::time::Duration::ZERO,
            call_count: 1,
            children: Vec::new(),
        }
    }

    fn into_scope(self) -> ScopeNode {
        let mut node = ScopeNode::new(self.name);
        node.duration = self.duration;
        node.call_count = self.call_count;
        for child in self.children {
            merge_into(&mut node.children, child.into_scope());
        }
        node
    }
}

fn merge_into(children: &mut Vec<ScopeNode>, incoming: ScopeNode) {
    if let Some(existing) = children.iter_mut().find(|c| c.name == incoming.name) {
        existing.duration += incoming.duration;
        existing.call_count += incoming.call_count;
        for ic in incoming.children {
            merge_into(&mut existing.children, ic);
        }
    } else {
        children.push(incoming);
    }
}

/// The active frame's scope state, owned by the thread-local while a frame is
/// open. `stack` holds the path of currently-open nodes; `roots` the completed
/// top-level nodes.
struct FrameState {
    roots: Vec<LiveNode>,
    stack: Vec<LiveNode>,
}

thread_local! {
    static ACTIVE: RefCell<Option<FrameState>> = const { RefCell::new(None) };
}

/// Push a scope onto the active frame, returning whether a frame was active.
pub(crate) fn push_scope(name: &'static str) -> bool {
    ACTIVE.with(|a| {
        let mut slot = a.borrow_mut();
        match slot.as_mut() {
            Some(frame) => {
                frame.stack.push(LiveNode::new(name));
                true
            }
            None => false,
        }
    })
}

/// Pop the top scope (LIFO), recording its duration and attaching it to its
/// parent (or roots), merging same-name siblings.
pub(crate) fn pop_scope() {
    ACTIVE.with(|a| {
        let mut slot = a.borrow_mut();
        if let Some(frame) = slot.as_mut() {
            if let Some(mut node) = frame.stack.pop() {
                node.duration = node.start.elapsed();
                match frame.stack.last_mut() {
                    Some(parent) => merge_live(&mut parent.children, node),
                    None => merge_live(&mut frame.roots, node),
                }
            }
        }
    });
}

fn merge_live(children: &mut Vec<LiveNode>, incoming: LiveNode) {
    if let Some(existing) = children.iter_mut().find(|c| c.name == incoming.name) {
        existing.duration += incoming.duration;
        existing.call_count += incoming.call_count;
        for ic in incoming.children {
            merge_live(&mut existing.children, ic);
        }
    } else {
        children.push(incoming);
    }
}

/// Main-thread frame profiler. Not `Sync`.
pub struct Profiler {
    config: ProfilerConfig,
    frame_index: u64,
    frame_start: Instant,
    history: Vec<FrameReport>,
    stats: HashMap<&'static str, RollingStats>,
    _not_sync: core::marker::PhantomData<*const ()>,
}

impl Profiler {
    pub fn new(config: ProfilerConfig) -> Self {
        let history_len = config.history_len.max(1);
        Self {
            history: Vec::with_capacity(history_len),
            stats: HashMap::new(),
            config,
            frame_index: 0,
            frame_start: Instant::now(),
            _not_sync: core::marker::PhantomData,
        }
    }

    /// Start a new frame: installs the active scope state on this thread and
    /// records the frame start instant.
    pub fn begin_frame(&mut self) {
        self.frame_start = Instant::now();
        ACTIVE.with(|a| {
            *a.borrow_mut() = Some(FrameState {
                roots: Vec::new(),
                stack: Vec::new(),
            });
        });
    }

    /// Close the frame: any open scopes are closed LIFO and the report is flagged
    /// `malformed`. Folds the tree into rolling stats and pushes a `FrameReport`
    /// into the history ring.
    pub fn end_frame(&mut self) {
        let total = self.frame_start.elapsed();
        let frame = ACTIVE.with(|a| a.borrow_mut().take());
        let mut frame = match frame {
            Some(f) => f,
            None => return,
        };
        let malformed = !frame.stack.is_empty();
        // Close any unbalanced open scopes in LIFO order.
        while let Some(mut node) = frame.stack.pop() {
            node.duration = node.start.elapsed();
            match frame.stack.last_mut() {
                Some(parent) => merge_live(&mut parent.children, node),
                None => merge_live(&mut frame.roots, node),
            }
        }
        let mut root = ScopeNode::new("<root>");
        for live in frame.roots {
            merge_into(&mut root.children, live.into_scope());
        }

        // Fold per-scope inclusive durations into rolling stats.
        let window = self.config.stats_window;
        fold_stats(&root, &mut self.stats, window);

        let report = FrameReport {
            frame_index: self.frame_index,
            total,
            root,
            malformed,
        };
        self.push_report(report);
        self.frame_index += 1;
    }

    fn push_report(&mut self, report: FrameReport) {
        let cap = self.config.history_len.max(1);
        if self.history.len() == cap {
            self.history.remove(0);
        }
        self.history.push(report);
    }

    /// Monotonically increasing frame counter.
    pub fn frame_index(&self) -> u64 {
        self.frame_index
    }

    /// The most recent completed frame, if any.
    pub fn last_report(&self) -> Option<&FrameReport> {
        self.history.last()
    }

    /// Completed frames oldest-to-newest, up to `history_len`.
    pub fn history(&self) -> &[FrameReport] {
        &self.history
    }

    /// Rolling stats for a named scope.
    pub fn scope_stats(&self, name: &str) -> Option<&RollingStats> {
        self.stats.get(name)
    }

    /// Reset history and stats; the frame counter is preserved.
    pub fn clear(&mut self) {
        self.history.clear();
        self.stats.clear();
    }
}

fn fold_stats(root: &ScopeNode, stats: &mut HashMap<&'static str, RollingStats>, window: usize) {
    for child in &root.children {
        fold_node(child, stats, window);
    }
}

fn fold_node(node: &ScopeNode, stats: &mut HashMap<&'static str, RollingStats>, window: usize) {
    stats
        .entry(node.name)
        .or_insert_with(|| RollingStats::new(window))
        .record(node.duration);
    for child in &node.children {
        fold_node(child, stats, window);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nested_scopes_build_tree() {
        let mut p = Profiler::new(ProfilerConfig::default());
        p.begin_frame();
        {
            let _outer = ScopeGuard::enter("outer");
            {
                let _inner = ScopeGuard::enter("inner");
            }
        }
        p.end_frame();
        let report = p.last_report().expect("report");
        assert!(!report.malformed);
        assert_eq!(report.root.children.len(), 1);
        let outer = &report.root.children[0];
        assert_eq!(outer.name, "outer");
        assert_eq!(outer.call_count, 1);
        assert_eq!(outer.children.len(), 1);
        assert_eq!(outer.children[0].name, "inner");
    }

    #[test]
    fn same_name_siblings_merge() {
        let mut p = Profiler::new(ProfilerConfig::default());
        p.begin_frame();
        for _ in 0..3 {
            let _g = ScopeGuard::enter("task");
        }
        p.end_frame();
        let report = p.last_report().expect("report");
        assert_eq!(report.root.children.len(), 1);
        assert_eq!(report.root.children[0].name, "task");
        assert_eq!(report.root.children[0].call_count, 3);
    }

    #[test]
    fn unbalanced_scope_sets_malformed() {
        let mut p = Profiler::new(ProfilerConfig::default());
        p.begin_frame();
        // Leak a guard so it is not dropped before end_frame.
        let g = ScopeGuard::enter("leaky");
        std::mem::forget(g);
        p.end_frame();
        let report = p.last_report().expect("report");
        assert!(report.malformed);
        assert_eq!(report.root.children.len(), 1);
        assert_eq!(report.root.children[0].name, "leaky");
    }

    #[test]
    fn noop_when_no_profiler_active() {
        // No begin_frame: guard must be inert.
        let g = ScopeGuard::enter("orphan");
        drop(g);
        // Nothing to assert beyond no panic; ACTIVE stays None.
        ACTIVE.with(|a| assert!(a.borrow().is_none()));
    }

    #[test]
    fn history_and_frame_index() {
        let mut p = Profiler::new(ProfilerConfig {
            history_len: 2,
            stats_window: 4,
        });
        for _ in 0..3 {
            p.begin_frame();
            {
                let _g = ScopeGuard::enter("s");
            }
            p.end_frame();
        }
        assert_eq!(p.frame_index(), 3);
        assert_eq!(p.history().len(), 2);
        let st = p.scope_stats("s").expect("stats");
        assert_eq!(st.count(), 3);
    }

    #[test]
    fn clear_resets() {
        let mut p = Profiler::new(ProfilerConfig::default());
        p.begin_frame();
        {
            let _g = ScopeGuard::enter("s");
        }
        p.end_frame();
        p.clear();
        assert!(p.last_report().is_none());
        assert!(p.scope_stats("s").is_none());
        assert!(p.history().is_empty());
    }
}
