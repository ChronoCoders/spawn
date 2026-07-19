//! The reflection-driven inspector: build one editable row per reflected scalar
//! leaf of the primary selection, and route every edit through the command stack
//! so it is undoable. A continuous `DragValue` drag coalesces into one undo entry
//! via the command merge (the Phase 1 special case of a scoped transaction).
//!
//! The [`SetReflectedField`] command (a `spawn_editor::Command`) writes a leaf
//! through the step-0 `World::reflect_set_field`, capturing the prior value for
//! revert; it lives here (keeps spawn-editor reflection-agnostic).

use core::any::Any;

use spawn_ecs::{ComponentId, Entity, FieldKind, FieldValue, World};
use spawn_editor::{Command, CommandStack, EditorError, EditorResult};
use spawn_ui::{
    Border, Checkbox, Dimension, DragValue, FlexDirection, FontId, Label, Panel, Size, Style,
    UiResult, UiTree,
};

use crate::theme::Theme;

/// A command that writes one reflected scalar leaf, capturing the prior value for
/// revert. Mergeable across a continuous edit on the same `(entity, component,
/// field)` so a `DragValue` drag stays one undo step.
pub(crate) struct SetReflectedField {
    entity: Entity,
    component: ComponentId,
    field: &'static str,
    new: FieldValue,
    old: Option<FieldValue>,
}

impl SetReflectedField {
    pub(crate) fn new(
        entity: Entity,
        component: ComponentId,
        field: &'static str,
        new: FieldValue,
    ) -> Self {
        Self {
            entity,
            component,
            field,
            new,
            old: None,
        }
    }

    fn write(&self, world: &mut World, value: FieldValue) -> EditorResult<()> {
        world
            .reflect_set_field(self.entity, self.component, self.field, value)
            .map_err(|_| EditorError::ComponentMissing {
                entity: self.entity,
                component: "reflected field",
            })
    }
}

impl Command for SetReflectedField {
    fn apply(&mut self, world: &mut World) -> EditorResult<()> {
        if self.old.is_none() {
            self.old = world.reflect_get_field(self.entity, self.component, self.field);
            if self.old.is_none() {
                return Err(EditorError::ComponentMissing {
                    entity: self.entity,
                    component: "reflected field",
                });
            }
        }
        self.write(world, self.new)
    }

    fn revert(&mut self, world: &mut World) -> EditorResult<()> {
        match self.old {
            Some(old) => self.write(world, old),
            None => Ok(()),
        }
    }

    fn label(&self) -> &str {
        "Edit Field"
    }

    fn as_any(&self) -> &dyn Any {
        self
    }

    fn try_merge(&mut self, next: &dyn Command) -> bool {
        match next.as_any().downcast_ref::<SetReflectedField>() {
            Some(other)
                if other.entity == self.entity
                    && other.component == self.component
                    && other.field == self.field =>
            {
                self.new = other.new;
                true
            }
            _ => false,
        }
    }
}

/// One built inspector row: the field it edits and its widget node.
#[derive(Debug, Clone, Copy)]
pub struct FieldRow {
    pub component: ComponentId,
    pub field: &'static str,
    pub kind: FieldKind,
    pub widget: spawn_ui::NodeId,
}

/// Builds the inspector rows for `entity` under `parent`, clearing any prior
/// rows. Returns the row descriptors (for per-frame value refresh and edit
/// dispatch). One collapsible section per reflected component; one row per scalar
/// leaf (`F32`/integer → `DragValue`, `Bool` → `Checkbox`).
pub fn build_rows(
    tree: &mut UiTree,
    world: &World,
    entity: Entity,
    parent: spawn_ui::NodeId,
    font: FontId,
    theme: &Theme,
) -> UiResult<Vec<FieldRow>> {
    crate::util::clear_children(tree, parent)?;
    let mut rows = Vec::new();
    for component in world.reflected_components(entity) {
        let section = Panel::new(tree, parent, section_style(theme))?;
        Label::new(
            tree,
            section,
            component.type_name,
            font,
            header_style(theme),
        )?;
        for field in component.fields {
            let row = Panel::new(tree, section, row_style())?;
            Label::new(tree, row, field.name, font, label_style(theme))?;
            let value = world.reflect_get_field(entity, component.component, field.name);
            let widget = match field.kind {
                FieldKind::Bool => {
                    let checked = matches!(value, Some(FieldValue::Bool(true)));
                    Checkbox::new(tree, row, checked, checkbox_style(theme))?
                }
                _ => {
                    let v = scalar_of(value);
                    DragValue::new(tree, row, v, font, value_style(theme))?
                }
            };
            rows.push(FieldRow {
                component: component.component,
                field: field.name,
                kind: field.kind,
                widget,
            });
        }
    }
    Ok(rows)
}

