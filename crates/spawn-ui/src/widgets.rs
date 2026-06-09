//! Minimum-viable widget compositions over the retained [`UiTree`].
//!
//! These are the widgets the editor needs — [`Panel`], [`Label`], [`Button`],
//! [`Checkbox`], [`DragValue`] — built as compositions over the Phase 1 tree, not
//! new tree primitives. A constructor creates styled nodes (and text/content);
//! the per-frame query helpers read a drained [`UiEvent`] slice or the tree's
//! retained hover/press state and allocate nothing. Widgets keep no state outside
//! the tree: the caller owns the model values and the returned [`NodeId`]s.
//!
//! Drawing is unchanged — widgets emit only the existing [`DrawCommand`] variants
//! ([`DrawCommand::Text`] is rendered by the spawn-render text path).
//!
//! [`DrawCommand`]: crate::draw::DrawCommand
//! [`DrawCommand::Text`]: crate::draw::DrawCommand::Text

// Each widget is a zero-sized namespace whose `new` builds a tree node and
// returns its `NodeId` (not `Self`) — the intended composition API, so the
// `new`-returns-`Self` convention does not apply here.
#![allow(clippy::new_ret_no_self)]

use spawn_core::Vec2;

use crate::draw::FontId;
use crate::error::{UiError, UiResult};
use crate::input::{PointerButton, UiEvent};
use crate::style::Style;
use crate::tree::{NodeId, UiTree};

/// `true` if `events` contains a primary [`UiEvent::Click`] on `node`.
fn primary_clicked(node: NodeId, events: &[UiEvent]) -> bool {
    events.iter().any(|event| {
        matches!(
            event,
            UiEvent::Click { node: n, button } if *n == node && *button == PointerButton::Primary
        )
    })
}

/// Formats a value for display in a [`DragValue`]. Fixed 3-decimal form is the
/// MVP; exact text entry is deferred.
fn format_value(value: f32) -> String {
    format!("{value:.3}")
}

/// A styled container node: background, optional border, flex layout for its
/// children, optional `overflow_clip` for scroll regions. The editor's docked
/// regions are panels.
pub struct Panel;

impl Panel {
    /// Creates a panel node under `parent` with `style`.
    pub fn new(tree: &mut UiTree, parent: NodeId, style: Style) -> UiResult<NodeId> {
        tree.create_node(style, parent)
    }
}

/// A text node: measured via the [`TextMeasure`](crate::layout::TextMeasure) hook
/// and drawn via the spawn-render text path.
pub struct Label;

impl Label {
    /// Creates a label node under `parent` showing `text` in `font`.
    pub fn new(
        tree: &mut UiTree,
        parent: NodeId,
        text: impl Into<String>,
        font: FontId,
        style: Style,
    ) -> UiResult<NodeId> {
        let id = tree.create_node(style, parent)?;
        tree.set_text(id, Some(text.into()))?;
        tree.set_font(id, font)?;
        Ok(id)
    }
}

/// An interactive labelled node. Click detection scans the drained event queue;
/// hover/press visuals read [`UiTree::hovered`]/[`UiTree::active`].
pub struct Button;

impl Button {
    /// Creates a button node under `parent` showing `text` in `font`.
    pub fn new(
        tree: &mut UiTree,
        parent: NodeId,
        text: impl Into<String>,
        font: FontId,
        style: Style,
    ) -> UiResult<NodeId> {
        let id = tree.create_node(style, parent)?;
        tree.set_text(id, Some(text.into()))?;
        tree.set_font(id, font)?;
        Ok(id)
    }

    /// Whether `node` was clicked (primary press and release on it) this frame,
    /// per the drained `events`. Allocation-free.
    pub fn clicked(node: NodeId, events: &[UiEvent]) -> bool {
        primary_clicked(node, events)
    }
}

/// A two-state toggle. The box outline is `style.border`; `style.background` is
/// the check fill, shown by forcing its alpha to `1.0` when checked and `0.0`
/// when unchecked (so the caller's chosen fill `rgb` is preserved across toggles
/// and a single node both detects the click and shows the state — no covering
/// child to steal the hit).
pub struct Checkbox;

impl Checkbox {
    /// Creates a checkbox node under `parent` in the given initial `checked`
    /// state. `style.background` is the check fill color; its alpha is driven by
    /// the checked state.
    pub fn new(tree: &mut UiTree, parent: NodeId, checked: bool, style: Style) -> UiResult<NodeId> {
        let id = tree.create_node(style, parent)?;
        Self::set_checked(tree, id, checked)?;
        Ok(id)
    }

