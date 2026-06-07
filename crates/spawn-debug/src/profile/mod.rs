//! Frame profiler: scoped RAII timers, a hierarchical scope tree, frame reports,
//! rolling per-scope stats, and a history ring.
//!
//! Threading: Phase 1 is main-thread-only. `Profiler` is not `Sync`;
//! `profile_scope!` is valid only on the thread that owns the active profiler.
//! The active frame's scope state lives in a `thread_local!` so `profile_scope!`
//! finds it without raw pointers or `unsafe`. When no profiler is active on the
//! thread, `profile_scope!` is a no-op.
//!
//! Allocation: node storage is pooled and reused across frames. `begin_frame`
//! draws cleared `ScopeNode` buffers from a free pool; `end_frame` returns
//! evicted history reports' buffers to that pool. After warm-up the steady-state
//! frame path performs no heap allocation.

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

/// A `ScopeNode` paired with the open-scope start instant, used while a node is
/// on the live stack. The node carries its (recyclable) `children` buffer.
struct OpenScope {
    node: ScopeNode,
    start: Instant,
}

/// The active frame's scope state, owned by the thread-local while a frame is
/// open. `stack` holds currently-open scopes; `roots` the completed top-level
/// nodes. Both Vecs and all node `children` buffers are reused across frames.
struct FrameState {
    roots: Vec<ScopeNode>,
    stack: Vec<OpenScope>,
    pool: NodePool,
}

/// A free list of `ScopeNode`s whose `children` buffers are retained for reuse,
/// plus spare `roots`/`stack` frame buffers. Recycling clears contents but keeps
/// capacity, so a warmed pool serves steady-state frames without allocating.
#[derive(Default)]
struct NodePool {
    free: Vec<ScopeNode>,
    roots_spare: Vec<Vec<ScopeNode>>,
    stack_spare: Vec<Vec<OpenScope>>,
}

impl NodePool {
    fn take_roots(&mut self) -> Vec<ScopeNode> {
        self.roots_spare.pop().unwrap_or_default()
    }

    fn take_stack(&mut self) -> Vec<OpenScope> {
        self.stack_spare.pop().unwrap_or_default()
    }

    fn recycle_roots(&mut self, mut buf: Vec<ScopeNode>) {
        while let Some(node) = buf.pop() {
            self.recycle(node);
        }
        self.roots_spare.push(buf);
    }

    fn recycle_stack(&mut self, mut buf: Vec<OpenScope>) {
        while let Some(open) = buf.pop() {
            self.recycle(open.node);
        }
        self.stack_spare.push(buf);
    }

    fn take(&mut self, name: &'static str) -> ScopeNode {
        match self.free.pop() {
            Some(mut node) => {
                node.name = name;
                node.duration = core::time::Duration::ZERO;
                node.call_count = 0;
                node.children.clear();
                node
            }
            None => ScopeNode::new(name),
        }
    }

    fn recycle(&mut self, mut node: ScopeNode) {
        while let Some(child) = node.children.pop() {
            self.recycle(child);
        }
        self.free.push(node);
    }
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
                let node = frame.pool.take(name);
                frame.stack.push(OpenScope {
                    node,
                    start: Instant::now(),
                });
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
            close_top(frame);
        }
    });
}

fn close_top(frame: &mut FrameState) {
    if let Some(open) = frame.stack.pop() {
        let mut node = open.node;
        node.duration = open.start.elapsed();
        node.call_count = node.call_count.saturating_add(1);
        match frame.stack.last_mut() {
            Some(parent) => merge_node(&mut parent.node.children, node, &mut frame.pool),
            None => merge_node(&mut frame.roots, node, &mut frame.pool),
        }
    }
}