/// Refreshes each row's displayed value from reflection (steady-state, no
/// rebuild). Allocation-free.
pub fn refresh_values(tree: &mut UiTree, world: &World, entity: Entity, rows: &[FieldRow]) {
    for row in rows {
        match world.reflect_get_field(entity, row.component, row.field) {
            Some(FieldValue::Bool(b)) => {
                let _ = Checkbox::set_checked(tree, row.widget, b);
            }
            Some(other) => {
                let _ = DragValue::set_value(tree, row.widget, scalar_of(Some(other)));
            }
            None => {}
        }
    }
}

/// Applies a `DragValue` edit: the field's current value plus `delta` (already
/// scaled by the caller). Routed through `execute_merged`, so consecutive edits
/// of the same `(entity, component, field)`, a continuous drag, coalesce into
/// one undo entry (the merge is the Phase 1 special case of a transaction); a
/// discrete edit is one entry too. Bool rows are toggled via [`apply_bool`].
pub fn apply_scalar_delta(
    commands: &mut CommandStack,
    world: &mut World,
    row: FieldRow,
    entity: Entity,
    delta: f32,
) -> EditorResult<()> {
    let current = world
        .reflect_get_field(entity, row.component, row.field)
        .map(|v| scalar_of(Some(v)))
        .unwrap_or(0.0);
    let next = quantize(row.kind, current + delta);
    commands.execute_merged(
        Box::new(SetReflectedField::new(
            entity,
            row.component,
            row.field,
            next,
        )),
        world,
    )
}

/// Toggles a `Bool` row as one undo entry (a discrete click).
pub fn apply_bool(
    commands: &mut CommandStack,
    world: &mut World,
    row: FieldRow,
    entity: Entity,
) -> EditorResult<()> {
    let current = matches!(
        world.reflect_get_field(entity, row.component, row.field),
        Some(FieldValue::Bool(true))
    );
    commands.execute(
        Box::new(SetReflectedField::new(
            entity,
            row.component,
            row.field,
            FieldValue::Bool(!current),
        )),
        world,
    )
}

fn scalar_of(value: Option<FieldValue>) -> f32 {
    match value {
        Some(FieldValue::F32(v)) => v,
        Some(FieldValue::I32(v)) => v as f32,
        Some(FieldValue::U32(v)) => v as f32,
        Some(FieldValue::U64(v)) => v as f32,
        Some(FieldValue::Bool(b)) => b as i32 as f32,
        None => 0.0,
    }
}

fn quantize(kind: FieldKind, v: f32) -> FieldValue {
    match kind {
        FieldKind::F32 => FieldValue::F32(v),
        FieldKind::I32 => FieldValue::I32(v.round() as i32),
        FieldKind::U32 => FieldValue::U32(v.round().max(0.0) as u32),
        FieldKind::U64 => FieldValue::U64(v.round().max(0.0) as u64),
        FieldKind::Bool => FieldValue::Bool(v != 0.0),
    }
}

fn section_style(theme: &Theme) -> Style {
    Style {
        flex_direction: FlexDirection::Column,
        background: theme.surface_raised,
        padding: spawn_ui::Edges::all(4.0),
        margin: spawn_ui::Edges::axis(0.0, 2.0),
        ..Default::default()
    }
}

fn header_style(theme: &Theme) -> Style {
    Style {
        size: Size {
            width: Dimension::Auto,
            height: Dimension::Px(14.0),
        },
        background: theme.accent_dim,
        ..Default::default()
    }
}

