//! CPU entity picking: intersect a world ray against per-entity bounds and pick
//! the nearest. No GPU readback (pixel-exact ID-buffer picking is out of scope).

use spawn_core::{Transform3D, Vec3};
use spawn_ecs::{Entity, World};
use spawn_editor::Ray;

/// The nearest entity whose bound the `ray` hits, or `None`. Bounds are spheres
/// derived from each entity's `Transform3D` (a small default radius scaled by the
/// transform's largest scale axis).
pub fn pick_entity(world: &World, ray: Ray) -> Option<Entity> {
    let mut best: Option<(f32, Entity)> = None;
    for (entity, transform) in world.query::<(Entity, &Transform3D)>().iter() {
        if let Some(t) = ray_sphere(ray, transform.translation, bound_radius(transform)) {
            if best.is_none_or(|(bt, _)| t < bt) {
                best = Some((t, entity));
            }
        }
    }
    best.map(|(_, e)| e)
}

/// A picking-bound radius for an entity: a base radius scaled by its largest
/// (absolute) scale axis, floored so a zero-scale entity is still pickable.
fn bound_radius(transform: &Transform3D) -> f32 {
    let s = transform.scale;
    let max_axis = s.x.abs().max(s.y.abs()).max(s.z.abs());
    (max_axis * 0.6).max(0.25)
}

/// Ray–sphere intersection: the nearest non-negative parameter `t` at which `ray`
/// meets the sphere `(center, radius)`, or `None`. `ray.direction` is unit length.
fn ray_sphere(ray: Ray, center: Vec3, radius: f32) -> Option<f32> {
    let oc = ray.origin - center;
    let b = oc.dot(ray.direction);
    let c = oc.dot(oc) - radius * radius;
    let disc = b * b - c;
    if disc < 0.0 {
        return None;
    }
    let sqrt_d = disc.sqrt();
    let t0 = -b - sqrt_d;
    if t0 >= 0.0 {
        return Some(t0);
    }
    let t1 = -b + sqrt_d;
    if t1 >= 0.0 {
        Some(t1)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn world_with(transforms: &[Transform3D]) -> (World, Vec<Entity>) {
        let mut w = World::new();
        w.register::<Transform3D>();
        let ids = transforms.iter().map(|t| w.spawn_with((*t,))).collect();
        (w, ids)
    }

    #[test]
    fn picks_nearest_of_two() {
        let near = Transform3D::from_translation(Vec3::new(0.0, 0.0, -2.0));
        let far = Transform3D::from_translation(Vec3::new(0.0, 0.0, -8.0));
        let (w, ids) = world_with(&[far, near]); // declared far-first
        let ray = Ray {
            origin: Vec3::ZERO,
            direction: Vec3::new(0.0, 0.0, -1.0),
        };
        assert_eq!(pick_entity(&w, ray), Some(ids[1]), "nearer entity wins");
    }

    #[test]
    fn miss_returns_none() {
        let (w, _) = world_with(&[Transform3D::from_translation(Vec3::new(50.0, 0.0, 0.0))]);
        let ray = Ray {
            origin: Vec3::ZERO,
            direction: Vec3::new(0.0, 0.0, -1.0),
        };
        assert_eq!(pick_entity(&w, ray), None);
    }

    #[test]
    fn ray_behind_sphere_does_not_hit() {
        // Sphere behind the ray origin (ray points +Z, sphere at -Z).
        assert!(ray_sphere(
            Ray {
                origin: Vec3::ZERO,
                direction: Vec3::new(0.0, 0.0, 1.0),
            },
            Vec3::new(0.0, 0.0, -3.0),
            0.5,
        )
        .is_none());
    }
}