    /// Whether `node` was toggled (primary-clicked) this frame. The caller flips
    /// its model bit and calls [`set_checked`](Checkbox::set_checked).
    /// Allocation-free.
    pub fn toggled(node: NodeId, events: &[UiEvent]) -> bool {
        primary_clicked(node, events)
    }

    /// Updates the box fill to reflect `checked` (fill alpha `1.0`/`0.0`).
    pub fn set_checked(tree: &mut UiTree, node: NodeId, checked: bool) -> UiResult<()> {
        let mut style = tree.style(node).copied().ok_or(UiError::InvalidNode)?;
        style.background.a = if checked { 1.0 } else { 0.0 };
        tree.set_style(node, style)
    }
}

/// A numeric field for the inspector: displays an `f32` and reports a horizontal
/// drag delta while pressed. Exact text entry is deferred — drag-to-edit is the
/// MVP.
pub struct DragValue;

impl DragValue {
    /// Creates a drag-value node under `parent` displaying `value` in `font`.
    pub fn new(
        tree: &mut UiTree,
        parent: NodeId,
        value: f32,
        font: FontId,
        style: Style,
    ) -> UiResult<NodeId> {
        let id = tree.create_node(style, parent)?;
        tree.set_text(id, Some(format_value(value)))?;
        tree.set_font(id, font)?;
        Ok(id)
    }

    /// Updates the displayed value (parallels [`Checkbox::set_checked`]). The
    /// caller owns the value and re-displays it after applying a drag delta.
    pub fn set_value(tree: &mut UiTree, node: NodeId, value: f32) -> UiResult<()> {
        tree.set_text(node, Some(format_value(value)))
    }

