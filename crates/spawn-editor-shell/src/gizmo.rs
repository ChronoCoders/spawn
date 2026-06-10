//! Gizmo rendering (translate/rotate/scale handles as overlay line geometry) and
//! the drag controller, over the Phase 1 `spawn_editor::gizmo` math. The gizmo
//! anchors at the primary selection's translation; a drag is one transaction.

use spawn_core::{Color, Quat, Transform3D, Vec3};
use spawn_ecs::{Entity, World};
use spawn_editor::{
    axis_drag_delta, ray_axis_handle_hit, rotation_angle_around_axis, Axis, CommandStack,
    EditorResult, Ray, SetTransform3D,
};
use spawn_render::LineSegment;

const RING_SEGMENTS: usize = 24;
const HANDLE_RADIUS: f32 = 0.12;

/// The active manipulation mode (a toolbar toggle).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GizmoMode {
    Translate,
    Rotate,
    Scale,
}

/// In-flight drag state: the picked axis, the previous frame's ray (incremental
/// deltas), and the dragged entity.
struct Drag {
    axis: Axis,
    prev_ray: Ray,
    entity: Entity,
}

/// Drives a gizmo drag: picks a handle, accumulates incremental transform deltas
/// into one merged transaction, and commits or aborts.
pub struct GizmoController {
    mode: GizmoMode,
    drag: Option<Drag>,
}

impl Default for GizmoController {
    fn default() -> Self {
        Self {
            mode: GizmoMode::Translate,
            drag: None,
        }
    }
}

impl GizmoController {
    pub fn mode(&self) -> GizmoMode {
        self.mode
    }

    pub fn set_mode(&mut self, mode: GizmoMode) {
        self.mode = mode;
    }

    pub fn is_dragging(&self) -> bool {
        self.drag.is_some()
    }

    /// The axis currently being dragged (for accent highlighting), if any.
    pub fn active_axis(&self) -> Option<Axis> {
        self.drag.as_ref().map(|d| d.axis)
    }

    /// Attempts to begin a drag from a primary-down `ray` against the handles at
    /// `anchor` (the primary selection's translation) with on-screen size
    /// `handle_len`. Opens a transaction and returns `true` if a handle was hit.
    pub fn begin(
        &mut self,
        ray: Ray,
        anchor: Vec3,
        handle_len: f32,
        entity: Entity,
        commands: &mut CommandStack,
    ) -> bool {
        match pick_handle(ray, anchor, handle_len) {
            Some(axis) => {
                commands.begin_transaction(match self.mode {
                    GizmoMode::Translate => "Move",
                    GizmoMode::Rotate => "Rotate",
                    GizmoMode::Scale => "Scale",
                });
                self.drag = Some(Drag {
                    axis,
                    prev_ray: ray,
                    entity,
                });
                true
            }
            None => false,
        }
    }

    /// Advances an in-flight drag by the incremental delta from the previous ray
    /// to `ray`, writing the entity's new `Transform3D` as a merged command.
    pub fn update(
        &mut self,
        ray: Ray,
        anchor: Vec3,
        world: &mut World,
        commands: &mut CommandStack,
    ) -> EditorResult<()> {
        let Some(drag) = self.drag.as_mut() else {
            return Ok(());
        };
        let Some(current) = world.get::<Transform3D>(drag.entity).copied() else {
            return Ok(());
        };
        let next = apply_delta(self.mode, drag.axis, anchor, drag.prev_ray, ray, current);
        drag.prev_ray = ray;
        commands.execute_merged(Box::new(SetTransform3D::new(drag.entity, next)), world)
    }

    /// Commits the drag as one undo entry.
    pub fn end(&mut self, world: &mut World, commands: &mut CommandStack) -> EditorResult<()> {
        if self.drag.take().is_some() {
            commands.end_transaction(world)?;
        }
        Ok(())
    }

