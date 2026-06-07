//! Transform-sync and registration passes bridging the ECS and a 2D
//! [`PhysicsWorld`]. Mirror of the 3D [`super::systems`] module under the
//! `Transform3D`→`Transform2D`, `Vec3`→`Vec2`, angular-`Vec3`→`f32`
//! substitution rules; only the dimensional types differ.
//!
//! ## Why these are free functions, not registered [`spawn_ecs::System`]s
//!
//! The committed spawn-ecs runs systems with only `&World` plus a per-system
//! `&mut Commands`; systems never receive `&mut World`, and the crate has no
//! resource storage. Writing back post-step transforms requires `&mut`
//! component access, and the `PhysicsWorld` cannot be stored as an ECS resource.
//! These passes therefore take explicit `&mut World` / `&mut PhysicsWorld`
//! borrows. The normative ordering contract is preserved by the documented call
//! order below and enforced by [`run_physics_fixed_update_2d`].
//!
//! ## Ordering contract (normative)
//!
//! `register_physics_bodies_2d` → `sync_transforms_to_physics_2d` →
//! `PhysicsWorld::step` → `sync_physics_to_transforms_2d`, all within one
//! fixed-tick stage. `sync_transforms_to_physics_2d` MUST precede `step`;
//! `sync_physics_to_transforms_2d` MUST follow `step`.

use spawn_core::Transform2D;
use spawn_ecs::{Entity, World};

use crate::physics2d::PhysicsWorld;
use crate::shared::CollisionEvent;

use super::components::{Collider2D, PhysicsBody2D, RigidBody2D};

/// Tracks which entities own a live physics body so despawned entities have
/// their handles freed. Persisted across fixed ticks by the caller.
#[derive(Debug, Default)]
pub struct PhysicsSyncState2D {
    registered: Vec<(Entity, PhysicsBody2D)>,
}

impl PhysicsSyncState2D {
    pub fn new() -> Self {
        Self::default()
    }
}

/// Registers entities carrying [`RigidBody2D`] (and optional [`Collider2D`])
/// that lack a [`PhysicsBody2D`], and frees handles for entities that have since
/// been despawned. Attaches [`PhysicsBody2D`] to newly registered entities.
pub fn register_physics_bodies_2d(
    world: &mut World,
    physics: &mut PhysicsWorld,
    state: &mut PhysicsSyncState2D,
) {
    state.registered.retain(|(entity, link)| {
        if world.contains(*entity) {
            true
        } else {
            physics.remove_rigid_body(link.body);
            false
        }
    });

    let mut pending: Vec<(Entity, RigidBody2D, Option<Collider2D>)> = Vec::new();
    for entity in world
        .query::<Entity>()
        .with::<RigidBody2D>()
        .without::<PhysicsBody2D>()
        .iter_entities()
    {
        let rb = match world.get::<RigidBody2D>(entity) {
            Some(rb) => rb.clone(),
            None => continue,
        };
        let col = world.get::<Collider2D>(entity).cloned();
        pending.push((entity, rb, col));
    }

    for (entity, rb, col) in pending {
        let body = physics.add_rigid_body(rb.0);
        let collider = match col {
            Some(c) => match physics.add_collider(body, c.0) {
                Ok(handle) => Some(handle),
                Err(_) => {
                    physics.remove_rigid_body(body);
                    continue;
                }
            },
            None => None,
        };
        let link = PhysicsBody2D { body, collider };
        if world.insert(entity, link).is_ok() {
            state.registered.push((entity, link));
        } else {
            physics.remove_rigid_body(body);
        }
    }
}

/// Pushes each entity's ECS [`Transform2D`] into its physics body (ECS-as-authority
/// for externally moved / kinematic bodies). MUST run before [`PhysicsWorld::step`].
pub fn sync_transforms_to_physics_2d(world: &World, physics: &mut PhysicsWorld) {
    for (link, transform) in world.query::<(&PhysicsBody2D, &Transform2D)>().iter() {
        physics.set_body_transform(link.body, *transform);
    }
}

/// Writes each physics body's post-step pose back into its ECS [`Transform2D`],
/// preserving the component's existing `scale`. MUST run after
/// [`PhysicsWorld::step`].
pub fn sync_physics_to_transforms_2d(world: &mut World, physics: &PhysicsWorld) {
    let updates: Vec<(Entity, Transform2D)> = world
        .query::<(Entity, &PhysicsBody2D)>()
        .iter()
        .filter_map(|(entity, link)| physics.body_transform(link.body).map(|t| (entity, t)))
        .collect();
    for (entity, pose) in updates {
        if let Some(transform) = world.get_mut::<Transform2D>(entity) {
            transform.translation = pose.translation;
            transform.rotation = pose.rotation;
        }
    }
}