    /// The horizontal value delta while `node` is under an active primary press,
    /// else `None`. The press state is the tree's retained
    /// [`active`](UiTree::active) — a single frame's events cannot convey a held
    /// drag, so this reads the tree rather than an event slice. The caller scales
    /// the returned pixel delta into a value step. Allocation-free.
    pub fn drag_delta(
        tree: &UiTree,
        node: NodeId,
        prev_pointer: Vec2,
        pointer: Vec2,
    ) -> Option<f32> {
        if tree.active(PointerButton::Primary) == Some(node) {
            Some(pointer.x - prev_pointer.x)
        } else {
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layout::TextMeasure;
    use crate::style::{Dimension, Size};
    use crate::{DrawCommand, DrawList, UiInputState};
    use spawn_core::Color;

    struct FixedMeasure;
    impl TextMeasure for FixedMeasure {
        fn measure(&mut self, _t: &str, _w: Option<f32>) -> Vec2 {
            Vec2::new(8.0, 12.0)
        }
    }

    fn px(w: f32, h: f32) -> Size {
        Size {
            width: Dimension::Px(w),
            height: Dimension::Px(h),
        }
    }

    fn filled(w: f32, h: f32) -> Style {
        Style {
            size: px(w, h),
            ..Default::default()
        }
    }

    fn frame(pointer: Vec2, primary: bool) -> UiInputState {
        UiInputState {
            pointer,
            primary_down: primary,
            secondary_down: false,
            wheel: Vec2::ZERO,
        }
    }

    fn laid_out_root(w: f32, h: f32) -> UiTree {
        UiTree::new(filled(w, h))
    }

    #[test]
    fn panel_creates_styled_container() {
        let mut tree = laid_out_root(100.0, 100.0);
        let root = tree.root();
        let style = Style {
            background: Color::new(0.1, 0.1, 0.1, 1.0),
            ..filled(50.0, 50.0)
        };
        let panel = Panel::new(&mut tree, root, style).unwrap();
        assert_eq!(tree.parent(panel), Some(root));
        assert_eq!(tree.style(panel).unwrap().background, style.background);
    }

    #[test]
    fn label_sets_text_and_font_in_draw_list() {
        let mut tree = laid_out_root(100.0, 100.0);
        let root = tree.root();
        let label = Label::new(&mut tree, root, "Hi", FontId(7), filled(40.0, 16.0)).unwrap();
        assert_eq!(tree.text(label), Some("Hi"));
        let mut m = FixedMeasure;
        tree.compute_layout(Vec2::new(100.0, 100.0), &mut m)
            .unwrap();
        let mut dl = DrawList::default();
        tree.build_draw_list(&mut dl).unwrap();
        let text_cmd = dl.commands().iter().find_map(|c| match c {
            DrawCommand::Text {
                font, text_node, ..
            } if *text_node == label => Some(*font),
            _ => None,
        });
        assert_eq!(text_cmd, Some(FontId(7)));
    }

    #[test]
    fn button_click_press_release_same_node() {
        let mut tree = laid_out_root(100.0, 100.0);
        let root = tree.root();
        let btn = Button::new(&mut tree, root, "OK", FontId(0), filled(100.0, 100.0)).unwrap();
        let mut m = FixedMeasure;
        tree.compute_layout(Vec2::new(100.0, 100.0), &mut m)
            .unwrap();

        let mut events = Vec::new();
        let p = Vec2::new(50.0, 50.0);
        tree.update_input(&frame(p, false)).unwrap();
        tree.update_input(&frame(p, true)).unwrap();
        tree.update_input(&frame(p, false)).unwrap();
        tree.drain_events(&mut events).unwrap();
        assert!(Button::clicked(btn, &events));
    }

    #[test]
    fn button_not_clicked_without_release_on_node() {
        let mut tree = laid_out_root(100.0, 100.0);
        let root = tree.root();
        let btn = Button::new(&mut tree, root, "OK", FontId(0), filled(100.0, 100.0)).unwrap();
        // No events at all -> not clicked; a click on a different node -> not clicked.
        assert!(!Button::clicked(btn, &[]));
        let elsewhere = [UiEvent::Click {
            node: tree.root(),
            button: PointerButton::Primary,
        }];
        assert!(!Button::clicked(btn, &elsewhere));
    }

    #[test]
    fn checkbox_alpha_reflects_checked_state() {
        let mut tree = laid_out_root(100.0, 100.0);
        let root = tree.root();
        let style = Style {
            background: Color::new(0.2, 0.4, 0.9, 1.0),
            ..filled(16.0, 16.0)
        };
        let cb = Checkbox::new(&mut tree, root, false, style).unwrap();
        assert_eq!(tree.style(cb).unwrap().background.a, 0.0);
        // rgb preserved while toggling.
        Checkbox::set_checked(&mut tree, cb, true).unwrap();
        let bg = tree.style(cb).unwrap().background;
        assert_eq!(bg.a, 1.0);
        assert_eq!((bg.r, bg.g, bg.b), (0.2, 0.4, 0.9));
        Checkbox::set_checked(&mut tree, cb, false).unwrap();
        assert_eq!(tree.style(cb).unwrap().background.a, 0.0);
    }

    #[test]
    fn checkbox_toggled_on_click() {
        let mut tree = laid_out_root(100.0, 100.0);
        let root = tree.root();
        let cb = Checkbox::new(&mut tree, root, false, filled(100.0, 100.0)).unwrap();
        let mut m = FixedMeasure;
        tree.compute_layout(Vec2::new(100.0, 100.0), &mut m)
            .unwrap();
        let mut events = Vec::new();
        let p = Vec2::new(50.0, 50.0);
        tree.update_input(&frame(p, false)).unwrap();
        tree.update_input(&frame(p, true)).unwrap();
        tree.update_input(&frame(p, false)).unwrap();
        tree.drain_events(&mut events).unwrap();
        assert!(Checkbox::toggled(cb, &events));
    }

    #[test]
    fn dragvalue_text_and_set_value() {
        let mut tree = laid_out_root(100.0, 100.0);
        let root = tree.root();
        let dv = DragValue::new(&mut tree, root, 1.5, FontId(0), filled(60.0, 16.0)).unwrap();
        assert_eq!(tree.text(dv), Some("1.500"));
        DragValue::set_value(&mut tree, dv, -2.0).unwrap();
        assert_eq!(tree.text(dv), Some("-2.000"));
    }

    #[test]
    fn dragvalue_delta_only_while_pressed() {
        let mut tree = laid_out_root(100.0, 100.0);
        let root = tree.root();
        let dv = DragValue::new(&mut tree, root, 0.0, FontId(0), filled(100.0, 100.0)).unwrap();
        let mut m = FixedMeasure;
        tree.compute_layout(Vec2::new(100.0, 100.0), &mut m)
            .unwrap();
        let p = Vec2::new(50.0, 50.0);
        // Not pressed -> no delta.
        assert_eq!(
            DragValue::drag_delta(&tree, dv, Vec2::new(10.0, 0.0), Vec2::new(13.0, 0.0)),
            None
        );
        // Press on the node -> active; horizontal delta reported.
        tree.update_input(&frame(p, true)).unwrap();
        assert_eq!(
            DragValue::drag_delta(&tree, dv, Vec2::new(10.0, 5.0), Vec2::new(13.0, 9.0)),
            Some(3.0)
        );
        // Release -> no delta.
        tree.update_input(&frame(p, false)).unwrap();
        assert_eq!(
            DragValue::drag_delta(&tree, dv, Vec2::new(10.0, 0.0), Vec2::new(13.0, 0.0)),
            None
        );
    }
}
