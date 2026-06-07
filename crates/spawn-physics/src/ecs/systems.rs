//! Transform-sync and registration passes bridging the ECS and a
//! [`PhysicsWorld`].
//!
//! ## Why these are free functions, not registered [`spawn_ecs::System`]s
//!
//! The committed spawn-ecs runs systems with only `&World` plus a per-system
//! `&mut Commands`; systems never receive `&mut World`, and the crate has no
//! resource storage. Writing back post-step transforms requires `&mut`
//! component access, and the `PhysicsWorld` cannot be stored as an ECS resource.
//! These passes therefore take explicit `&mut World` / `&mut PhysicsWorld`
//! borrows. The normative ordering contract is preserved by the documented call
//! order below and enforced by [`run_physics_fixed_update`].
//!
//! ## Ordering contract (normative)
//!
//! `register_physics_bodies` → `sync_transforms_to_physics` → `PhysicsWorld::step`
//! → `sync_physics_to_transforms`, all within one fixed-tick stage.
//! `sync_transforms_to_physics` MUST precede `step`; `sync_physics_to_transforms`
//! MUST follow `step`.

use spawn_core::Transform3D;
use spawn_ecs::{Entity, World};

use crate::physics3d::PhysicsWorld;
use crate::shared::CollisionEvent;

use super::components::{Collider, PhysicsBody, RigidBody};

/// Tracks which entities own a live physics body so despawned entities have
/// their handles freed. Persisted across fixed ticks by the caller.
#[derive(Debug, Default)]
pub struct PhysicsSyncState {
    registered: Vec<(Entity, PhysicsBody)>,
}

impl PhysicsSyncState {
    pub fn new() -> Self {
        Self::default()
    }
}

/// Registers entities carrying [`RigidBody`] (and optional [`Collider`]) that
/// lack a [`PhysicsBody`], and frees handles for entities that have since been
/// despawned. Attaches [`PhysicsBody`] to newly registered entities.
pub fn register_physics_bodies(
    world: &mut World,
    physics: &mut PhysicsWorld,
    state: &mut PhysicsSyncState,
) {
    state.registered.retain(|(entity, link)| {
        if world.contains(*entity) {
            true
        } else {
            physics.remove_rigid_body(link.body);
            false
        }
    });

    let mut pending: Vec<(Entity, RigidBody, Option<Collider>)> = Vec::new();
    for entity in world
        .query::<Entity>()
        .with::<RigidBody>()
        .without::<PhysicsBody>()
        .iter_entities()
    {
        let rb = match world.get::<RigidBody>(entity) {
            Some(rb) => rb.clone(),
            None => continue,
        };
        let col = world.get::<Collider>(entity).cloned();
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
        let link = PhysicsBody { body, collider };
        if world.insert(entity, link).is_ok() {
            state.registered.push((entity, link));
        } else {
            physics.remove_rigid_body(body);
        }
    }
}

/// Pushes each entity's ECS [`Transform3D`] into its physics body (ECS-as-authority
/// for externally moved / kinematic bodies). MUST run before [`PhysicsWorld::step`].
pub fn sync_transforms_to_physics(world: &World, physics: &mut PhysicsWorld) {
    for (link, transform) in world.query::<(&PhysicsBody, &Transform3D)>().iter() {
        physics.set_body_transform(link.body, *transform);
    }
}

/// Writes each physics body's post-step pose back into its ECS [`Transform3D`],
/// preserving the component's existing `scale`. MUST run after
/// [`PhysicsWorld::step`].
pub fn sync_physics_to_transforms(world: &mut World, physics: &PhysicsWorld) {
    let updates: Vec<(Entity, Transform3D)> = world
        .query::<(Entity, &PhysicsBody)>()
        .iter()
        .filter_map(|(entity, link)| physics.body_transform(link.body).map(|t| (entity, t)))
        .collect();
    for (entity, pose) in updates {
        if let Some(transform) = world.get_mut::<Transform3D>(entity) {
            transform.translation = pose.translation;
            transform.rotation = pose.rotation;
        }
    }
}