/// Runs one fixed-tick stage in the normative order and returns the collision
/// events produced by the step.
pub fn run_physics_fixed_update_2d(
    world: &mut World,
    physics: &mut PhysicsWorld,
    state: &mut PhysicsSyncState2D,
    events: &mut Vec<CollisionEvent>,
) {
    register_physics_bodies_2d(world, physics, state);
    sync_transforms_to_physics_2d(world, physics);
    events.clear();
    events.extend_from_slice(physics.step());
    sync_physics_to_transforms_2d(world, physics);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::physics2d::{ColliderDesc, MassProperties, PhysicsConfig, RigidBodyDesc, Shape};
    use spawn_core::{ApproxEq, Vec2};

    fn setup(world: &mut World) {
        world.register::<RigidBody2D>();
        world.register::<Collider2D>();
        world.register::<PhysicsBody2D>();
        world.register::<Transform2D>();
    }

    #[test]
    fn registration_attaches_physics_body() {
        let mut world = World::new();
        setup(&mut world);
        let mut physics = PhysicsWorld::new(PhysicsConfig::default()).unwrap();
        let mut state = PhysicsSyncState2D::new();

        let e = world.spawn_with((
            RigidBody2D(RigidBodyDesc::dynamic()),
            Collider2D(ColliderDesc::new(Shape::Ball { radius: 0.5 })),
            Transform2D::from_translation(Vec2::new(0.0, 10.0)),
        ));
        register_physics_bodies_2d(&mut world, &mut physics, &mut state);
        assert!(world.has::<PhysicsBody2D>(e));
    }

    #[test]
    fn sync_to_physics_teleports_kinematic() {
        let mut world = World::new();
        setup(&mut world);
        let mut physics = PhysicsWorld::new(PhysicsConfig::default()).unwrap();
        let mut state = PhysicsSyncState2D::new();

        let e = world.spawn_with((
            RigidBody2D(RigidBodyDesc::kinematic()),
            Collider2D(ColliderDesc::new(Shape::Ball { radius: 0.5 })),
            Transform2D::IDENTITY,
        ));
        register_physics_bodies_2d(&mut world, &mut physics, &mut state);
        let teleport = Transform2D::from_translation(Vec2::new(5.0, 6.0));
        *world.get_mut::<Transform2D>(e).unwrap() = teleport;
        sync_transforms_to_physics_2d(&world, &mut physics);
        let link = *world.get::<PhysicsBody2D>(e).unwrap();
        assert!(physics
            .body_transform(link.body)
            .unwrap()
            .translation
            .approx_eq(Vec2::new(5.0, 6.0), 1e-4));
    }

    #[test]
    fn ordered_update_falls_and_preserves_scale() {
        let mut world = World::new();
        setup(&mut world);
        let mut physics = PhysicsWorld::new(PhysicsConfig::default()).unwrap();
        let mut state = PhysicsSyncState2D::new();
        let mut events = Vec::new();

        let mut start = Transform2D::from_translation(Vec2::new(0.0, 50.0));
        start.scale = Vec2::new(2.0, 3.0);
        let e = world.spawn_with((
            RigidBody2D(RigidBodyDesc::dynamic()),
            Collider2D(ColliderDesc::new(Shape::Ball { radius: 0.5 })),
            start,
        ));

        for _ in 0..30 {
            run_physics_fixed_update_2d(&mut world, &mut physics, &mut state, &mut events);
        }
        let t = *world.get::<Transform2D>(e).unwrap();
        assert!(t.translation.y < 50.0, "did not fall: {}", t.translation.y);
        assert!(t.scale.approx_eq(Vec2::new(2.0, 3.0), 1e-6));
    }

    #[test]
    fn ordering_contract_runs_in_order() {
        let mut world = World::new();
        setup(&mut world);
        let mut physics = PhysicsWorld::new(PhysicsConfig::default()).unwrap();
        let mut state = PhysicsSyncState2D::new();
        let mut events = Vec::new();

        let floor = world.spawn_with((
            RigidBody2D(RigidBodyDesc::fixed()),
            Collider2D(ColliderDesc::new(Shape::Cuboid {
                half_extents: Vec2::new(50.0, 0.5),
            })),
            Transform2D::IDENTITY,
        ));
        let ball = world.spawn_with((
            RigidBody2D(RigidBodyDesc::dynamic()),
            Collider2D(ColliderDesc::new(Shape::Ball { radius: 0.5 })),
            Transform2D::from_translation(Vec2::new(0.0, 3.0)),
        ));
        let mut saw_event = false;
        for _ in 0..200 {
            run_physics_fixed_update_2d(&mut world, &mut physics, &mut state, &mut events);
            if !events.is_empty() {
                saw_event = true;
            }
        }
        let _ = floor;
        assert!(saw_event);
        assert!(world.get::<Transform2D>(ball).unwrap().translation.y < 3.0);
    }

    #[test]
    fn colliderless_body_falls_and_syncs() {
        let mut world = World::new();
        setup(&mut world);
        let mut physics = PhysicsWorld::new(PhysicsConfig::default()).unwrap();
        let mut state = PhysicsSyncState2D::new();
        let mut events = Vec::new();

        let e = world.spawn_with((
            RigidBody2D(RigidBodyDesc::dynamic().with_mass(MassProperties::Mass(1.0))),
            Transform2D::from_translation(Vec2::new(0.0, 50.0)),
        ));
        register_physics_bodies_2d(&mut world, &mut physics, &mut state);
        let link = *world.get::<PhysicsBody2D>(e).unwrap();
        assert!(link.collider.is_none());
        for _ in 0..30 {
            run_physics_fixed_update_2d(&mut world, &mut physics, &mut state, &mut events);
        }
        assert!(world.get::<Transform2D>(e).unwrap().translation.y < 50.0);
    }

    #[test]
    fn despawn_frees_handle() {
        let mut world = World::new();
        setup(&mut world);
        let mut physics = PhysicsWorld::new(PhysicsConfig::default()).unwrap();
        let mut state = PhysicsSyncState2D::new();

        let e = world.spawn_with((
            RigidBody2D(RigidBodyDesc::dynamic()),
            Collider2D(ColliderDesc::new(Shape::Ball { radius: 0.5 })),
            Transform2D::IDENTITY,
        ));
        register_physics_bodies_2d(&mut world, &mut physics, &mut state);
        let link = *world.get::<PhysicsBody2D>(e).unwrap();
        assert!(physics.body_transform(link.body).is_some());

        world.despawn(e).unwrap();
        register_physics_bodies_2d(&mut world, &mut physics, &mut state);
        assert!(physics.body_transform(link.body).is_none());
    }
}
