//! Small shared helpers.

use spawn_ui::{NodeId, UiResult, UiTree};

/// Removes every child of `parent` (recycling the subtree), so a panel's content
/// can be rebuilt in place. A no-op for a leaf or stale node.
pub(crate) fn clear_children(tree: &mut UiTree, parent: NodeId) -> UiResult<()> {
    // Re-query each iteration so the immutable borrow of `tree` ends before the
    // `remove_node` mutation (removing a child also frees its subtree).
    while let Some(&child) = tree.children(parent).and_then(<[NodeId]>::first) {
        tree.remove_node(child)?;
    }
    Ok(())
}