/// Merge `incoming` into `children` by name: same-name siblings fold (durations
/// summed, call counts added, children merged recursively). The absorbed node's
/// buffers are recycled into `pool`.
fn merge_node(children: &mut Vec<ScopeNode>, mut incoming: ScopeNode, pool: &mut NodePool) {
    if let Some(pos) = children.iter().position(|c| c.name == incoming.name) {
        children[pos].duration += incoming.duration;
        children[pos].call_count = children[pos].call_count.saturating_add(incoming.call_count);
        // Drain incoming's children forward to preserve their observable order,
        // merging each into the surviving node; then recycle the emptied node.
        let mut grandchildren = core::mem::take(&mut incoming.children);
        for ic in grandchildren.drain(..) {
            let target = &mut children[pos].children;
            merge_node(target, ic, pool);
        }
        incoming.children = grandchildren;
        pool.recycle(incoming);
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
    pool: NodePool,
    stats: HashMap<&'static str, RollingStats>,
    _not_sync: core::marker::PhantomData<*const ()>,
}

impl Profiler {
    pub fn new(config: ProfilerConfig) -> Self {
        let history_len = config.history_len.max(1);
        Self {
            history: Vec::with_capacity(history_len),
            pool: NodePool::default(),
            stats: HashMap::new(),
            config,
            frame_index: 0,
            frame_start: Instant::now(),
            _not_sync: core::marker::PhantomData,
        }
    }

    /// Start a new frame: installs the active scope state on this thread and
    /// records the frame start instant. Reuses pooled node buffers.
    pub fn begin_frame(&mut self) {
        self.frame_start = Instant::now();
        let mut pool = std::mem::take(&mut self.pool);
        let roots = pool.take_roots();
        let stack = pool.take_stack();
        ACTIVE.with(|a| {
            *a.borrow_mut() = Some(FrameState { roots, stack, pool });
        });
    }

    /// Close the frame: any open scopes are closed LIFO and the report is flagged
    /// `malformed`. Folds the tree into rolling stats and pushes a `FrameReport`
    /// into the history ring, recycling any evicted report's node buffers.
    pub fn end_frame(&mut self) {
        let total = self.frame_start.elapsed();
        let frame = ACTIVE.with(|a| a.borrow_mut().take());
        let mut frame = match frame {
            Some(f) => f,
            None => return,
        };
        let malformed = !frame.stack.is_empty();
        // Close any unbalanced open scopes in LIFO order.
        while !frame.stack.is_empty() {
            close_top(&mut frame);
        }

        let FrameState {
            mut roots,
            stack,
            mut pool,
        } = frame;
        let mut root = pool.take("<root>");
        for child in roots.drain(..) {
            merge_node(&mut root.children, child, &mut pool);
        }

        // Reclaim the frame's buffers into the pool, then the pool itself.
        pool.recycle_roots(roots);
        pool.recycle_stack(stack);
        self.pool = pool;

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
        if self.history.len() < cap {
            self.history.push(report);
        } else {
            // Ring is full. Wrap around: rotate the oldest report to the tail,
            // recycle its node buffers into the pool, then overwrite that slot
            // with the new report. Observable order stays oldest->newest and the
            // backing buffer's capacity is never reallocated.
            self.history.rotate_left(1);
            if let Some(slot) = self.history.last_mut() {
                let evicted = std::mem::replace(slot, report);
                self.pool.recycle(evicted.root);
            }
        }
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

    /// Reset history and stats; the frame counter is preserved. Node buffers are
    /// recycled into the pool rather than freed.
    pub fn clear(&mut self) {
        while let Some(report) = self.history.pop() {
            self.pool.recycle(report.root);
        }
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
    fn top_level_order_preserved() {
        let mut p = Profiler::new(ProfilerConfig::default());
        p.begin_frame();
        for name in ["a", "b", "c"] {
            let _g = ScopeGuard::enter(name);
        }
        p.end_frame();
        let report = p.last_report().expect("report");
        let names: Vec<&str> = report.root.children.iter().map(|c| c.name).collect();
        assert_eq!(names, ["a", "b", "c"]);
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
    fn history_order_after_wraparound() {
        let mut p = Profiler::new(ProfilerConfig {
            history_len: 3,
            stats_window: 8,
        });
        for _ in 0..5 {
            p.begin_frame();
            p.end_frame();
        }
        let h = p.history();
        let indices: Vec<u64> = h.iter().map(|r| r.frame_index).collect();
        assert_eq!(indices, [2, 3, 4]);
        assert_eq!(p.last_report().expect("report").frame_index, 4);
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
