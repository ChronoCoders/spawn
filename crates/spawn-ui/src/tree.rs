//! Node identity, tree storage, hierarchy, and per-node content setters.

use spawn_core::{Rect, Vec2};

use crate::draw::{FontId, UiImage};
use crate::error::{UiError, UiResult};
use crate::style::{Display, Style};

/// Opaque, forge-proof handle to a node.
///
/// `index` selects a slot and `generation` guards against use-after-free: a
/// stale id whose generation differs from the live slot resolves to "invalid".
/// There is no null sentinel; ids are produced only by [`UiTree`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct NodeId {
    index: u32,
    generation: u32,
}

const _: () = assert!(std::mem::size_of::<NodeId>() == 8);

pub(crate) struct Node {
    pub(crate) style: Style,
    pub(crate) parent: Option<NodeId>,
    pub(crate) children: Vec<NodeId>,
    pub(crate) text: Option<String>,
    pub(crate) font: Option<FontId>,
    pub(crate) image: Option<UiImage>,
    pub(crate) dirty: bool,
    /// Cached root-space border-box rect; `None` until first layout or while
    /// excluded by `Display::None`.
    pub(crate) rect: Option<Rect>,
    /// Cached content rect (border box inset by padding), used for clipping.
    pub(crate) content_rect: Option<Rect>,
    /// Cached bottom-up intrinsic content size (padding included). Valid only
    /// while `dirty == false`; a dirty mark (which always propagates up to the
    /// root) invalidates it for this node and every ancestor.
    pub(crate) intrinsic: Option<Vec2>,
}

pub(crate) struct Slot {
    pub(crate) generation: u32,
    pub(crate) node: Option<Node>,
}

/// Retained tree of styled nodes with slotmap-style storage.
pub struct UiTree {
    pub(crate) slots: Vec<Slot>,
    pub(crate) free: Vec<u32>,
    pub(crate) root: NodeId,
    pub(crate) live: usize,
    /// `true` while any node is dirty or layout has never run.
    pub(crate) layout_dirty: bool,
    /// The `root_size` used by the last successful `compute_layout`.
    pub(crate) last_root_size: Option<Vec2>,
    pub(crate) hovered: Option<NodeId>,
    pub(crate) active_primary: Option<NodeId>,
    pub(crate) active_secondary: Option<NodeId>,
    pub(crate) events: Vec<crate::input::UiEvent>,
    /// Test-only instrumentation: number of `layout_node` arrangements performed
    /// by the most recent `compute_layout`. Proves incremental relayout skips
    /// clean subtrees (§6, §10).
    #[cfg(test)]
    pub(crate) layout_visits: u32,
}

impl UiTree {
    pub fn new(root_style: Style) -> Self {
        let root = NodeId {
            index: 0,
            generation: 0,
        };
        let node = Node {
            style: root_style,
            parent: None,
            children: Vec::new(),
            text: None,
            font: None,
            image: None,
            dirty: true,
            rect: None,
            content_rect: None,
            intrinsic: None,
        };
        Self {
            slots: vec![Slot {
                generation: 0,
                node: Some(node),
            }],
            free: Vec::new(),
            root,
            live: 1,
            layout_dirty: true,
            last_root_size: None,
            hovered: None,
            active_primary: None,
            active_secondary: None,
            events: Vec::new(),
            #[cfg(test)]
            layout_visits: 0,
        }
    }

    /// The root, valid for the tree's lifetime and never removable.
    pub fn root(&self) -> NodeId {
        self.root
    }

    pub(crate) fn resolve(&self, node: NodeId) -> Option<&Node> {
        let slot = self.slots.get(node.index as usize)?;
        if slot.generation != node.generation {
            return None;
        }
        slot.node.as_ref()
    }

    pub(crate) fn resolve_mut(&mut self, node: NodeId) -> Option<&mut Node> {
        let slot = self.slots.get_mut(node.index as usize)?;
        if slot.generation != node.generation {
            return None;
        }
        slot.node.as_mut()
    }

    /// `true` if `node` refers to a live slot with a matching generation.
    pub fn contains(&self, node: NodeId) -> bool {
        self.resolve(node).is_some()
    }

    /// Live node count, including the root.
    pub fn node_count(&self) -> usize {
        self.live
    }

    pub fn style(&self, node: NodeId) -> Option<&Style> {
        self.resolve(node).map(|n| &n.style)
    }

    /// `None` for the root or a stale id.
    pub fn parent(&self, node: NodeId) -> Option<NodeId> {
        self.resolve(node).and_then(|n| n.parent)
    }

