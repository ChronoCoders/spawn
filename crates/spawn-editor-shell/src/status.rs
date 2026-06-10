//! The status/debug strip: a quiet, always-on legibility line (the research's
//! antidote to editor opacity). Surfaces the mode, entity/draw counts, and the
//! active gizmo mode.

use spawn_ecs::World;
use spawn_ui::{FontId, Label, NodeId, Style, UiResult, UiTree};

use crate::gizmo::GizmoMode;
use crate::theme::Theme;

/// Builds the status strip's text node under `parent`.
pub fn build(tree: &mut UiTree, parent: NodeId, font: FontId, theme: &Theme) -> UiResult<NodeId> {
    Label::new(
        tree,
        parent,
        "",
        font,
        Style {
            background: theme.surface_raised,
            ..Default::default()
        },
    )
}

/// The status line text for the current frame.
pub fn text(world: &World, mode: GizmoMode, draws: usize, playing: bool) -> String {
    let mode = match mode {
        GizmoMode::Translate => "Move",
        GizmoMode::Rotate => "Rotate",
        GizmoMode::Scale => "Scale",
    };
    format!(
        "{} | entities {} | draws {} | gizmo {}",
        if playing { "PLAY" } else { "EDIT" },
        world.entity_count(),
        draws,
        mode,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_reports_mode_and_counts() {
        let mut world = World::new();
        world.register::<spawn_core::Transform3D>();
        world.spawn();
        world.spawn();
        let s = text(&world, GizmoMode::Rotate, 2, false);
        assert!(s.contains("EDIT"));
        assert!(s.contains("entities 2"));
        assert!(s.contains("draws 2"));
        assert!(s.contains("Rotate"));
        let p = text(&world, GizmoMode::Translate, 0, true);
        assert!(p.contains("PLAY"));
    }
}
