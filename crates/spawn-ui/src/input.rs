//! Pointer input bridging, hit testing, and event routing.

use spawn_core::Vec2;

use crate::error::{UiError, UiResult};
use crate::tree::{NodeId, UiTree};

/// Re-export so callers can bridge `spawn-input` button queries without naming
/// the platform crate.
pub use spawn_input::MouseButton;

/// Per-frame pointer state, populated by the caller from `spawn-input`.
///
/// spawn-ui never reads `spawn-input` directly; the caller bridges so the tree
/// stays renderer/window agnostic. `wheel` is the per-frame accumulated scroll.
#[derive(Debug, Clone, Copy)]
pub struct UiInputState {
    pub pointer: Vec2,
    pub primary_down: bool,
    pub secondary_down: bool,
    pub wheel: Vec2,
}

/// Logical pointer button after flattening the device button matrix.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PointerButton {
    Primary,
    Secondary,
}

/// UI events drained once per frame.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum UiEvent {
    PointerEnter(NodeId),
    PointerExit(NodeId),
    PointerDown { node: NodeId, button: PointerButton },
    PointerUp { node: NodeId, button: PointerButton },
    Click { node: NodeId, button: PointerButton },
    Wheel { node: NodeId, delta: Vec2 },
}

impl UiTree {
    /// Returns the topmost node containing `point`: depth-first in reverse child
    /// order, respecting `overflow_clip` (a point outside a clipping ancestor's
    /// content rect cannot hit that ancestor's descendants). `Display::None`
    /// subtrees are never hit. `None` if nothing is hit.
    pub fn hit_test(&self, point: Vec2) -> Option<NodeId> {
        let root = self.root();
        self.hit_node(root, point)
    }

    fn hit_node(&self, node: NodeId, point: Vec2) -> Option<NodeId> {
        if !self.is_displayed(node) {
            return None;
        }
        let n = self.resolve(node)?;
        let rect = n.rect?;
        let clip = n.style.overflow_clip;
        if clip {
            match n.content_rect {
                Some(cr) if cr.contains_point(point) => {}
                _ => {
                    // Outside this clipping node's content rect: descendants are
                    // unreachable, but the node itself may still be hit.
                    return if rect.contains_point(point) {
                        Some(node)
                    } else {
                        None
                    };
                }
            }
        }
        let children = n.children.clone();
        for c in children.iter().rev() {
            if let Some(hit) = self.hit_node(*c, point) {
                return Some(hit);
            }
        }
        if rect.contains_point(point) {
            Some(node)
        } else {
            None
        }
    }

    /// Hover state: the topmost hit node from the last `update_input`.
    pub fn hovered(&self) -> Option<NodeId> {
        self.hovered
    }

    /// The node holding the active (pressed) state for `button`, if any.
    pub fn active(&self, button: PointerButton) -> Option<NodeId> {
        match button {
            PointerButton::Primary => self.active_primary,
            PointerButton::Secondary => self.active_secondary,
        }
    }

    /// Runs hit testing for the frame, diffs hover/active, and enqueues events.
    ///
    /// Must run after a successful `compute_layout`; `Err(InvalidState)` if the
    /// tree is dirty or layout has never been computed.
    pub fn update_input(&mut self, input: &UiInputState) -> UiResult<()> {
        if self.layout_dirty {
            return Err(UiError::InvalidState {
                context: "update_input requires up-to-date layout",
            });
        }

        let hit = self.hit_test(input.pointer);

        // Hover diff: exit before enter, topmost only.
        if hit != self.hovered {
            if let Some(prev) = self.hovered {
                self.events.push(UiEvent::PointerExit(prev));
            }
            if let Some(next) = hit {
                self.events.push(UiEvent::PointerEnter(next));
            }
            self.hovered = hit;
        }

        self.button_transition(PointerButton::Primary, input.primary_down, hit, |t| {
            &mut t.active_primary
        });
        self.button_transition(PointerButton::Secondary, input.secondary_down, hit, |t| {
            &mut t.active_secondary
        });

        if input.wheel != Vec2::ZERO {
            if let Some(node) = hit {
                self.events.push(UiEvent::Wheel {
                    node,
                    delta: input.wheel,
                });
            }
        }

        Ok(())
    }