    /// Children in insertion (and z/hit) order; `None` if stale.
    pub fn children(&self, node: NodeId) -> Option<&[NodeId]> {
        self.resolve(node).map(|n| n.children.as_slice())
    }

    pub fn text(&self, node: NodeId) -> Option<&str> {
        self.resolve(node).and_then(|n| n.text.as_deref())
    }

    /// Appends a node as the last child of `parent`, marking the path dirty.
    pub fn create_node(&mut self, style: Style, parent: NodeId) -> UiResult<NodeId> {
        if self.resolve(parent).is_none() {
            return Err(UiError::InvalidNode);
        }
        let node = Node {
            style,
            parent: Some(parent),
            children: Vec::new(),
            text: None,
            font: None,
            image: None,
            dirty: true,
            rect: None,
            content_rect: None,
            intrinsic: None,
        };
        let id = match self.free.pop() {
            Some(index) => {
                let slot = &mut self.slots[index as usize];
                slot.node = Some(node);
                NodeId {
                    index,
                    generation: slot.generation,
                }
            }
            None => {
                let index = self.slots.len() as u32;
                self.slots.push(Slot {
                    generation: 0,
                    node: Some(node),
                });
                NodeId {
                    index,
                    generation: 0,
                }
            }
        };
        if let Some(p) = self.resolve_mut(parent) {
            p.children.push(id);
        }
        self.live += 1;
        self.mark_dirty(parent);
        Ok(id)
    }

    /// Removes `node` and its whole subtree, recycling slots (generations
    /// bumped). The former parent is marked dirty.
    pub fn remove_node(&mut self, node: NodeId) -> UiResult<()> {
        if node == self.root {
            return Err(UiError::CannotRemoveRoot);
        }
        let parent = match self.resolve(node) {
            Some(n) => n.parent,
            None => return Err(UiError::InvalidNode),
        };
        if let Some(p) = parent {
            if let Some(pn) = self.resolve_mut(p) {
                pn.children.retain(|c| *c != node);
            }
        }
        self.free_subtree(node);
        if let Some(p) = parent {
            self.mark_dirty(p);
        }
        Ok(())
    }

    fn free_subtree(&mut self, node: NodeId) {
        let children = match self.resolve(node) {
            Some(n) => n.children.clone(),
            None => return,
        };
        for c in children {
            self.free_subtree(c);
        }
        let slot = &mut self.slots[node.index as usize];
        slot.node = None;
        slot.generation = slot.generation.wrapping_add(1);
        self.free.push(node.index);
        self.live -= 1;
    }

    /// Replaces the style of `node` and marks its subtree (and the path to the
    /// root) dirty.
    pub fn set_style(&mut self, node: NodeId, style: Style) -> UiResult<()> {
        match self.resolve_mut(node) {
            Some(n) => {
                n.style = style;
                self.mark_dirty(node);
                Ok(())
            }
            None => Err(UiError::InvalidNode),
        }
    }

    /// Sets or clears measured-text content; marks the subtree dirty.
    pub fn set_text(&mut self, node: NodeId, text: Option<String>) -> UiResult<()> {
        match self.resolve_mut(node) {
            Some(n) => {
                n.text = text;
                self.mark_dirty(node);
                Ok(())
            }
            None => Err(UiError::InvalidNode),
        }
    }

    /// Sets or clears the node's image content (does not affect layout).
    pub fn set_image(&mut self, node: NodeId, image: Option<UiImage>) -> UiResult<()> {
        match self.resolve_mut(node) {
            Some(n) => {
                n.image = image;
                Ok(())
            }
            None => Err(UiError::InvalidNode),
        }
    }

    /// Sets the font used by this node's text layout box.
    pub fn set_font(&mut self, node: NodeId, font: FontId) -> UiResult<()> {
        match self.resolve_mut(node) {
            Some(n) => {
                n.font = Some(font);
                Ok(())
            }
            None => Err(UiError::InvalidNode),
        }
    }

    /// Reorders `parent`'s children. `order` must be a permutation of the
    /// current children, else `Err(InvalidArgument)`.
    pub fn set_children_order(&mut self, parent: NodeId, order: &[NodeId]) -> UiResult<()> {
        let current = match self.resolve(parent) {
            Some(n) => &n.children,
            None => return Err(UiError::InvalidNode),
        };
        if order.len() != current.len() {
            return Err(UiError::InvalidArgument {
                context: "order is not a permutation of children",
            });
        }
        for c in current {
            if !order.contains(c) {
                return Err(UiError::InvalidArgument {
                    context: "order is not a permutation of children",
                });
            }
        }
        for o in order {
            if !current.contains(o) {
                return Err(UiError::InvalidArgument {
                    context: "order is not a permutation of children",
                });
            }
        }
        if let Some(n) = self.resolve_mut(parent) {
            n.children.clear();
            n.children.extend_from_slice(order);
        }
        self.mark_dirty(parent);
        Ok(())
    }