/// Runs one fixed-tick stage in the normative order and returns the collision
/// events produced by the step.
pub fn run_physics_fixed_update(
    world: &mut World,
    physics: &mut PhysicsWorld,
    state: &mut PhysicsSyncState,
    events: &mut Vec<CollisionEvent>,
) {
    register_physics_bodies(world, physics, state);
    sync_transforms_to_physics(world, physics);
    events.clear();
    events.extend_from_slice(physics.step());
    sync_physics_to_transforms(world, physics);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::physics3d::{ColliderDesc, MassProperties, PhysicsConfig, RigidBodyDesc, Shape};
    use spawn_core::{ApproxEq, Vec3};

    fn setup(world: &mut World) {
        world.register::<RigidBody>();
        world.register::<Collider>();
        world.register::<PhysicsBody>();
        world.register::<Transform3D>();
    }

    #[test]
    fn registration_attaches_physics_body() {
        let mut world = World::new();
        setup(&mut world);
        let mut physics = PhysicsWorld::new(PhysicsConfig::default()).unwrap();
        let mut state = PhysicsSyncState::new();

        let e = world.spawn_with((
            RigidBody(RigidBodyDesc::dynamic()),
            Collider(ColliderDesc::new(Shape::Ball { radius: 0.5 })),
            Transform3D::from_translation(Vec3::new(0.0, 10.0, 0.0)),
        ));
        register_physics_bodies(&mut world, &mut physics, &mut state);
        assert!(world.has::<PhysicsBody>(e));
    }

    #[test]
    fn sync_to_physics_teleports_kinematic() {
        let mut world = World::new();
        setup(&mut world);
        let mut physics = PhysicsWorld::new(PhysicsConfig::default()).unwrap();
        let mut state = PhysicsSyncState::new();

        let e = world.spawn_with((
            RigidBody(RigidBodyDesc::kinematic()),
            Collider(ColliderDesc::new(Shape::Ball { radius: 0.5 })),
            Transform3D::IDENTITY,
        ));
        register_physics_bodies(&mut world, &mut physics, &mut state);
        let teleport = Transform3D::from_translation(Vec3::new(5.0, 6.0, 7.0));
        *world.get_mut::<Transform3D>(e).unwrap() = teleport;
        sync_transforms_to_physics(&world, &mut physics);
        let link = *world.get::<PhysicsBody>(e).unwrap();
        assert!(physics
            .body_transform(link.body)
            .unwrap()
            .translation
            .approx_eq(Vec3::new(5.0, 6.0, 7.0), 1e-4));
    }

    #[test]
    fn ordered_update_falls_and_preserves_scale() {
        let mut world = World::new();
        setup(&mut world);
        let mut physics = PhysicsWorld::new(PhysicsConfig::default()).unwrap();
        let mut state = PhysicsSyncState::new();
        let mut events = Vec::new();

        let mut start = Transform3D::from_translation(Vec3::new(0.0, 50.0, 0.0));
        start.scale = Vec3::new(2.0, 3.0, 4.0);
        let e = world.spawn_with((
            RigidBody(RigidBodyDesc::dynamic()),
            Collider(ColliderDesc::new(Shape::Ball { radius: 0.5 })),
            start,
        ));

        for _ in 0..30 {
            run_physics_fixed_update(&mut world, &mut physics, &mut state, &mut events);
        }
        let t = *world.get::<Transform3D>(e).unwrap();
        assert!(t.translation.y < 50.0, "did not fall: {}", t.translation.y);
        assert!(t.scale.approx_eq(Vec3::new(2.0, 3.0, 4.0), 1e-6));
    }

    #[test]
    fn ordering_contract_runs_in_order() {
        let mut world = World::new();
        setup(&mut world);
        let mut physics = PhysicsWorld::new(PhysicsConfig::default()).unwrap();
        let mut state = PhysicsSyncState::new();
        let mut events = Vec::new();

        let floor = world.spawn_with((
            RigidBody(RigidBodyDesc::fixed()),
            Collider(ColliderDesc::new(Shape::Cuboid {
                half_extents: Vec3::new(50.0, 0.5, 50.0),
            })),
            Transform3D::IDENTITY,
        ));
        let ball = world.spawn_with((
            RigidBody(RigidBodyDesc::dynamic()),
            Collider(ColliderDesc::new(Shape::Ball { radius: 0.5 })),
            Transform3D::from_translation(Vec3::new(0.0, 3.0, 0.0)),
        ));
        let mut saw_event = false;
        for _ in 0..200 {
            run_physics_fixed_update(&mut world, &mut physics, &mut state, &mut events);
            if !events.is_empty() {
                saw_event = true;
            }
        }
        let _ = floor;
        assert!(saw_event);
        assert!(world.get::<Transform3D>(ball).unwrap().translation.y < 3.0);
    }

    #[test]
    fn colliderless_body_falls_and_syncs() {
        let mut world = World::new();
        setup(&mut world);
        let mut physics = PhysicsWorld::new(PhysicsConfig::default()).unwrap();
        let mut state = PhysicsSyncState::new();
        let mut events = Vec::new();

        let e = world.spawn_with((
            RigidBody(RigidBodyDesc::dynamic().with_mass(MassProperties::Mass(1.0))),
            Transform3D::from_translation(Vec3::new(0.0, 50.0, 0.0)),
        ));
        register_physics_bodies(&mut world, &mut physics, &mut state);
        let link = *world.get::<PhysicsBody>(e).unwrap();
        assert!(link.collider.is_none());
        for _ in 0..30 {
            run_physics_fixed_update(&mut world, &mut physics, &mut state, &mut events);
        }
        assert!(world.get::<Transform3D>(e).unwrap().translation.y < 50.0);
    }

    #[test]
    fn despawn_frees_handle() {
        let mut world = World::new();
        setup(&mut world);
        let mut physics = PhysicsWorld::new(PhysicsConfig::default()).unwrap();
        let mut state = PhysicsSyncState::new();

        let e = world.spawn_with((
            RigidBody(RigidBodyDesc::dynamic()),
            Collider(ColliderDesc::new(Shape::Ball { radius: 0.5 })),
            Transform3D::IDENTITY,
        ));
        register_physics_bodies(&mut world, &mut physics, &mut state);
        let link = *world.get::<PhysicsBody>(e).unwrap();
        assert!(physics.body_transform(link.body).is_some());

        world.despawn(e).unwrap();
        register_physics_bodies(&mut world, &mut physics, &mut state);
        assert!(physics.body_transform(link.body).is_none());
    }
}