    /// Aborts the drag, restoring the pre-drag transform.
    pub fn abort(&mut self, world: &mut World, commands: &mut CommandStack) -> EditorResult<()> {
        if self.drag.take().is_some() {
            commands.abort_transaction(world)?;
        }
        Ok(())
    }
}

/// Picks the nearest axis handle the `ray` hits (a cylinder per axis from
/// `anchor` of length `handle_len`), or `None`.
pub fn pick_handle(ray: Ray, anchor: Vec3, handle_len: f32) -> Option<Axis> {
    let mut best: Option<(f32, Axis)> = None;
    for axis in [Axis::X, Axis::Y, Axis::Z] {
        if let Some(t) =
            ray_axis_handle_hit(ray, anchor, axis, handle_len, HANDLE_RADIUS * handle_len)
        {
            if best.is_none_or(|(bt, _)| t < bt) {
                best = Some((t, axis));
            }
        }
    }
    best.map(|(_, a)| a)
}

/// Produces the new transform after applying the incremental gizmo delta between
/// `prev_ray` and `curr_ray` for `mode`/`axis` about `anchor`.
fn apply_delta(
    mode: GizmoMode,
    axis: Axis,
    anchor: Vec3,
    prev_ray: Ray,
    curr_ray: Ray,
    mut t: Transform3D,
) -> Transform3D {
    match mode {
        GizmoMode::Translate => {
            let d = axis_drag_delta(prev_ray, curr_ray, anchor, axis);
            t.translation += axis.unit() * d;
        }
        GizmoMode::Scale => {
            let d = axis_drag_delta(prev_ray, curr_ray, anchor, axis);
            match axis {
                Axis::X => t.scale.x += d,
                Axis::Y => t.scale.y += d,
                Axis::Z => t.scale.z += d,
            }
        }
        GizmoMode::Rotate => {
            let angle = rotation_angle_around_axis(prev_ray, curr_ray, anchor, axis);
            if let Some(q) = Quat::from_axis_angle(axis.unit(), angle) {
                t.rotation = q * t.rotation;
            }
        }
    }
    t
}

/// Appends the gizmo handle line geometry for `mode` to `out`. The active axis
/// uses `accent`; the others use conventional dimmed axis colors.
pub fn gizmo_lines(
    mode: GizmoMode,
    anchor: Vec3,
    handle_len: f32,
    active: Option<Axis>,
    accent: Color,
    out: &mut Vec<LineSegment>,
) {
    for axis in [Axis::X, Axis::Y, Axis::Z] {
        let color = if active == Some(axis) {
            accent
        } else {
            axis_color(axis)
        };
        match mode {
            GizmoMode::Translate | GizmoMode::Scale => {
                out.push(LineSegment {
                    start: anchor,
                    end: anchor + axis.unit() * handle_len,
                    color,
                });
            }
            GizmoMode::Rotate => push_ring(anchor, axis, handle_len, color, out),
        }
    }
}

fn push_ring(center: Vec3, axis: Axis, radius: f32, color: Color, out: &mut Vec<LineSegment>) {
    let (u, v) = ring_basis(axis);
    let point = |a: f32| center + (u * a.cos() + v * a.sin()) * radius;
    for i in 0..RING_SEGMENTS {
        let a0 = (i as f32) / RING_SEGMENTS as f32 * std::f32::consts::TAU;
        let a1 = ((i + 1) as f32) / RING_SEGMENTS as f32 * std::f32::consts::TAU;
        out.push(LineSegment {
            start: point(a0),
            end: point(a1),
            color,
        });
    }
}

/// The two in-plane basis vectors for a ring perpendicular to `axis`.
fn ring_basis(axis: Axis) -> (Vec3, Vec3) {
    match axis {
        Axis::X => (Vec3::Y, Vec3::Z),
        Axis::Y => (Vec3::Z, Vec3::X),
        Axis::Z => (Vec3::X, Vec3::Y),
    }
}

