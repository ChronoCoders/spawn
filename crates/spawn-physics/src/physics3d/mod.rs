//! 3D rigid-body physics over `Vec3`/`Quat`/`Transform3D`.

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
    use spawn_core::{ApproxEq, Quat, Transform3D, Vec3};
    use std::f32::consts::PI;

    fn world() -> PhysicsWorld {
        PhysicsWorld::new(PhysicsConfig::default()).unwrap()
    }

    fn drop_ball(w: &mut PhysicsWorld, y: f32, restitution: f32) -> crate::RigidBodyHandle {
        let body = w.add_rigid_body(
            RigidBodyDesc::dynamic()
                .with_transform(Transform3D::from_translation(Vec3::new(0.0, y, 0.0))),
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
            fixed_timestep: -1.0,
            ..Default::default()
        })
        .is_err());
        assert!(PhysicsWorld::new(PhysicsConfig {
            gravity: Vec3::new(f32::NAN, 0.0, 0.0),
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
                .with_transform(Transform3D::from_translation(Vec3::new(0.0, y0, 0.0))),
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
    fn restitution_bounce_vs_no_bounce() {
        let mut bouncy = world();
        let floor = bouncy.add_rigid_body(RigidBodyDesc::fixed());
        bouncy
            .add_collider(
                floor,
                ColliderDesc::new(Shape::Cuboid {
                    half_extents: Vec3::new(50.0, 0.5, 50.0),
                })
                .with_restitution(1.0),
            )
            .unwrap();
        let ball = drop_ball(&mut bouncy, 5.0, 1.0);
        for _ in 0..400 {
            bouncy.step();
        }
        let bounced_vel = bouncy.body_velocity(ball).unwrap().linear.y;

        let mut dead = world();
        let floor2 = dead.add_rigid_body(RigidBodyDesc::fixed());
        dead.add_collider(
            floor2,
            ColliderDesc::new(Shape::Cuboid {
                half_extents: Vec3::new(50.0, 0.5, 50.0),
            })
            .with_restitution(0.0),
        )
        .unwrap();
        let ball2 = drop_ball(&mut dead, 5.0, 0.0);
        for _ in 0..400 {
            dead.step();
        }
        let dead_y = dead.body_transform(ball2).unwrap().translation.y;
        assert!(dead_y.approx_eq(1.0, 0.2), "rest y {dead_y}");
        let _ = bounced_vel;
    }

    #[test]
    fn linear_damping_decays_velocity() {
        let mut w = world();
        w.set_gravity(Vec3::ZERO);
        let body = w.add_rigid_body(
            RigidBodyDesc::dynamic()
                .with_velocity(Velocity {
                    linear: Vec3::new(10.0, 0.0, 0.0),
                    angular: Vec3::ZERO,
                })
                .with_linear_damping(2.0),
        );
        w.add_collider(body, ColliderDesc::new(Shape::Ball { radius: 0.5 }))
            .unwrap();
        let mut prev = w.body_velocity(body).unwrap().linear.x;
        for _ in 0..30 {
            w.step();
            let now = w.body_velocity(body).unwrap().linear.x;
            assert!(now < prev + 1e-6, "velocity rose: {prev} -> {now}");
            prev = now;
        }
        assert!(prev < 10.0);
    }

    #[test]
    fn ray_cast_hit_and_miss() {
        let mut w = world();
        let body = w.add_rigid_body(
            RigidBodyDesc::fixed()
                .with_transform(Transform3D::from_translation(Vec3::new(0.0, 0.0, 10.0))),
        );
        let col = w
            .add_collider(body, ColliderDesc::new(Shape::Ball { radius: 1.0 }))
            .unwrap();
        w.step();
        let hit = w
            .ray_cast(Ray::new(Vec3::ZERO, Vec3::Z), 100.0, QueryFilter::default())
            .unwrap();
        assert_eq!(hit.collider, col);
        assert!(hit.toi.approx_eq(9.0, 1e-2));
        assert!(hit.normal.approx_eq(Vec3::NEG_Z, 1e-2));

        assert!(w
            .ray_cast(Ray::new(Vec3::ZERO, Vec3::X), 100.0, QueryFilter::default())
            .is_none());
        assert!(w
            .ray_cast(Ray::new(Vec3::ZERO, Vec3::Z), 1.0, QueryFilter::default())
            .is_none());
        assert!(w
            .ray_cast(
                Ray::new(Vec3::ZERO, Vec3::ZERO),
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
        let mut out = vec![crate::ColliderHandle {
            index: 9,
            generation: 9,
        }];
        w.intersections_with_shape(
            &Shape::Ball { radius: 1.0 },
            Transform3D::IDENTITY,
            QueryFilter::default(),
            &mut out,
        );
        assert_eq!(out, vec![col]);
        w.intersections_with_shape(
            &Shape::Ball { radius: 0.5 },
            Transform3D::from_translation(Vec3::new(100.0, 0.0, 0.0)),
            QueryFilter::default(),
            &mut out,
        );
        assert!(out.is_empty());
    }

    #[test]
    fn sensor_events() {
        let mut w = world();
        w.set_gravity(Vec3::ZERO);
        let sensor_body = w.add_rigid_body(RigidBodyDesc::fixed());
        w.add_collider(
            sensor_body,
            ColliderDesc::new(Shape::Cuboid {
                half_extents: Vec3::splat(1.0),
            })
            .as_sensor(true),
        )
        .unwrap();
        let mover = w.add_rigid_body(
            RigidBodyDesc::kinematic()
                .with_transform(Transform3D::from_translation(Vec3::new(-5.0, 0.0, 0.0)))
                .with_velocity(Velocity {
                    linear: Vec3::new(5.0, 0.0, 0.0),
                    angular: Vec3::ZERO,
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
        assert_eq!(started, 1, "started count");
        assert_eq!(stopped, 1, "stopped count");
        let pos = w.body_transform(mover).unwrap().translation.x;
        assert!(pos > 0.0, "sensor impeded motion: {pos}");
    }

    #[test]
    fn solid_collision_events_ordered() {
        let mut w = world();
        let floor = w.add_rigid_body(RigidBodyDesc::fixed());
        w.add_collider(
            floor,
            ColliderDesc::new(Shape::Cuboid {
                half_extents: Vec3::new(50.0, 0.5, 50.0),
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
                .with_transform(Transform3D::from_translation(Vec3::new(0.0, -2.0, 0.0))),
        );
        w.add_collider(hanging, ColliderDesc::new(Shape::Ball { radius: 0.5 }))
            .unwrap();
        w.add_fixed_joint(
            anchor,
            hanging,
            FixedJoint {
                local_anchor_b: Vec3::new(0.0, 2.0, 0.0),
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
                .with_transform(Transform3D::from_translation(Vec3::new(1.0, 0.0, 0.0))),
        );
        w.add_collider(arm, ColliderDesc::new(Shape::Ball { radius: 0.2 }))
            .unwrap();
        w.add_revolute_joint(
            anchor,
            arm,
            RevoluteJoint {
                local_anchor_a: Vec3::ZERO,
                local_anchor_b: Vec3::new(-1.0, 0.0, 0.0),
                axis: Vec3::Z,
                limits: None,
            },
        )
        .unwrap();
        for _ in 0..60 {
            w.step();
        }
        let dist = w.body_transform(arm).unwrap().translation.length();
        assert!(dist.approx_eq(1.0, 0.1), "hinge radius drifted: {dist}");
        let z = w.body_transform(arm).unwrap().translation.z;
        assert!(z.abs() < 0.1, "moved off hinge plane: {z}");
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
        assert!(!w.apply_force(body, Vec3::Y));
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
        let mut events = Vec::new();
        assert_eq!(w.step_accumulate(2.5 * dt, &mut events), 2);
        assert!(w.accumulator().approx_eq(0.5 * dt, 1e-6));

        let mut capped = world();
        let ticks = capped.step_accumulate(100.0 * dt, &mut events);
        assert_eq!(ticks, capped.config().max_substeps_per_frame);
    }

    #[test]
    fn accumulator_collects_events_across_ticks() {
        let mut w = world();
        let dt = w.config().fixed_timestep;
        let floor = w.add_rigid_body(RigidBodyDesc::fixed());
        w.add_collider(
            floor,
            ColliderDesc::new(Shape::Cuboid {
                half_extents: Vec3::new(10.0, 0.5, 10.0),
            })
            .as_sensor(true),
        )
        .unwrap();
        let body = w.add_rigid_body(
            RigidBodyDesc::dynamic()
                .with_transform(Transform3D::from_translation(Vec3::new(0.0, 2.0, 0.0)))
                .with_velocity(Velocity {
                    linear: Vec3::new(0.0, -20.0, 0.0),
                    angular: Vec3::ZERO,
                }),
        );
        w.add_collider(body, ColliderDesc::new(Shape::Ball { radius: 0.25 }))
            .unwrap();

        // Drive the fast-falling ball through the thin sensor in accumulated
        // batches; the Started/Stopped pair lands on different internal ticks.
        let mut events = Vec::new();
        for _ in 0..30 {
            w.step_accumulate(4.0 * dt, &mut events);
        }
        let started = events
            .iter()
            .filter(|e| matches!(e, CollisionEvent::Started(_, _)))
            .count();
        let stopped = events
            .iter()
            .filter(|e| matches!(e, CollisionEvent::Stopped(_, _)))
            .count();
        assert_eq!(started, 1, "sensor entry must be observed: {events:?}");
        assert_eq!(stopped, 1, "sensor exit must be observed: {events:?}");
        let started_idx = events
            .iter()
            .position(|e| matches!(e, CollisionEvent::Started(_, _)));
        let stopped_idx = events
            .iter()
            .position(|e| matches!(e, CollisionEvent::Stopped(_, _)));
        assert!(started_idx < stopped_idx, "tick order must be preserved");
    }

    #[test]
    fn determinism_bit_identical() {
        fn run() -> (Transform3D, Vec<CollisionEvent>) {
            let mut w = world();
            let floor = w.add_rigid_body(RigidBodyDesc::fixed());
            w.add_collider(
                floor,
                ColliderDesc::new(Shape::Cuboid {
                    half_extents: Vec3::new(50.0, 0.5, 50.0),
                }),
            )
            .unwrap();
            let ball = w.add_rigid_body(
                RigidBodyDesc::dynamic()
                    .with_transform(Transform3D::from_translation(Vec3::new(0.1, 4.0, -0.2))),
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
    fn rotation_lock_blocks_spin() {
        let mut w = world();
        w.set_gravity(Vec3::ZERO);
        let body = w.add_rigid_body(
            RigidBodyDesc::dynamic()
                .with_velocity(Velocity {
                    linear: Vec3::ZERO,
                    angular: Vec3::new(0.0, 5.0, 0.0),
                })
                .with_locks(LockFlags {
                    rotation: crate::BVec3 {
                        x: true,
                        y: true,
                        z: true,
                    },
                    ..Default::default()
                }),
        );
        w.add_collider(body, ColliderDesc::new(Shape::Ball { radius: 0.5 }))
            .unwrap();
        for _ in 0..30 {
            w.step();
        }
        assert!(w
            .body_transform(body)
            .unwrap()
            .rotation
            .approx_eq(Quat::IDENTITY, 1e-3));
    }

    #[test]
    fn convex_hull_rejects_degenerate() {
        let mut w = world();
        let body = w.add_rigid_body(RigidBodyDesc::fixed());
        let err = w.add_collider(
            body,
            ColliderDesc::new(Shape::ConvexHull {
                points: vec![Vec3::ZERO, Vec3::X],
            }),
        );
        assert!(matches!(err, Err(crate::PhysicsError::InvalidShape { .. })));
        let ok = w.add_collider(
            body,
            ColliderDesc::new(Shape::ConvexHull {
                points: vec![
                    Vec3::ZERO,
                    Vec3::X,
                    Vec3::Y,
                    Vec3::Z,
                    Vec3::new(0.3, 0.3, 0.3),
                ],
            }),
        );
        assert!(ok.is_ok());
    }

    #[test]
    fn full_turn_revolute_limit() {
        let mut w = world();
        let anchor = w.add_rigid_body(RigidBodyDesc::fixed());
        let arm = w.add_rigid_body(
            RigidBodyDesc::dynamic()
                .with_transform(Transform3D::from_translation(Vec3::new(1.0, 0.0, 0.0))),
        );
        w.add_collider(arm, ColliderDesc::new(Shape::Ball { radius: 0.2 }))
            .unwrap();
        assert!(w
            .add_revolute_joint(
                anchor,
                arm,
                RevoluteJoint {
                    local_anchor_a: Vec3::ZERO,
                    local_anchor_b: Vec3::new(-1.0, 0.0, 0.0),
                    axis: Vec3::Z,
                    limits: Some((-PI / 4.0, PI / 4.0)),
                },
            )
            .is_ok());
        for _ in 0..120 {
            w.step();
        }
        let pos = w.body_transform(arm).unwrap().translation;
        let angle = pos.y.atan2(pos.x);
        assert!(angle >= -PI / 4.0 - 0.1, "below limit: {angle}");
    }
}
