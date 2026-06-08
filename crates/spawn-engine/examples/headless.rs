//! Headless example: the complete frame loop with a physics fixed hook, a
//! variable-rate system, and proxy extraction, against the headless backend.
//! Runs a fixed number of frames to a clean shutdown.
//!
//! Run with `cargo run -p spawn-engine --example headless`.

use std::error::Error;

use spawn_asset::AssetId;
use spawn_core::{Mat4, Transform3D, Vec3};
use spawn_ecs::{Commands, Query, Res, World};
use spawn_engine::{App, EngineConfig, RenderProxies, RenderProxy, ScheduleLabel, Time};
use spawn_physics::ecs::{run_physics_fixed_update, Collider, PhysicsSyncState, RigidBody};
use spawn_physics::physics3d::{ColliderDesc, PhysicsConfig, PhysicsWorld, RigidBodyDesc, Shape};
use spawn_physics::CollisionEvent;

fn main() -> Result<(), Box<dyn Error>> {
    let fixed_dt = 1.0 / 60.0;

    let mut app = App::new();
    app.set_config(EngineConfig {
        fixed_timestep: fixed_dt,
        ..Default::default()
    });

    // A dynamic body that falls under gravity.
    let start = Transform3D::from_translation(Vec3::new(0.0, 5.0, 0.0));
    app.world_mut().spawn_with((
        start,
        RigidBody(RigidBodyDesc::dynamic().with_transform(start)),
        Collider(ColliderDesc::new(Shape::Ball { radius: 0.5 })),
    ));

    // Physics as an exclusive fixed hook (the spawn-physics substepped reference).
    let mut physics = PhysicsWorld::new(PhysicsConfig {
        fixed_timestep: fixed_dt,
        ..Default::default()
    })?;
    let mut sync = PhysicsSyncState::new();
    let mut events: Vec<CollisionEvent> = Vec::new();
    app.add_fixed_hook(move |world, _time| {
        run_physics_fixed_update(world, &mut physics, &mut sync, &mut events);
        Ok(())
    });

    // A variable-rate system reading the frame clock.
    app.add_system(
        ScheduleLabel::Update,
        |_q: Query<'_, &Transform3D, ()>, _time: Res<'_, Time>, _c: &mut Commands<'_>| Ok(()),
    );

    // Extract each renderable transform into a backend-owned proxy.
    app.add_extract(|world: &World, proxies: &mut RenderProxies| {
        let mesh = AssetId::from_canonical_path("mesh");
        let material = AssetId::from_canonical_path("material");
        proxies.camera.projection = Mat4::IDENTITY;
        for transform in world.query::<&Transform3D>().iter() {
            proxies.draws.push(RenderProxy {
                model: transform.to_mat4(),
                mesh,
                material,
            });
        }
    });

    app.run_headless(120)?;
    println!("headless: ran 120 frames to a clean shutdown");
    Ok(())
}