    /// Marks `node`'s subtree dirty and propagates the dirty mark up to the
    /// root, since a descendant size change can resize ancestors' `Auto` boxes.
    pub(crate) fn mark_dirty(&mut self, node: NodeId) {
        self.layout_dirty = true;
        self.mark_subtree_dirty(node);
        let mut current = self.resolve(node).and_then(|n| n.parent);
        while let Some(p) = current {
            match self.resolve_mut(p) {
                Some(pn) => {
                    pn.dirty = true;
                    current = pn.parent;
                }
                None => break,
            }
        }
    }

    fn mark_subtree_dirty(&mut self, node: NodeId) {
        let children = match self.resolve_mut(node) {
            Some(n) => {
                n.dirty = true;
                n.children.clone()
            }
            None => return,
        };
        for c in children {
            self.mark_subtree_dirty(c);
        }
    }

    pub(crate) fn is_displayed(&self, node: NodeId) -> bool {
        self.resolve(node)
            .map(|n| n.style.display != Display::None)
            .unwrap_or(false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::style::Style;

    #[test]
    fn root_is_valid_and_unremovable() {
        let mut tree = UiTree::new(Style::default());
        let root = tree.root();
        assert!(tree.contains(root));
        assert_eq!(tree.parent(root), None);
        assert_eq!(tree.remove_node(root), Err(UiError::CannotRemoveRoot));
        assert_eq!(tree.node_count(), 1);
    }

    #[test]
    fn create_and_children_order() {
        let mut tree = UiTree::new(Style::default());
        let root = tree.root();
        let a = tree.create_node(Style::default(), root).unwrap();
        let b = tree.create_node(Style::default(), root).unwrap();
        assert_eq!(tree.children(root), Some([a, b].as_slice()));
        assert_eq!(tree.parent(a), Some(root));
        assert_eq!(tree.node_count(), 3);
    }

    #[test]
    fn stale_id_after_remove() {
        let mut tree = UiTree::new(Style::default());
        let root = tree.root();
        let a = tree.create_node(Style::default(), root).unwrap();
        let child = tree.create_node(Style::default(), a).unwrap();
        tree.remove_node(a).unwrap();
        assert!(!tree.contains(a));
        assert!(!tree.contains(child));
        assert_eq!(tree.style(a), None);
        assert_eq!(
            tree.set_style(a, Style::default()),
            Err(UiError::InvalidNode)
        );
        assert_eq!(tree.node_count(), 1);
    }

    #[test]
    fn slot_recycling_bumps_generation() {
        let mut tree = UiTree::new(Style::default());
        let root = tree.root();
        let a = tree.create_node(Style::default(), root).unwrap();
        tree.remove_node(a).unwrap();
        let b = tree.create_node(Style::default(), root).unwrap();
        // Same slot index reused, but the stale handle must not resolve.
        assert!(!tree.contains(a));
        assert!(tree.contains(b));
    }

    #[test]
    fn create_on_stale_parent_errors() {
        let mut tree = UiTree::new(Style::default());
        let root = tree.root();
        let a = tree.create_node(Style::default(), root).unwrap();
        tree.remove_node(a).unwrap();
        assert_eq!(
            tree.create_node(Style::default(), a),
            Err(UiError::InvalidNode)
        );
    }

    #[test]
    fn reorder_must_be_permutation() {
        let mut tree = UiTree::new(Style::default());
        let root = tree.root();
        let a = tree.create_node(Style::default(), root).unwrap();
        let b = tree.create_node(Style::default(), root).unwrap();
        assert!(tree.set_children_order(root, &[b, a]).is_ok());
        assert_eq!(tree.children(root), Some([b, a].as_slice()));
        assert!(tree.set_children_order(root, &[a]).is_err());
        assert!(tree.set_children_order(root, &[a, a]).is_err());
    }

    #[test]
    fn text_setter_roundtrip() {
        let mut tree = UiTree::new(Style::default());
        let root = tree.root();
        tree.set_text(root, Some("hi".to_string())).unwrap();
        assert_eq!(tree.text(root), Some("hi"));
        tree.set_text(root, None).unwrap();
        assert_eq!(tree.text(root), None);
    }
}