    fn button_transition(
        &mut self,
        button: PointerButton,
        down: bool,
        hit: Option<NodeId>,
        slot: impl Fn(&mut UiTree) -> &mut Option<NodeId>,
    ) {
        let prev = *slot(self);
        match (prev, down) {
            (None, true) => {
                if let Some(node) = hit {
                    self.events.push(UiEvent::PointerDown { node, button });
                    *slot(self) = Some(node);
                }
            }
            (Some(active_node), false) => {
                if let Some(node) = hit {
                    self.events.push(UiEvent::PointerUp { node, button });
                    if node == active_node {
                        self.events.push(UiEvent::Click { node, button });
                    }
                }
                *slot(self) = None;
            }
            _ => {}
        }
    }

    /// Appends queued events to `out` and empties the internal queue.
    pub fn drain_events(&mut self, out: &mut Vec<UiEvent>) -> UiResult<()> {
        out.append(&mut self.events);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layout::TextMeasure;
    use crate::style::{AlignItems, Dimension, Size, Style};

    struct ZeroMeasure;
    impl TextMeasure for ZeroMeasure {
        fn measure(&mut self, _t: &str, _w: Option<f32>) -> Vec2 {
            Vec2::ZERO
        }
    }

    fn px(w: f32, h: f32) -> Size {
        Size {
            width: Dimension::Px(w),
            height: Dimension::Px(h),
        }
    }

    fn laid_out() -> (UiTree, NodeId, NodeId) {
        let mut tree = UiTree::new(Style {
            size: px(200.0, 200.0),
            ..Default::default()
        });
        let root = tree.root();
        let a = tree
            .create_node(
                Style {
                    size: px(100.0, 100.0),
                    align_items: AlignItems::Start,
                    ..Default::default()
                },
                root,
            )
            .unwrap();
        let b = tree
            .create_node(
                Style {
                    size: px(100.0, 100.0),
                    align_items: AlignItems::Start,
                    ..Default::default()
                },
                root,
            )
            .unwrap();
        let mut m = ZeroMeasure;
        tree.compute_layout(Vec2::new(200.0, 200.0), &mut m)
            .unwrap();
        (tree, a, b)
    }

    fn frame(pointer: Vec2, primary: bool) -> UiInputState {
        UiInputState {
            pointer,
            primary_down: primary,
            secondary_down: false,
            wheel: Vec2::ZERO,
        }
    }

    #[test]
    fn hit_test_topmost_returns_last_child() {
        // Two siblings overlap on the main axis via negative-margin layering:
        // both occupy the same region; the later child (drawn on top) wins.
        let mut tree = UiTree::new(Style {
            size: px(100.0, 100.0),
            ..Default::default()
        });
        let root = tree.root();
        let under = tree
            .create_node(
                Style {
                    size: px(100.0, 100.0),
                    flex_shrink: 0.0,
                    align_items: AlignItems::Start,
                    ..Default::default()
                },
                root,
            )
            .unwrap();
        let over = tree
            .create_node(
                Style {
                    size: px(100.0, 100.0),
                    flex_shrink: 0.0,
                    margin: crate::style::Edges {
                        left: -100.0,
                        ..Default::default()
                    },
                    align_items: AlignItems::Start,
                    ..Default::default()
                },
                root,
            )
            .unwrap();
        let mut m = ZeroMeasure;
        tree.compute_layout(Vec2::new(100.0, 100.0), &mut m)
            .unwrap();
        // `over`'s negative lead margin pulls it back over `under`; both cover
        // the same rect, so the topmost (last) child is returned.
        let _ = under;
        assert_eq!(tree.hit_test(Vec2::new(50.0, 50.0)), Some(over));
    }

    #[test]
    fn miss_returns_none() {
        let (tree, _a, _b) = laid_out();
        assert_eq!(tree.hit_test(Vec2::new(500.0, 500.0)), None);
    }

    #[test]
    fn clip_excludes_descendants_outside() {
        let mut tree = UiTree::new(Style {
            size: px(200.0, 200.0),
            ..Default::default()
        });
        let root = tree.root();
        let clipper = tree
            .create_node(
                Style {
                    size: px(50.0, 50.0),
                    overflow_clip: true,
                    align_items: AlignItems::Start,
                    ..Default::default()
                },
                root,
            )
            .unwrap();
        let child = tree
            .create_node(
                Style {
                    size: px(100.0, 100.0),
                    align_items: AlignItems::Start,
                    ..Default::default()
                },
                clipper,
            )
            .unwrap();
        let mut m = ZeroMeasure;
        tree.compute_layout(Vec2::new(200.0, 200.0), &mut m)
            .unwrap();
        // child extends to 100x100 but clipper content is 50x50.
        assert_eq!(tree.hit_test(Vec2::new(10.0, 10.0)), Some(child));
        // Point inside child but outside clipper -> not child; clipper not there
        // either (outside its rect) -> root.
        assert_eq!(tree.hit_test(Vec2::new(70.0, 70.0)), Some(root));
    }

    #[test]
    fn enter_exit_ordering() {
        let (mut tree, a, b) = laid_out();
        let mut out = Vec::new();

        tree.update_input(&frame(Vec2::new(10.0, 10.0), false))
            .unwrap();
        tree.drain_events(&mut out).unwrap();
        assert_eq!(out, vec![UiEvent::PointerEnter(a)]);
        assert_eq!(tree.hovered(), Some(a));

        out.clear();
        tree.update_input(&frame(Vec2::new(110.0, 10.0), false))
            .unwrap();
        tree.drain_events(&mut out).unwrap();
        assert_eq!(out, vec![UiEvent::PointerExit(a), UiEvent::PointerEnter(b)]);
    }

    #[test]
    fn click_press_release_same_target() {
        let (mut tree, a, _b) = laid_out();
        let mut out = Vec::new();
        tree.update_input(&frame(Vec2::new(10.0, 10.0), false))
            .unwrap();
        tree.update_input(&frame(Vec2::new(10.0, 10.0), true))
            .unwrap();
        tree.update_input(&frame(Vec2::new(10.0, 10.0), false))
            .unwrap();
        tree.drain_events(&mut out).unwrap();
        assert!(out.contains(&UiEvent::PointerDown {
            node: a,
            button: PointerButton::Primary
        }));
        assert!(out.contains(&UiEvent::PointerUp {
            node: a,
            button: PointerButton::Primary
        }));
        assert!(out.contains(&UiEvent::Click {
            node: a,
            button: PointerButton::Primary
        }));
    }

    #[test]
    fn down_a_up_b_no_click() {
        let (mut tree, a, b) = laid_out();
        let mut out = Vec::new();
        tree.update_input(&frame(Vec2::new(10.0, 10.0), false))
            .unwrap();
        tree.update_input(&frame(Vec2::new(10.0, 10.0), true))
            .unwrap();
        tree.update_input(&frame(Vec2::new(110.0, 10.0), false))
            .unwrap();
        tree.drain_events(&mut out).unwrap();
        assert!(out.contains(&UiEvent::PointerDown {
            node: a,
            button: PointerButton::Primary
        }));
        assert!(out.contains(&UiEvent::PointerUp {
            node: b,
            button: PointerButton::Primary
        }));
        assert!(!out.iter().any(|e| matches!(e, UiEvent::Click { .. })));
    }

    #[test]
    fn wheel_routes_to_hovered() {
        let (mut tree, a, _b) = laid_out();
        let mut out = Vec::new();
        let input = UiInputState {
            pointer: Vec2::new(10.0, 10.0),
            primary_down: false,
            secondary_down: false,
            wheel: Vec2::new(0.0, -3.0),
        };
        tree.update_input(&input).unwrap();
        tree.drain_events(&mut out).unwrap();
        assert!(out.contains(&UiEvent::Wheel {
            node: a,
            delta: Vec2::new(0.0, -3.0)
        }));
    }

    #[test]
    fn drain_empties_queue() {
        let (mut tree, _a, _b) = laid_out();
        let mut out = Vec::new();
        tree.update_input(&frame(Vec2::new(10.0, 10.0), false))
            .unwrap();
        tree.drain_events(&mut out).unwrap();
        assert!(!out.is_empty());
        out.clear();
        tree.drain_events(&mut out).unwrap();
        assert!(out.is_empty());
    }

    #[test]
    fn update_input_requires_layout() {
        let mut tree = UiTree::new(Style::default());
        let input = frame(Vec2::ZERO, false);
        assert_eq!(
            tree.update_input(&input),
            Err(UiError::InvalidState {
                context: "update_input requires up-to-date layout"
            })
        );
    }
}