fn axis_color(axis: Axis) -> Color {
    match axis {
        Axis::X => Color::new(0.80, 0.25, 0.25, 1.0),
        Axis::Y => Color::new(0.25, 0.75, 0.30, 1.0),
        Axis::Z => Color::new(0.30, 0.45, 0.90, 1.0),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use spawn_core::{ApproxEq, Vec3};

    fn world_with(t: Transform3D) -> (World, Entity) {
        let mut w = World::new();
        w.register::<Transform3D>();
        let e = w.spawn_with((t,));
        (w, e)
    }

    fn down_ray(x: f32, z: f32) -> Ray {
        Ray {
            origin: Vec3::new(x, 5.0, z),
            direction: Vec3::NEG_Y,
        }
    }

    #[test]
    fn translate_drag_moves_along_axis_one_transaction() {
        let (mut w, e) = world_with(Transform3D::IDENTITY);
        let mut s = CommandStack::new(16);
        let mut g = GizmoController::default();
        g.set_mode(GizmoMode::Translate);
        // Start a drag on the X handle (ray straight down through the X axis).
        let start = down_ray(1.0, 0.0);
        assert!(g.begin(start, Vec3::ZERO, 5.0, e, &mut s));
        // Drag from x=1 to x=4 along the axis line.
        g.update(down_ray(4.0, 0.0), Vec3::ZERO, &mut w, &mut s)
            .unwrap();
        g.end(&mut w, &mut s).unwrap();
        let moved = w.get::<Transform3D>(e).unwrap().translation;
        assert!(
            (moved.x - 3.0).abs() < 1e-4,
            "moved +3 along X, got {}",
            moved.x
        );
        assert_eq!(s.len(), 1, "one undo entry for the whole drag");
        s.undo(&mut w).unwrap();
        assert!(w
            .get::<Transform3D>(e)
            .unwrap()
            .translation
            .approx_eq_default(Vec3::ZERO));
    }

    #[test]
    fn abort_restores_pre_drag_transform() {
        let (mut w, e) = world_with(Transform3D::IDENTITY);
        let mut s = CommandStack::new(16);
        let mut g = GizmoController::default();
        assert!(g.begin(down_ray(1.0, 0.0), Vec3::ZERO, 5.0, e, &mut s));
        g.update(down_ray(4.0, 0.0), Vec3::ZERO, &mut w, &mut s)
            .unwrap();
        g.abort(&mut w, &mut s).unwrap();
        assert!(w
            .get::<Transform3D>(e)
            .unwrap()
            .translation
            .approx_eq_default(Vec3::ZERO));
        assert_eq!(s.len(), 0);
    }

    #[test]
    fn pick_handle_hits_the_aligned_axis() {
        // A straight-down ray through x=1 hits the X handle.
        assert_eq!(
            pick_handle(down_ray(1.0, 0.0), Vec3::ZERO, 5.0),
            Some(Axis::X)
        );
        // Far off any axis → miss.
        assert_eq!(pick_handle(down_ray(20.0, 20.0), Vec3::ZERO, 5.0), None);
    }

    #[test]
    fn gizmo_lines_active_axis_uses_accent() {
        let accent = Color::new(1.0, 0.0, 0.0, 1.0);
        let mut out = Vec::new();
        gizmo_lines(
            GizmoMode::Translate,
            Vec3::ZERO,
            5.0,
            Some(Axis::Y),
            accent,
            &mut out,
        );
        assert_eq!(out.len(), 3, "three translate axis lines");
        // The Y line uses the accent; X/Z do not.
        let y = out[1];
        assert_eq!(y.color, accent);
        assert_ne!(out[0].color, accent);
        // Rotate emits rings.
        out.clear();
        gizmo_lines(GizmoMode::Rotate, Vec3::ZERO, 5.0, None, accent, &mut out);
        assert_eq!(out.len(), RING_SEGMENTS * 3);
    }
}
