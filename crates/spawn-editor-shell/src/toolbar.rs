//! The toolbar: Play/Stop, gizmo-mode toggle, and Undo/Redo. The accent marks
//! the active gizmo mode and the play state; nothing else.

use spawn_ui::{Button, Dimension, FontId, NodeId, Size, Style, UiEvent, UiResult, UiTree};

use crate::gizmo::GizmoMode;
use crate::theme::Theme;

/// What the toolbar reports the user did this frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolbarAction {
    TogglePlay,
    SetMode(GizmoMode),
    Undo,
    Redo,
}

/// The toolbar's button nodes.
pub struct Toolbar {
    play: NodeId,
    modes: [(NodeId, GizmoMode); 3],
    undo: NodeId,
    redo: NodeId,
}

impl Toolbar {
    /// Builds the toolbar buttons under `parent`.
    pub fn build(tree: &mut UiTree, parent: NodeId, font: FontId, theme: &Theme) -> UiResult<Self> {
        let play = Button::new(tree, parent, "Play", font, button(theme, 44.0))?;
        let mv = Button::new(tree, parent, "Move", font, button(theme, 48.0))?;
        let rot = Button::new(tree, parent, "Rotate", font, button(theme, 56.0))?;
        let scl = Button::new(tree, parent, "Scale", font, button(theme, 48.0))?;
        let undo = Button::new(tree, parent, "Undo", font, button(theme, 44.0))?;
        let redo = Button::new(tree, parent, "Redo", font, button(theme, 44.0))?;
        Ok(Self {
            play,
            modes: [
                (mv, GizmoMode::Translate),
                (rot, GizmoMode::Rotate),
                (scl, GizmoMode::Scale),
            ],
            undo,
            redo,
        })
    }

    /// The action the toolbar's drained `events` indicate, if any.
    pub fn action(&self, events: &[UiEvent]) -> Option<ToolbarAction> {
        if Button::clicked(self.play, events) {
            return Some(ToolbarAction::TogglePlay);
        }
        for (node, mode) in self.modes {
            if Button::clicked(node, events) {
                return Some(ToolbarAction::SetMode(mode));
            }
        }
        if Button::clicked(self.undo, events) {
            return Some(ToolbarAction::Undo);
        }
        if Button::clicked(self.redo, events) {
            return Some(ToolbarAction::Redo);
        }
        None
    }

    /// Refreshes the accent on the active gizmo mode and the play state.
    pub fn refresh(
        &self,
        tree: &mut UiTree,
        mode: GizmoMode,
        playing: bool,
        theme: &Theme,
    ) -> UiResult<()> {
        set_active(tree, self.play, playing, theme)?;
        for (node, m) in self.modes {
            set_active(tree, node, m == mode, theme)?;
        }
        Ok(())
    }
}

fn set_active(tree: &mut UiTree, node: NodeId, active: bool, theme: &Theme) -> UiResult<()> {
    let mut style = tree
        .style(node)
        .copied()
        .ok_or(spawn_ui::UiError::InvalidNode)?;
    style.background = if active {
        theme.accent
    } else {
        theme.surface_overlay
    };
    tree.set_style(node, style)
}

fn button(theme: &Theme, width: f32) -> Style {
    Style {
        size: Size {
            width: Dimension::Px(width),
            height: Dimension::Px(20.0),
        },
        background: theme.surface_overlay,
        ..Default::default()
    }
}
