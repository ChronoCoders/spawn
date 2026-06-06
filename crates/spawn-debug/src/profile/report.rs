//! Per-frame profiling report.

use super::scope::ScopeNode;

/// A completed frame's profiling result. `root` is a synthetic node whose
/// children are the top-level scopes.
pub struct FrameReport {
    pub frame_index: u64,
    pub total: core::time::Duration,
    pub root: ScopeNode,
    pub malformed: bool,
}

impl FrameReport {
    /// Frames per second; `0.0` when `total` is zero.
    pub fn fps(&self) -> f32 {
        let s = self.total.as_secs_f32();
        if s <= 0.0 {
            0.0
        } else {
            1.0 / s
        }
    }

    /// Depth-first `(name, duration, call_count)` for tabular display. Excludes
    /// the synthetic root.
    pub fn flatten(&self) -> Vec<(&'static str, core::time::Duration, u32)> {
        let mut out = Vec::new();
        for child in &self.root.children {
            flatten_node(child, &mut out);
        }
        out
    }

    /// Top-`k` scopes by duration, descending (flattened, excludes root).
    pub fn hottest(&self, k: usize) -> Vec<(&'static str, core::time::Duration)> {
        let mut all: Vec<(&'static str, core::time::Duration)> = self
            .flatten()
            .into_iter()
            .map(|(name, dur, _)| (name, dur))
            .collect();
        all.sort_by_key(|entry| std::cmp::Reverse(entry.1));
        all.truncate(k);
        all
    }
}

fn flatten_node(node: &ScopeNode, out: &mut Vec<(&'static str, core::time::Duration, u32)>) {
    out.push((node.name, node.duration, node.call_count));
    for child in &node.children {
        flatten_node(child, out);
    }
}
