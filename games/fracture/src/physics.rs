use std::collections::HashMap;

use spawn_core::{Quat, Transform3D, Vec3};
use spawn_ecs::{Commands, Component, EcsResult, Entity, World};
use spawn_engine::Time;
use spawn_physics::ecs::{
    register_physics_bodies, sync_physics_to_transforms, sync_transforms_to_physics, Collider,
    PhysicsBody, PhysicsSyncState, RigidBody,
};
use spawn_physics::physics3d::{
    ColliderDesc, LockFlags, PhysicsConfig, PhysicsWorld, RigidBodyDesc, Shape, Velocity,
};
use spawn_physics::{BVec3, ColliderHandle, CollisionEvent, PhysicsResult};

use crate::components::{Wall, WallSide};
use crate::field;
use crate::resources::{Collisions, Contact};

pub struct LinVel(pub Vec3);

impl Component for LinVel {}

fn planar_locks() -> LockFlags {
    LockFlags {
        translation: BVec3 {
            x: false,
            y: false,
            z: true,
        },
        rotation: BVec3 {
            x: true,
            y: true,
            z: true,
        },
    }
}

pub fn ball_bodies() -> (RigidBody, Collider) {
    (
        RigidBody(RigidBodyDesc::dynamic().with_locks(planar_locks())),
        Collider(
            ColliderDesc::new(Shape::Ball {
                radius: field::BALL_RADIUS,
            })
            .with_restitution(1.0)
            .with_friction(0.0),
        ),
    )
}

pub fn paddle_bodies() -> (RigidBody, Collider) {
    paddle_bodies_with(field::PADDLE_HALF_WIDTH)
}

pub fn paddle_bodies_with(half_width: f32) -> (RigidBody, Collider) {
    (
        RigidBody(RigidBodyDesc::kinematic().with_locks(planar_locks())),
        Collider(
            ColliderDesc::new(Shape::Cuboid {
                half_extents: Vec3::new(
                    half_width,
                    field::PADDLE_HALF_HEIGHT,
                    field::PADDLE_HALF_DEPTH,
                ),
            })
            .with_restitution(1.0)
            .with_friction(0.0),
        ),
    )
}

pub fn brick_bodies() -> (RigidBody, Collider) {
    (
        RigidBody(RigidBodyDesc::fixed()),
        Collider(
            ColliderDesc::new(Shape::Cuboid {
                half_extents: Vec3::new(
                    field::BRICK_HALF_WIDTH,
                    field::BRICK_HALF_HEIGHT,
                    field::BRICK_HALF_DEPTH,
                ),
            })
            .with_restitution(1.0)
            .with_friction(0.0),
        ),
    )
}

pub fn wall_bodies(half_extents: Vec3, sensor: bool) -> (RigidBody, Collider) {
    (
        RigidBody(RigidBodyDesc::fixed()),
        Collider(
            ColliderDesc::new(Shape::Cuboid { half_extents })
                .with_restitution(1.0)
                .with_friction(0.0)
                .as_sensor(sensor),
        ),
    )
}

pub fn powerup_bodies() -> (RigidBody, Collider) {
    (
        RigidBody(
            RigidBodyDesc::kinematic()
                .with_locks(planar_locks())
                .with_velocity(Velocity {
                    linear: Vec3::new(0.0, -field::POWERUP_FALL_SPEED, 0.0),
                    angular: Vec3::ZERO,
                }),
        ),
        Collider(
            ColliderDesc::new(Shape::Cuboid {
                half_extents: Vec3::new(
                    field::POWERUP_HALF,
                    field::POWERUP_HALF,
                    field::POWERUP_HALF,
                ),
            })
            .as_sensor(true),
        ),
    )
}

pub fn box_transform(center: Vec3, half_extents: Vec3) -> Transform3D {
    Transform3D {
        translation: center,
        rotation: Quat::IDENTITY,
        scale: Vec3::new(
            half_extents.x * 2.0,
            half_extents.y * 2.0,
            half_extents.z * 2.0,
        ),
    }
}

pub fn ball_transform(center: Vec3) -> Transform3D {
    Transform3D {
        translation: center,
        rotation: Quat::IDENTITY,
        scale: Vec3::new(
            field::BALL_RADIUS * 2.0,
            field::BALL_RADIUS * 2.0,
            field::BALL_RADIUS * 2.0,
        ),
    }
}

pub fn ball_start() -> Vec3 {
    Vec3::new(
        0.0,
        field::PADDLE_Y + field::PADDLE_HALF_HEIGHT + field::BALL_RADIUS,
        0.0,
    )
}

