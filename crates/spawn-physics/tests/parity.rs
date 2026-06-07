//! 2D/3D parity: a shared parameterized free-fall + bounce scenario run through
//! both `physics2d` and `physics3d` must agree in the shared XY plane within
//! epsilon, proving the 2D module is a behaviorally faithful mirror of the 3D
//! one (spec §11 "2D/3D parity"). Only built when both backends are present.

#![cfg(all(feature = "dim2", feature = "dim3"))]

use spawn_core::ApproxEq;
use spawn_physics::{physics2d, physics3d};

/// Scenario parameters shared by both dimensions.
struct Scenario {
    drop_height: f32,
    restitution: f32,
    radius: f32,
    floor_half: f32,
    floor_y: f32,
    ticks: u32,
}

// Tick counts are chosen large enough for each ball to come fully to rest, so
// both dimensions converge to the identical resting pose (x = 0, y = floor_y +
// 0.5 + radius); comparing at rest keeps the parity bound tight even with
// restitution, where mid-flight trajectories are sensitive to step ordering.
const SCENARIOS: &[Scenario] = &[
    Scenario {
        drop_height: 5.0,
        restitution: 0.0,
        radius: 0.5,
        floor_half: 50.0,
        floor_y: 0.0,
        ticks: 600,
    },
    Scenario {
        drop_height: 8.0,
        restitution: 0.8,
        radius: 0.4,
        floor_half: 50.0,
        floor_y: 0.0,
        ticks: 1200,
    },
    Scenario {
        drop_height: 12.0,
        restitution: 0.5,
        radius: 0.6,
        floor_half: 50.0,
        floor_y: -1.0,
        ticks: 900,
    },
];

/// Runs the scenario in 3D (gravity along -Y, body at z = 0) and returns the
/// final (x, y) of the falling ball in the shared plane.
fn run_3d(s: &Scenario) -> (f32, f32) {
    use physics3d::{ColliderDesc, PhysicsConfig, PhysicsWorld, RigidBodyDesc, Shape};
    use spawn_core::{Transform3D, Vec3};

    let mut w = PhysicsWorld::new(PhysicsConfig::default()).unwrap();
    let floor = w.add_rigid_body(RigidBodyDesc::fixed().with_transform(
        Transform3D::from_translation(Vec3::new(0.0, s.floor_y, 0.0)),
    ));
    w.add_collider(
        floor,
        ColliderDesc::new(Shape::Cuboid {
            half_extents: Vec3::new(s.floor_half, 0.5, s.floor_half),
        })
        .with_restitution(s.restitution),
    )
    .unwrap();
    let ball = w.add_rigid_body(RigidBodyDesc::dynamic().with_transform(
        Transform3D::from_translation(Vec3::new(0.0, s.drop_height, 0.0)),
    ));
    w.add_collider(
        ball,
        ColliderDesc::new(Shape::Ball { radius: s.radius }).with_restitution(s.restitution),
    )
    .unwrap();
    for _ in 0..s.ticks {
        w.step();
    }
    let t = w.body_transform(ball).unwrap().translation;
    (t.x, t.y)
}

/// Runs the same scenario in 2D and returns the final (x, y).
fn run_2d(s: &Scenario) -> (f32, f32) {
    use physics2d::{ColliderDesc, PhysicsConfig, PhysicsWorld, RigidBodyDesc, Shape};
    use spawn_core::{Transform2D, Vec2};

    let mut w = PhysicsWorld::new(PhysicsConfig::default()).unwrap();
    let floor = w.add_rigid_body(
        RigidBodyDesc::fixed()
            .with_transform(Transform2D::from_translation(Vec2::new(0.0, s.floor_y))),
    );
    w.add_collider(
        floor,
        ColliderDesc::new(Shape::Cuboid {
            half_extents: Vec2::new(s.floor_half, 0.5),
        })
        .with_restitution(s.restitution),
    )
    .unwrap();
    let ball = w.add_rigid_body(
        RigidBodyDesc::dynamic()
            .with_transform(Transform2D::from_translation(Vec2::new(0.0, s.drop_height))),
    );
    w.add_collider(
        ball,
        ColliderDesc::new(Shape::Ball { radius: s.radius }).with_restitution(s.restitution),
    )
    .unwrap();
    for _ in 0..s.ticks {
        w.step();
    }
    let t = w.body_transform(ball).unwrap().translation;
    (t.x, t.y)
}

#[test]
fn free_fall_and_bounce_parity() {
    for s in SCENARIOS {
        let (x3, y3) = run_3d(s);
        let (x2, y2) = run_2d(s);
        assert!(
            x2.approx_eq(x3, 1e-3),
            "x parity: 2d {x2} vs 3d {x3} (h {})",
            s.drop_height
        );
        assert!(
            y2.approx_eq(y3, 1e-3),
            "y parity: 2d {y2} vs 3d {y3} (h {})",
            s.drop_height
        );
    }
}
