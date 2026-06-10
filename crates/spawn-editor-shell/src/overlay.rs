//! Overlay line assembly: the viewport grid and the selection highlight (the
//! primary in the accent, others dim). Gizmo handle lines are appended by the
//! caller via [`crate::gizmo::gizmo_lines`]. Toggleable, minimal by default.

use spawn_core::{Color, Transform3D, Vec3};
use spawn_ecs::World;
use spawn_editor::Selection;
use spawn_render::LineSegment;

use crate::theme::Theme;

const GRID_HALF: i32 = 10;
const GRID_STEP: f32 = 1.0;

/// Clears `out` and appends the (optional) grid and the selection-highlight
/// wireframes for the current `selection`. The primary selection uses
/// `theme.accent`; the rest use `theme.text_muted`.
pub fn assemble(
    world: &World,
    selection: &Selection,
    theme: &Theme,
    show_grid: bool,
    out: &mut Vec<LineSegment>,
) {
    out.clear();
    if show_grid {
        push_grid(theme, out);
    }
    let primary = selection.primary();
    for entity in selection.iter() {
        if let Some(transform) = world.get::<Transform3D>(entity) {
            let color = if Some(entity) == primary {
                theme.accent
            } else {
                theme.text_muted
            };
            push_box(transform, color, out);
        }
    }
}

fn push_grid(theme: &Theme, out: &mut Vec<LineSegment>) {
    let color = Color::new(
        theme.surface_overlay.r,
        theme.surface_overlay.g,
        theme.surface_overlay.b,
        1.0,
    );
    let extent = GRID_HALF as f32 * GRID_STEP;
    for i in -GRID_HALF..=GRID_HALF {
        let p = i as f32 * GRID_STEP;
        out.push(LineSegment {
            start: Vec3::new(p, 0.0, -extent),
            end: Vec3::new(p, 0.0, extent),
            color,
        });
        out.push(LineSegment {
            start: Vec3::new(-extent, 0.0, p),
            end: Vec3::new(extent, 0.0, p),
            color,
        });
    }
}

/// A 12-edge axis-aligned wireframe box around an entity, sized by its largest
/// scale axis (a floored half-extent so a zero-scale entity is still visible).
fn push_box(transform: &Transform3D, color: Color, out: &mut Vec<LineSegment>) {
    let c = transform.translation;
    let s = transform.scale;
    let h = (s.x.abs().max(s.y.abs()).max(s.z.abs()) * 0.5).max(0.25);
    let corner = |sx: f32, sy: f32, sz: f32| Vec3::new(c.x + sx * h, c.y + sy * h, c.z + sz * h);
    let v = [
        corner(-1.0, -1.0, -1.0),
        corner(1.0, -1.0, -1.0),
        corner(1.0, 1.0, -1.0),
        corner(-1.0, 1.0, -1.0),
        corner(-1.0, -1.0, 1.0),
        corner(1.0, -1.0, 1.0),
        corner(1.0, 1.0, 1.0),
        corner(-1.0, 1.0, 1.0),
    ];
    const EDGES: [(usize, usize); 12] = [
        (0, 1),
        (1, 2),
        (2, 3),
        (3, 0), // back face
        (4, 5),
        (5, 6),
        (6, 7),
        (7, 4), // front face
        (0, 4),
        (1, 5),
        (2, 6),
        (3, 7), // connectors
    ];
    for (a, b) in EDGES {
        out.push(LineSegment {
            start: v[a],
            end: v[b],
            color,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn selection_highlight_uses_accent_for_primary() {
        let mut world = World::new();
        world.register::<Transform3D>();
        let a = world.spawn_with((Transform3D::from_translation(Vec3::new(1.0, 0.0, 0.0)),));
        let b = world.spawn_with((Transform3D::from_translation(Vec3::new(-1.0, 0.0, 0.0)),));
        let mut sel = Selection::new();
        sel.select(a);
        sel.select(b); // b is primary (last selected)
        let theme = Theme::dark();
        let mut out = Vec::new();
        assemble(&world, &sel, &theme, false, &mut out);
        // Two boxes × 12 edges, no grid.
        assert_eq!(out.len(), 24);
        // Every segment of the primary (b) box is the accent; the other is muted.
        let accent_count = out.iter().filter(|l| l.color == theme.accent).count();
        let muted_count = out.iter().filter(|l| l.color == theme.text_muted).count();
        assert_eq!(accent_count, 12);
        assert_eq!(muted_count, 12);
    }

    #[test]
    fn grid_toggles() {
        let world = {
            let mut w = World::new();
            w.register::<Transform3D>();
            w
        };
        let sel = Selection::new();
        let theme = Theme::dark();
        let mut out = Vec::new();
        assemble(&world, &sel, &theme, false, &mut out);
        assert!(out.is_empty(), "no grid, no selection → no lines");
        assemble(&world, &sel, &theme, true, &mut out);
        assert!(!out.is_empty(), "grid produces lines");
    }
}
