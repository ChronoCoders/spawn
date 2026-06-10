//! The outliner: a flat list of live entities. Click selects; the selected row
//! uses the accent. Hierarchy/parenting is deferred (flat list).

use spawn_ecs::{Entity, World};
use spawn_editor::Selection;
use spawn_ui::{Button, Dimension, FontId, NodeId, Size, Style, UiEvent, UiResult, UiTree};

use crate::theme::Theme;
use crate::util::clear_children;

/// The outliner's per-entity rows, rebuilt when the entity set or selection
/// changes.
pub struct Outliner {
    rows: Vec<(NodeId, Entity)>,
}

impl Outliner {
    /// Rebuilds the rows under `parent` from the world's live entities, accenting
    /// the selected rows.
    pub fn rebuild(
        tree: &mut UiTree,
        world: &World,
        selection: &Selection,
        parent: NodeId,
        font: FontId,
        theme: &Theme,
    ) -> UiResult<Self> {
        clear_children(tree, parent)?;
        let mut rows = Vec::new();
        for entity in world.query::<Entity>().iter_entities() {
            let selected = selection.is_selected(entity);
            let label = format!("Entity {}", entity.index());
            let node = Button::new(tree, parent, label, font, row_style(theme, selected))?;
            rows.push((node, entity));
        }
        Ok(Self { rows })
    }

    /// The entity whose row was clicked this frame, if any.
    pub fn clicked(&self, events: &[UiEvent]) -> Option<Entity> {
        self.rows
            .iter()
            .find(|(node, _)| Button::clicked(*node, events))
            .map(|(_, e)| *e)
    }
}

fn row_style(theme: &Theme, selected: bool) -> Style {
    Style {
        size: Size {
            width: Dimension::Auto,
            height: Dimension::Px(14.0),
        },
        background: if selected {
            theme.accent
        } else {
            theme.surface_raised
        },
        margin: spawn_ui::Edges::axis(0.0, 1.0),
        ..Default::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use spawn_core::Transform3D;

    #[test]
    fn rebuild_lists_live_entities_and_accents_selection() {
        let mut world = World::new();
        world.register::<Transform3D>();
        let a = world.spawn_with((Transform3D::IDENTITY,));
        let _b = world.spawn_with((Transform3D::IDENTITY,));
        let mut sel = Selection::new();
        sel.select(a);

        let mut tree = UiTree::new(Style::default());
        let root = tree.root();
        let outliner =
            Outliner::rebuild(&mut tree, &world, &sel, root, FontId(1), &Theme::dark()).unwrap();
        assert_eq!(outliner.rows.len(), 2);
        // The selected row (a) is accented.
        let (a_node, _) = outliner.rows.iter().find(|(_, e)| *e == a).unwrap();
        assert_eq!(
            tree.style(*a_node).unwrap().background,
            Theme::dark().accent
        );
    }
}
