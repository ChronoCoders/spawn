//! Fixed-timestep decoupling from render rate, the spiral-of-death guard, and
//! end-to-end physics determinism.

use spawn_core::{Transform3D, Vec3};
use spawn_engine::{App, EngineConfig, HeadlessBackend};
use spawn_physics::ecs::{run_physics_fixed_update, Collider, PhysicsSyncState, RigidBody};
use spawn_physics::physics3d::{ColliderDesc, PhysicsConfig, PhysicsWorld, RigidBodyDesc, Shape};
use spawn_physics::CollisionEvent;

#[test]
fn fixed_ticks_decoupled_from_frame_rate() {
    // 1/64 s fixed step, 1/32 s frame delta (both exact in f32) → exactly two
    // fixed ticks per frame, independent of the render-frame count.
    let mut app = App::new();
    app.set_config(EngineConfig {
        fixed_timestep: 1.0 / 64.0,
        ..Default::default()
    });
    let mut engine = app
        .build_headless_with(1.0 / 32.0, Box::new(HeadlessBackend::new()))
        .unwrap();
    for _ in 0..10 {
        engine.tick().unwrap();
    }
    assert_eq!(engine.time().frame(), 10);
    assert_eq!(engine.time().fixed_tick(), 20);
}

#[test]
fn spiral_guard_caps_fixed_steps_per_frame() {
    let mut app = App::new();
    app.set_config(EngineConfig {
        fixed_timestep: 1.0 / 64.0,
        max_fixed_steps_per_frame: 8,
        max_frame_delta: 100.0,
        ..Default::default()
    });
    // A 10 s frame would want 640 fixed steps; the guard caps it at 8.
    let mut engine = app
        .build_headless_with(10.0, Box::new(HeadlessBackend::new()))
        .unwrap();
    engine.tick().unwrap();
    assert_eq!(engine.time().fixed_tick(), 8);
}

/// Final fall height of a dynamic body after a fixed number of frames, as raw
/// f32 bits for exact comparison.
fn run_fall_sim() -> u32 {
    let fixed_dt = 1.0 / 60.0;
    let mut app = App::new();
    app.set_config(EngineConfig {
        fixed_timestep: fixed_dt,
        ..Default::default()
    });
    let start = Transform3D::from_translation(Vec3::new(0.0, 10.0, 0.0));
    app.world_mut().spawn_with((
        start,
        RigidBody(RigidBodyDesc::dynamic().with_transform(start)),
        Collider(ColliderDesc::new(Shape::Ball { radius: 0.5 })),
    ));

    let mut physics = PhysicsWorld::new(PhysicsConfig {
        fixed_timestep: fixed_dt,
        ..Default::default()
    })
    .unwrap();
    let mut sync = PhysicsSyncState::new();
    let mut events: Vec<CollisionEvent> = Vec::new();
    app.add_fixed_hook(move |world, _time| {
        run_physics_fixed_update(world, &mut physics, &mut sync, &mut events);
        Ok(())
    });

    let mut engine = app.build_headless().unwrap();
    for _ in 0..90 {
        engine.tick().unwrap();
    }
    let y = engine
        .world()
        .query::<&Transform3D>()
        .iter()
        .next()
        .unwrap()
        .translation
        .y;
    y.to_bits()
}

#[test]
fn physics_is_deterministic_end_to_end() {
    let a = run_fall_sim();
    let b = run_fall_sim();
    assert_eq!(
        a, b,
        "identical inputs must produce bit-identical physics state"
    );
    // The body must actually have fallen from y = 10.
    assert!(f32::from_bits(a) < 10.0);
}