fn row_style() -> Style {
    Style {
        flex_direction: FlexDirection::Row,
        gap: 4.0,
        size: Size {
            width: Dimension::Auto,
            height: Dimension::Px(14.0),
        },
        ..Default::default()
    }
}

fn label_style(theme: &Theme) -> Style {
    Style {
        size: Size {
            width: Dimension::Px(96.0),
            height: Dimension::Px(12.0),
        },
        background: theme.surface_raised,
        ..Default::default()
    }
}

fn value_style(theme: &Theme) -> Style {
    Style {
        flex_grow: 1.0,
        size: Size {
            width: Dimension::Auto,
            height: Dimension::Px(12.0),
        },
        background: theme.surface_overlay,
        ..Default::default()
    }
}

fn checkbox_style(theme: &Theme) -> Style {
    Style {
        size: Size {
            width: Dimension::Px(12.0),
            height: Dimension::Px(12.0),
        },
        background: theme.accent,
        border: Border {
            width: 1.0,
            color: theme.text_muted,
        },
        ..Default::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use spawn_core::{Transform3D, Vec3};

    fn world_with_transform(x: f32) -> (World, Entity) {
        let mut w = World::new();
        w.register_reflect::<Transform3D>();
        let e = w.spawn_with((Transform3D::from_translation(Vec3::new(x, 0.0, 0.0)),));
        (w, e)
    }

    #[test]
    fn set_reflected_field_roundtrips_and_merges() {
        let (mut w, e) = world_with_transform(1.0);
        let id = w.component_id::<Transform3D>().unwrap();
        let mut s = CommandStack::new(16);
        s.execute(
            Box::new(SetReflectedField::new(
                e,
                id,
                "translation.x",
                FieldValue::F32(5.0),
            )),
            &mut w,
        )
        .unwrap();
        assert_eq!(w.get::<Transform3D>(e).unwrap().translation.x, 5.0);
        s.undo(&mut w).unwrap();
        assert_eq!(w.get::<Transform3D>(e).unwrap().translation.x, 1.0);

        // A merged drag of the same field collapses to one entry.
        for v in [2.0, 3.0, 4.0] {
            s.execute_merged(
                Box::new(SetReflectedField::new(
                    e,
                    id,
                    "translation.x",
                    FieldValue::F32(v),
                )),
                &mut w,
            )
            .unwrap();
        }
        // (one prior undone entry remains on redo; the three merged form one new entry)
        s.undo(&mut w).unwrap();
        assert_eq!(w.get::<Transform3D>(e).unwrap().translation.x, 1.0);
    }

    #[test]
    fn build_rows_enumerates_transform_leaves() {
        let (w, e) = world_with_transform(0.0);
        let mut tree = UiTree::new(Style::default());
        let root = tree.root();
        let rows = build_rows(&mut tree, &w, e, root, FontId(1), &Theme::dark()).unwrap();
        assert_eq!(rows.len(), 10, "Transform3D exposes 10 scalar leaves");
        assert!(rows.iter().any(|r| r.field == "translation.x"));
        assert!(rows.iter().all(|r| r.kind == FieldKind::F32));
    }

    #[test]
    fn drag_edit_merges_into_one_undo_entry() {
        let (mut w, e) = world_with_transform(1.0);
        let mut tree = UiTree::new(Style::default());
        let root = tree.root();
        let rows = build_rows(&mut tree, &w, e, root, FontId(1), &Theme::dark()).unwrap();
        let row = rows
            .iter()
            .find(|r| r.field == "translation.y")
            .copied()
            .unwrap();
        let mut s = CommandStack::new(16);
        // A multi-frame drag: each call reads the current value and adds the delta.
        for delta in [1.0, 1.0, 0.5] {
            apply_scalar_delta(&mut s, &mut w, row, e, delta).unwrap();
        }
        assert!((w.get::<Transform3D>(e).unwrap().translation.y - 2.5).abs() < 1e-5);
        assert_eq!(
            s.len(),
            1,
            "a continuous drag coalesces into one undo entry"
        );
        // One undo rewinds the whole drag to the pre-edit value.
        s.undo(&mut w).unwrap();
        assert_eq!(w.get::<Transform3D>(e).unwrap().translation.y, 0.0);
    }
}
