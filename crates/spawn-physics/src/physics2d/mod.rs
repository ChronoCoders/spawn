//! 2D rigid-body physics over `Vec2`/`f32` rotation/`Transform2D`.

mod body;
mod collider;
mod convert;
mod joint;
mod query;
mod world;

pub use body::{LockFlags, MassProperties, RigidBodyDesc, Velocity};
pub use collider::{ColliderDesc, Shape};
pub use joint::{FixedJoint, RevoluteJoint};
pub use query::{Ray, RayHit};
pub use world::{PhysicsConfig, PhysicsWorld};

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shared::{CollisionEvent, QueryFilter};
    use spawn_core::{ApproxEq, Transform2D, Vec2};

    fn world() -> PhysicsWorld {
        PhysicsWorld::new(PhysicsConfig::default()).unwrap()
    }

    fn drop_ball(w: &mut PhysicsWorld, y: f32, restitution: f32) -> crate::RigidBodyHandle {
        let body = w.add_rigid_body(
            RigidBodyDesc::dynamic()
                .with_transform(Transform2D::from_translation(Vec2::new(0.0, y))),
        );
        w.add_collider(
            body,
            ColliderDesc::new(Shape::Ball { radius: 0.5 }).with_restitution(restitution),
        )
        .unwrap();
        body
    }

    #[test]
    fn config_validation() {
        assert!(PhysicsWorld::new(PhysicsConfig {
            fixed_timestep: 0.0,
            ..Default::default()
        })
        .is_err());
        assert!(PhysicsWorld::new(PhysicsConfig {
            gravity: Vec2::new(f32::NAN, 0.0),
            ..Default::default()
        })
        .is_err());
        assert!(PhysicsWorld::new(PhysicsConfig::default()).is_ok());
    }

    #[test]
    fn free_fall_matches_analytic() {
        let mut w = world();
        let dt = w.config().fixed_timestep;
        let y0 = 100.0;
        let body = w.add_rigid_body(
            RigidBodyDesc::dynamic()
                .with_transform(Transform2D::from_translation(Vec2::new(0.0, y0))),
        );
        w.add_collider(body, ColliderDesc::new(Shape::Ball { radius: 0.5 }))
            .unwrap();
        let n: u32 = 60;
        for _ in 0..n {
            w.step();
        }
        // Discrete symplectic Euler closed form over Rapier's small-steps
        // solver: each tick is integrated as `num_solver_iterations` (default 4)
        // substeps of h = dt/4, so after n ticks (m = 4n substeps):
        // y_n = y0 - g*h²*m(m+1)/2. This is exactly what Rapier computes, so
        // the bound is tight; the continuous 0.5*g*t² form is NOT the reference.
        let g = 9.81;
        let substeps = 4.0;
        let h = dt / substeps;
        let m = n as f32 * substeps;
        let reference = y0 - g * h * h * m * (m + 1.0) / 2.0;
        let actual = w.body_transform(body).unwrap().translation.y;
        assert!(
            (actual - reference).abs() < 1e-3,
            "actual {actual} reference {reference}"
        );
    }

    #[test]
    fn restitution_no_bounce_rests() {
        let mut w = world();
        let floor = w.add_rigid_body(RigidBodyDesc::fixed());
        w.add_collider(
            floor,
            ColliderDesc::new(Shape::Cuboid {
                half_extents: Vec2::new(50.0, 0.5),
            })
            .with_restitution(0.0),
        )
        .unwrap();
        let ball = drop_ball(&mut w, 5.0, 0.0);
        for _ in 0..400 {
            w.step();
        }
        let y = w.body_transform(ball).unwrap().translation.y;
        assert!(y.approx_eq(1.0, 0.2), "rest y {y}");
    }

    #[test]
    fn linear_damping_decays_velocity() {
        let mut w = world();
        w.set_gravity(Vec2::ZERO);
        let body = w.add_rigid_body(
            RigidBodyDesc::dynamic()
                .with_velocity(Velocity {
                    linear: Vec2::new(10.0, 0.0),
                    angular: 0.0,
                })
                .with_linear_damping(2.0),
        );
        w.add_collider(body, ColliderDesc::new(Shape::Ball { radius: 0.5 }))
            .unwrap();
        let mut prev = w.body_velocity(body).unwrap().linear.x;
        for _ in 0..30 {
            w.step();
            let now = w.body_velocity(body).unwrap().linear.x;
            assert!(now < prev + 1e-6);
            prev = now;
        }
        assert!(prev < 10.0);
    }

    #[test]
    fn ray_cast_hit_and_miss() {
        let mut w = world();
        let body = w.add_rigid_body(
            RigidBodyDesc::fixed()
                .with_transform(Transform2D::from_translation(Vec2::new(10.0, 0.0))),
        );
        let col = w
            .add_collider(body, ColliderDesc::new(Shape::Ball { radius: 1.0 }))
            .unwrap();
        w.step();
        let hit = w
            .ray_cast(Ray::new(Vec2::ZERO, Vec2::X), 100.0, QueryFilter::default())
            .unwrap();
        assert_eq!(hit.collider, col);
        assert!(hit.toi.approx_eq(9.0, 1e-2));
        assert!(hit.normal.approx_eq(Vec2::NEG_X, 1e-2));

        assert!(w
            .ray_cast(Ray::new(Vec2::ZERO, Vec2::Y), 100.0, QueryFilter::default())
            .is_none());
        assert!(w
            .ray_cast(Ray::new(Vec2::ZERO, Vec2::X), 1.0, QueryFilter::default())
            .is_none());
        assert!(w
            .ray_cast(
                Ray::new(Vec2::ZERO, Vec2::ZERO),
                100.0,
                QueryFilter::default()
            )
            .is_none());
    }

    #[test]
    fn shape_overlap() {
        let mut w = world();
        let body = w.add_rigid_body(RigidBodyDesc::fixed());
        let col = w
            .add_collider(body, ColliderDesc::new(Shape::Ball { radius: 1.0 }))
            .unwrap();
        w.step();
        let mut out = Vec::new();
        w.intersections_with_shape(
            &Shape::Ball { radius: 1.0 },
            Transform2D::IDENTITY,
            QueryFilter::default(),
            &mut out,
        );
        assert_eq!(out, vec![col]);
        w.intersections_with_shape(
            &Shape::Ball { radius: 0.5 },
            Transform2D::from_translation(Vec2::new(100.0, 0.0)),
            QueryFilter::default(),
            &mut out,
        );
        assert!(out.is_empty());
    }

    #[test]
    fn sensor_events() {
        let mut w = world();
        w.set_gravity(Vec2::ZERO);
        let sensor_body = w.add_rigid_body(RigidBodyDesc::fixed());
        w.add_collider(
            sensor_body,
            ColliderDesc::new(Shape::Cuboid {
                half_extents: Vec2::splat(1.0),
            })
            .as_sensor(true),
        )
        .unwrap();
        let mover = w.add_rigid_body(
            RigidBodyDesc::kinematic()
                .with_transform(Transform2D::from_translation(Vec2::new(-5.0, 0.0)))
                .with_velocity(Velocity {
                    linear: Vec2::new(5.0, 0.0),
                    angular: 0.0,
                }),
        );
        w.add_collider(mover, ColliderDesc::new(Shape::Ball { radius: 0.5 }))
            .unwrap();
        let mut started = 0;
        let mut stopped = 0;
        for _ in 0..120 {
            for ev in w.step() {
                match ev {
                    CollisionEvent::Started(_, _) => started += 1,
                    CollisionEvent::Stopped(_, _) => stopped += 1,
                }
            }
        }
        assert_eq!(started, 1);
        assert_eq!(stopped, 1);
        assert!(w.body_transform(mover).unwrap().translation.x > 0.0);
    }

    #[test]
    fn solid_collision_events_ordered() {
        let mut w = world();
        let floor = w.add_rigid_body(RigidBodyDesc::fixed());
        w.add_collider(
            floor,
            ColliderDesc::new(Shape::Cuboid {
                half_extents: Vec2::new(50.0, 0.5),
            }),
        )
        .unwrap();
        drop_ball(&mut w, 3.0, 0.0);
        let mut saw_started = false;
        for _ in 0..200 {
            for ev in w.step() {
                if let CollisionEvent::Started(a, b) = ev {
                    assert!((a.index, a.generation) <= (b.index, b.generation));
                    saw_started = true;
                }
            }
        }
        assert!(saw_started);
    }

    #[test]
    fn fixed_joint_keeps_relative_pose() {
        let mut w = world();
        let anchor = w.add_rigid_body(RigidBodyDesc::fixed());
        let hanging = w.add_rigid_body(
            RigidBodyDesc::dynamic()
                .with_transform(Transform2D::from_translation(Vec2::new(0.0, -2.0))),
        );
        w.add_collider(hanging, ColliderDesc::new(Shape::Ball { radius: 0.5 }))
            .unwrap();
        w.add_fixed_joint(
            anchor,
            hanging,
            FixedJoint {
                local_anchor_b: Vec2::new(0.0, 2.0),
                ..Default::default()
            },
        )
        .unwrap();
        for _ in 0..120 {
            w.step();
        }
        let y = w.body_transform(hanging).unwrap().translation.y;
        assert!(y.approx_eq(-2.0, 0.2), "joint drifted: {y}");
    }

    #[test]
    fn revolute_joint_constrains_motion() {
        let mut w = world();
        let anchor = w.add_rigid_body(RigidBodyDesc::fixed());
        let arm = w.add_rigid_body(
            RigidBodyDesc::dynamic()
                .with_transform(Transform2D::from_translation(Vec2::new(1.0, 0.0))),
        );
        w.add_collider(arm, ColliderDesc::new(Shape::Ball { radius: 0.2 }))
            .unwrap();
        w.add_revolute_joint(
            anchor,
            arm,
            RevoluteJoint {
                local_anchor_a: Vec2::ZERO,
                local_anchor_b: Vec2::new(-1.0, 0.0),
                limits: None,
            },
        )
        .unwrap();
        for _ in 0..60 {
            w.step();
        }
        let dist = w.body_transform(arm).unwrap().translation.length();
        assert!(dist.approx_eq(1.0, 0.1), "hinge radius drifted: {dist}");
    }

    #[test]
    fn handle_invalidation() {
        let mut w = world();
        let body = w.add_rigid_body(RigidBodyDesc::dynamic());
        let col = w
            .add_collider(body, ColliderDesc::new(Shape::Ball { radius: 0.5 }))
            .unwrap();
        assert!(w.remove_rigid_body(body));
        assert!(!w.remove_rigid_body(body));
        assert!(w.body_transform(body).is_none());
        assert!(!w.set_body_velocity(body, Velocity::default()));
        assert!(w.body_velocity(body).is_none());
        assert!(!w.apply_force(body, Vec2::Y));
        assert_eq!(
            w.add_collider(body, ColliderDesc::new(Shape::Ball { radius: 0.5 })),
            Err(crate::PhysicsError::InvalidHandle)
        );
        assert!(!w.remove_collider(col));

        let fresh = w.add_rigid_body(RigidBodyDesc::dynamic());
        assert_ne!(fresh, body);
        assert!(w.body_transform(fresh).is_some());
    }

    #[test]
    fn accumulator_runs_and_caps() {
        let mut w = world();
        let dt = w.config().fixed_timestep;
        assert_eq!(w.step_accumulate(2.5 * dt), 2);
        assert!(w.accumulator().approx_eq(0.5 * dt, 1e-6));

        let mut capped = world();
        assert_eq!(
            capped.step_accumulate(100.0 * dt),
            capped.config().max_substeps_per_frame
        );
    }

    #[test]
    fn determinism_bit_identical() {
        fn run() -> (Transform2D, Vec<CollisionEvent>) {
            let mut w = world();
            let floor = w.add_rigid_body(RigidBodyDesc::fixed());
            w.add_collider(
                floor,
                ColliderDesc::new(Shape::Cuboid {
                    half_extents: Vec2::new(50.0, 0.5),
                }),
            )
            .unwrap();
            let ball = w.add_rigid_body(
                RigidBodyDesc::dynamic()
                    .with_transform(Transform2D::from_translation(Vec2::new(0.1, 4.0))),
            );
            w.add_collider(ball, ColliderDesc::new(Shape::Ball { radius: 0.5 }))
                .unwrap();
            let mut events = Vec::new();
            for _ in 0..120 {
                events.extend_from_slice(w.step());
            }
            (w.body_transform(ball).unwrap(), events)
        }
        let (t1, e1) = run();
        let (t2, e2) = run();
        assert_eq!(t1, t2);
        assert_eq!(e1, e2);
    }

    #[test]
    fn convex_hull_rejects_degenerate() {
        let mut w = world();
        let body = w.add_rigid_body(RigidBodyDesc::fixed());
        assert!(matches!(
            w.add_collider(
                body,
                ColliderDesc::new(Shape::ConvexHull {
                    points: vec![Vec2::ZERO, Vec2::X],
                }),
            ),
            Err(crate::PhysicsError::InvalidShape { .. })
        ));
        assert!(w
            .add_collider(
                body,
                ColliderDesc::new(Shape::ConvexHull {
                    points: vec![Vec2::ZERO, Vec2::X, Vec2::Y],
                }),
            )
            .is_ok());
    }
}