fn wall_geometry(side: WallSide) -> (Vec3, Vec3) {
    let half_t = field::WALL_THICKNESS * 0.5;
    let half_d = field::WALL_DEPTH * 0.5;
    match side {
        WallSide::Left => (
            Vec3::new(-(field::HALF_WIDTH + half_t), 0.0, 0.0),
            Vec3::new(half_t, field::HALF_HEIGHT + field::WALL_THICKNESS, half_d),
        ),
        WallSide::Right => (
            Vec3::new(field::HALF_WIDTH + half_t, 0.0, 0.0),
            Vec3::new(half_t, field::HALF_HEIGHT + field::WALL_THICKNESS, half_d),
        ),
        WallSide::Top => (
            Vec3::new(0.0, field::HALF_HEIGHT + half_t, 0.0),
            Vec3::new(field::HALF_WIDTH + field::WALL_THICKNESS, half_t, half_d),
        ),
        WallSide::Bottom => (
            Vec3::new(0.0, -(field::HALF_HEIGHT + half_t), 0.0),
            Vec3::new(field::HALF_WIDTH + field::WALL_THICKNESS, half_t, half_d),
        ),
    }
}

fn spawn_wall(commands: &mut Commands<'_>, side: WallSide) {
    let (center, half) = wall_geometry(side);
    let sensor = side == WallSide::Bottom;
    let (rb, col) = wall_bodies(half, sensor);
    commands.spawn_with((box_transform(center, half), Wall { side }, rb, col));
}

pub fn spawn_field(commands: &mut Commands<'_>) -> EcsResult<()> {
    spawn_wall(commands, WallSide::Left);
    spawn_wall(commands, WallSide::Right);
    spawn_wall(commands, WallSide::Top);
    spawn_wall(commands, WallSide::Bottom);

    let paddle_center = Vec3::new(0.0, field::PADDLE_Y, 0.0);
    let paddle_half = Vec3::new(
        field::PADDLE_HALF_WIDTH,
        field::PADDLE_HALF_HEIGHT,
        field::PADDLE_HALF_DEPTH,
    );
    let (paddle_rb, paddle_col) = paddle_bodies();
    commands.spawn_with((
        box_transform(paddle_center, paddle_half),
        crate::components::Paddle {
            half_width: field::PADDLE_HALF_WIDTH,
            min_x: -field::HALF_WIDTH + field::PADDLE_HALF_WIDTH,
            max_x: field::HALF_WIDTH - field::PADDLE_HALF_WIDTH,
        },
        paddle_rb,
        paddle_col,
    ));

    let (ball_rb, ball_col) = ball_bodies();
    commands.spawn_with((
        ball_transform(ball_start()),
        crate::components::Ball {
            speed: 0.0,
            launched: false,
        },
        LinVel(Vec3::ZERO),
        ball_rb,
        ball_col,
    ));

    Ok(())
}

fn config() -> PhysicsConfig {
    PhysicsConfig {
        gravity: Vec3::ZERO,
        fixed_timestep: field::FIXED_TIMESTEP,
        max_substeps_per_frame: 1,
    }
}

fn apply_velocities(world: &World, physics: &mut PhysicsWorld) {
    for (link, vel) in world.query::<(&PhysicsBody, &LinVel)>().iter() {
        physics.set_body_velocity(
            link.body,
            Velocity {
                linear: vel.0,
                angular: Vec3::ZERO,
            },
        );
    }
}

fn readback_velocities(world: &mut World, physics: &PhysicsWorld) {
    let updates: Vec<(Entity, Vec3)> = world
        .query::<(Entity, &PhysicsBody)>()
        .with::<LinVel>()
        .iter()
        .filter_map(|(entity, link)| physics.body_velocity(link.body).map(|v| (entity, v.linear)))
        .collect();
    for (entity, linear) in updates {
        if let Some(vel) = world.get_mut::<LinVel>(entity) {
            vel.0 = linear;
        }
    }
}

fn publish_contacts(
    world: &mut World,
    events: &[CollisionEvent],
    colliders: &mut HashMap<ColliderHandle, Entity>,
) {
    colliders.clear();
    for (entity, link) in world.query::<(Entity, &PhysicsBody)>().iter() {
        if let Some(handle) = link.collider {
            colliders.insert(handle, entity);
        }
    }
    if let Some(mut collisions) = world.get_resource_mut::<Collisions>() {
        collisions.contacts.clear();
        for event in events {
            let (a, b, started) = match event {
                CollisionEvent::Started(a, b) => (*a, *b, true),
                CollisionEvent::Stopped(a, b) => (*a, *b, false),
            };
            if let (Some(&a), Some(&b)) = (colliders.get(&a), colliders.get(&b)) {
                collisions.contacts.push(Contact { a, b, started });
            }
        }
    }
}

pub fn fixed_hook() -> PhysicsResult<impl FnMut(&mut World, &Time) -> EcsResult<()> + Send> {
    let mut physics = PhysicsWorld::new(config())?;
    let mut state = PhysicsSyncState::new();
    let mut events: Vec<CollisionEvent> = Vec::new();
    let mut colliders: HashMap<ColliderHandle, Entity> = HashMap::new();
    Ok(move |world: &mut World, _time: &Time| -> EcsResult<()> {
        register_physics_bodies(world, &mut physics, &mut state);
        sync_transforms_to_physics(world, &mut physics);
        apply_velocities(world, &mut physics);
        events.clear();
        events.extend_from_slice(physics.step());
        sync_physics_to_transforms(world, &physics);
        readback_velocities(world, &physics);
        publish_contacts(world, &events, &mut colliders);
        Ok(())
    })
}
