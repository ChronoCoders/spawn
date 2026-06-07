//! The 3D [`PhysicsWorld`]: owns all Rapier state and drives the fixed step.

use std::sync::Mutex;

use rapier3d::na;
use rapier3d::na::Vector3;
use rapier3d::prelude as rp;
use spawn_core::{Transform3D, Vec3};

use super::body::{MassProperties, RigidBodyDesc, Velocity};
use super::collider::{ColliderDesc, Shape};
use super::convert::{
    col_to_handle, handle_to_col, handle_to_joint, handle_to_rb, iso_to_transform, joint_to_handle,
    na_to_vec, point_to_vec, quat_to_na, rb_to_handle, transform_to_iso, vec_to_na,
};
use super::joint::{FixedJoint, RevoluteJoint};
use super::query::{Ray, RayHit};
use crate::error::{PhysicsError, PhysicsResult};
use crate::handles::{ColliderHandle, JointHandle, RigidBodyHandle};
use crate::shared::{order_pair, BodyType, CollisionEvent, CollisionGroups, QueryFilter};

/// Configuration for a [`PhysicsWorld`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PhysicsConfig {
    pub gravity: Vec3,
    /// Seconds per tick; must be `> 0`.
    pub fixed_timestep: f32,
    /// Upper bound on ticks drained per [`PhysicsWorld::step_accumulate`] call.
    pub max_substeps_per_frame: u32,
}

impl Default for PhysicsConfig {
    fn default() -> Self {
        Self {
            gravity: Vec3::new(0.0, -9.81, 0.0),
            fixed_timestep: 1.0 / 60.0,
            max_substeps_per_frame: 8,
        }
    }
}

/// Collects Rapier collision events during a single pipeline step.
struct EventCollector {
    events: Mutex<Vec<CollisionEvent>>,
}

impl EventCollector {
    fn new() -> Self {
        Self {
            events: Mutex::new(Vec::new()),
        }
    }

    fn clear(&self) {
        if let Ok(mut events) = self.events.lock() {
            events.clear();
        }
    }

    fn drain_into(&self, out: &mut Vec<CollisionEvent>) {
        if let Ok(mut events) = self.events.lock() {
            out.append(&mut events);
        }
    }
}

impl rp::EventHandler for EventCollector {
    fn handle_collision_event(
        &self,
        _bodies: &rp::RigidBodySet,
        _colliders: &rp::ColliderSet,
        event: rp::CollisionEvent,
        _contact_pair: Option<&rp::ContactPair>,
    ) {
        let mapped = match event {
            rp::CollisionEvent::Started(a, b, _) => {
                let (a, b) = order_pair(col_to_handle(a), col_to_handle(b));
                CollisionEvent::Started(a, b)
            }
            rp::CollisionEvent::Stopped(a, b, _) => {
                let (a, b) = order_pair(col_to_handle(a), col_to_handle(b));
                CollisionEvent::Stopped(a, b)
            }
        };
        if let Ok(mut events) = self.events.lock() {
            events.push(mapped);
        }
    }

    fn handle_contact_force_event(
        &self,
        _dt: f32,
        _bodies: &rp::RigidBodySet,
        _colliders: &rp::ColliderSet,
        _contact_pair: &rp::ContactPair,
        _total_force_magnitude: f32,
    ) {
    }
}

/// Owns all Rapier simulation state and advances it in fixed ticks.
pub struct PhysicsWorld {
    config: PhysicsConfig,
    integration: rp::IntegrationParameters,
    pipeline: rp::PhysicsPipeline,
    query_pipeline: rp::QueryPipeline,
    islands: rp::IslandManager,
    broad_phase: rp::DefaultBroadPhase,
    narrow_phase: rp::NarrowPhase,
    bodies: rp::RigidBodySet,
    colliders: rp::ColliderSet,
    impulse_joints: rp::ImpulseJointSet,
    multibody_joints: rp::MultibodyJointSet,
    ccd_solver: rp::CCDSolver,
    accumulator: f32,
    collector: EventCollector,
    event_buffer: Vec<CollisionEvent>,
}

fn interaction_groups(groups: CollisionGroups) -> rp::InteractionGroups {
    rp::InteractionGroups::new(
        rp::Group::from_bits_truncate(groups.memberships),
        rp::Group::from_bits_truncate(groups.filter),
    )
}

impl PhysicsWorld {
    /// Builds an empty world. Rejects a non-positive timestep or non-finite gravity.
    pub fn new(config: PhysicsConfig) -> PhysicsResult<Self> {
        if config.fixed_timestep <= 0.0 || config.fixed_timestep.is_nan() {
            return Err(PhysicsError::InvalidConfig {
                context: "fixed_timestep must be > 0",
            });
        }
        if !config.gravity.is_finite() {
            return Err(PhysicsError::InvalidConfig {
                context: "gravity must be finite",
            });
        }
        let integration = rp::IntegrationParameters {
            dt: config.fixed_timestep,
            ..rp::IntegrationParameters::default()
        };
        Ok(Self {
            config,
            integration,
            pipeline: rp::PhysicsPipeline::new(),
            query_pipeline: rp::QueryPipeline::new(),
            islands: rp::IslandManager::new(),
            broad_phase: rp::DefaultBroadPhase::new(),
            narrow_phase: rp::NarrowPhase::new(),
            bodies: rp::RigidBodySet::new(),
            colliders: rp::ColliderSet::new(),
            impulse_joints: rp::ImpulseJointSet::new(),
            multibody_joints: rp::MultibodyJointSet::new(),
            ccd_solver: rp::CCDSolver::new(),
            accumulator: 0.0,
            collector: EventCollector::new(),
            event_buffer: Vec::new(),
        })
    }

    pub fn config(&self) -> PhysicsConfig {
        self.config
    }

    pub fn set_gravity(&mut self, gravity: Vec3) {
        self.config.gravity = gravity;
    }

    fn gravity_na(&self) -> Vector3<f32> {
        vec_to_na(self.config.gravity)
    }

    /// Advances exactly one fixed tick and returns the events generated this
    /// tick, borrowed from an internal buffer (valid until the next mutation).
    pub fn step(&mut self) -> &[CollisionEvent] {
        self.event_buffer.clear();
        self.collector.clear();
        let gravity = self.gravity_na();
        let hooks: &dyn rp::PhysicsHooks = &();
        self.pipeline.step(
            &gravity,
            &self.integration,
            &mut self.islands,
            &mut self.broad_phase,
            &mut self.narrow_phase,
            &mut self.bodies,
            &mut self.colliders,
            &mut self.impulse_joints,
            &mut self.multibody_joints,
            &mut self.ccd_solver,
            Some(&mut self.query_pipeline),
            hooks,
            &self.collector,
        );
        self.query_pipeline.update(&self.colliders);
        self.collector.drain_into(&mut self.event_buffer);
        &self.event_buffer
    }

    /// Drains the accumulator: runs `floor(acc / dt)` ticks, capped at
    /// `max_substeps_per_frame`, and returns the number of ticks run.
    ///
    /// APPENDS every internal tick's collision events to `events` in tick
    /// order, preserving cross-tick `Started`/`Stopped` pairs. The caller owns
    /// clearing; a reused `Vec` keeps this allocation-free in steady state.
    pub fn step_accumulate(&mut self, frame_dt: f32, events: &mut Vec<CollisionEvent>) -> u32 {
        if !frame_dt.is_finite() || frame_dt <= 0.0 {
            return 0;
        }
        self.accumulator += frame_dt;
        let dt = self.config.fixed_timestep;
        let mut ticks = 0;
        while self.accumulator >= dt && ticks < self.config.max_substeps_per_frame {
            self.step();
            events.extend_from_slice(&self.event_buffer);
            self.accumulator -= dt;
            ticks += 1;
        }
        ticks
    }

    pub fn accumulator(&self) -> f32 {
        self.accumulator
    }

    fn rapier_body_type(t: BodyType) -> rp::RigidBodyType {
        match t {
            BodyType::Dynamic => rp::RigidBodyType::Dynamic,
            BodyType::Kinematic => rp::RigidBodyType::KinematicVelocityBased,
            BodyType::Fixed => rp::RigidBodyType::Fixed,
        }
    }

    pub fn add_rigid_body(&mut self, desc: RigidBodyDesc) -> RigidBodyHandle {
        let mut builder = rp::RigidBodyBuilder::new(Self::rapier_body_type(desc.body_type))
            .position(transform_to_iso(desc.transform))
            .linvel(vec_to_na(desc.velocity.linear))
            .angvel(vec_to_na(desc.velocity.angular))
            .linear_damping(desc.linear_damping)
            .angular_damping(desc.angular_damping)
            .ccd_enabled(desc.ccd_enabled)
            .can_sleep(desc.can_sleep);
        builder = builder
            .enabled_translations(
                !desc.locks.translation.x,
                !desc.locks.translation.y,
                !desc.locks.translation.z,
            )
            .enabled_rotations(
                !desc.locks.rotation.x,
                !desc.locks.rotation.y,
                !desc.locks.rotation.z,
            );
        if let MassProperties::Mass(mass) = desc.mass {
            builder = builder.additional_mass(mass);
        }
        rb_to_handle(self.bodies.insert(builder.build()))
    }

    fn build_collider(desc: &ColliderDesc) -> PhysicsResult<rp::Collider> {
        let mut builder = match &desc.shape {
            Shape::Ball { radius } => rp::ColliderBuilder::ball(*radius),
            Shape::Cuboid { half_extents } => {
                rp::ColliderBuilder::cuboid(half_extents.x, half_extents.y, half_extents.z)
            }
            Shape::Capsule {
                half_height,
                radius,
            } => rp::ColliderBuilder::capsule_y(*half_height, *radius),
            Shape::ConvexHull { points } => {
                // parry panics (rather than returning None) on fewer than 4 points.
                if points.len() < 4 {
                    return Err(PhysicsError::InvalidShape {
                        context: "convex hull needs at least 4 points",
                    });
                }
                let pts: Vec<na::Point3<f32>> = points
                    .iter()
                    .map(|p| na::Point3::new(p.x, p.y, p.z))
                    .collect();
                rp::ColliderBuilder::convex_hull(&pts).ok_or(PhysicsError::InvalidShape {
                    context: "degenerate convex hull point set",
                })?
            }
        };
        builder = builder
            .position(transform_to_iso(desc.local_transform))
            .friction(desc.friction)
            .restitution(desc.restitution)
            .density(desc.density)
            .sensor(desc.is_sensor)
            .collision_groups(interaction_groups(desc.groups))
            .active_events(rp::ActiveEvents::COLLISION_EVENTS)
            .active_collision_types(rp::ActiveCollisionTypes::all());
        Ok(builder.build())
    }

    pub fn add_collider(
        &mut self,
        body: RigidBodyHandle,
        desc: ColliderDesc,
    ) -> PhysicsResult<ColliderHandle> {
        if !self.bodies.contains(handle_to_rb(body)) {
            return Err(PhysicsError::InvalidHandle);
        }
        let collider = Self::build_collider(&desc)?;
        let handle =
            self.colliders
                .insert_with_parent(collider, handle_to_rb(body), &mut self.bodies);
        Ok(col_to_handle(handle))
    }

    pub fn add_collider_standalone(&mut self, desc: ColliderDesc) -> PhysicsResult<ColliderHandle> {
        let collider = Self::build_collider(&desc)?;
        Ok(col_to_handle(self.colliders.insert(collider)))
    }

    pub fn remove_rigid_body(&mut self, handle: RigidBodyHandle) -> bool {
        self.bodies
            .remove(
                handle_to_rb(handle),
                &mut self.islands,
                &mut self.colliders,
                &mut self.impulse_joints,
                &mut self.multibody_joints,
                true,
            )
            .is_some()
    }

    pub fn remove_collider(&mut self, handle: ColliderHandle) -> bool {
        self.colliders
            .remove(
                handle_to_col(handle),
                &mut self.islands,
                &mut self.bodies,
                true,
            )
            .is_some()
    }

    pub fn add_fixed_joint(
        &mut self,
        a: RigidBodyHandle,
        b: RigidBodyHandle,
        j: FixedJoint,
    ) -> PhysicsResult<JointHandle> {
        if !self.bodies.contains(handle_to_rb(a)) || !self.bodies.contains(handle_to_rb(b)) {
            return Err(PhysicsError::InvalidHandle);
        }
        let data = rp::FixedJointBuilder::new()
            .local_frame1(na::Isometry3::from_parts(
                na::Translation3::from(vec_to_na(j.local_anchor_a)),
                quat_to_na(j.local_frame_a),
            ))
            .local_frame2(na::Isometry3::from_parts(
                na::Translation3::from(vec_to_na(j.local_anchor_b)),
                quat_to_na(j.local_frame_b),
            ));
        let handle =
            self.impulse_joints
                .insert(handle_to_rb(a), handle_to_rb(b), data.build(), true);
        Ok(joint_to_handle(handle))
    }

    pub fn add_revolute_joint(
        &mut self,
        a: RigidBodyHandle,
        b: RigidBodyHandle,
        j: RevoluteJoint,
    ) -> PhysicsResult<JointHandle> {
        if !self.bodies.contains(handle_to_rb(a)) || !self.bodies.contains(handle_to_rb(b)) {
            return Err(PhysicsError::InvalidHandle);
        }
        let axis = j.axis.normalize().ok_or(PhysicsError::InvalidJoint {
            context: "revolute axis must be non-zero",
        })?;
        let unit = na::Unit::new_normalize(vec_to_na(axis));
        let mut builder = rp::RevoluteJointBuilder::new(unit)
            .local_anchor1(na::Point3::from(vec_to_na(j.local_anchor_a)))
            .local_anchor2(na::Point3::from(vec_to_na(j.local_anchor_b)));
        if let Some((min, max)) = j.limits {
            builder = builder.limits([min, max]);
        }
        let handle =
            self.impulse_joints
                .insert(handle_to_rb(a), handle_to_rb(b), builder.build(), true);
        Ok(joint_to_handle(handle))
    }

    pub fn remove_joint(&mut self, handle: JointHandle) -> bool {
        self.impulse_joints
            .remove(handle_to_joint(handle), true)
            .is_some()
    }

    pub fn body_transform(&self, h: RigidBodyHandle) -> Option<Transform3D> {
        self.bodies
            .get(handle_to_rb(h))
            .map(|b| iso_to_transform(b.position()))
    }

    pub fn set_body_transform(&mut self, h: RigidBodyHandle, t: Transform3D) -> bool {
        match self.bodies.get_mut(handle_to_rb(h)) {
            Some(body) => {
                body.set_position(transform_to_iso(t), true);
                true
            }
            None => false,
        }
    }

    pub fn body_velocity(&self, h: RigidBodyHandle) -> Option<Velocity> {
        self.bodies.get(handle_to_rb(h)).map(|b| Velocity {
            linear: na_to_vec(*b.linvel()),
            angular: na_to_vec(*b.angvel()),
        })
    }

    pub fn set_body_velocity(&mut self, h: RigidBodyHandle, v: Velocity) -> bool {
        match self.bodies.get_mut(handle_to_rb(h)) {
            Some(body) => {
                body.set_linvel(vec_to_na(v.linear), true);
                body.set_angvel(vec_to_na(v.angular), true);
                true
            }
            None => false,
        }
    }

    pub fn apply_force(&mut self, h: RigidBodyHandle, force: Vec3) -> bool {
        match self.bodies.get_mut(handle_to_rb(h)) {
            Some(body) => {
                body.add_force(vec_to_na(force), true);
                true
            }
            None => false,
        }
    }

    pub fn apply_impulse(&mut self, h: RigidBodyHandle, impulse: Vec3) -> bool {
        match self.bodies.get_mut(handle_to_rb(h)) {
            Some(body) => {
                body.apply_impulse(vec_to_na(impulse), true);
                true
            }
            None => false,
        }
    }

    pub fn apply_torque(&mut self, h: RigidBodyHandle, torque: Vec3) -> bool {
        match self.bodies.get_mut(handle_to_rb(h)) {
            Some(body) => {
                body.add_torque(vec_to_na(torque), true);
                true
            }
            None => false,
        }
    }

    fn rapier_filter(&self, filter: &QueryFilter) -> rp::QueryFilter<'_> {
        let mut qf = rp::QueryFilter::new().groups(interaction_groups(filter.groups));
        if !filter.include_sensors {
            qf = qf.exclude_sensors();
        }
        if let Some(body) = filter.exclude_body {
            qf = qf.exclude_rigid_body(handle_to_rb(body));
        }
        if let Some(col) = filter.exclude_collider {
            qf = qf.exclude_collider(handle_to_col(col));
        }
        qf
    }

    pub fn ray_cast(&self, ray: Ray, max_toi: f32, filter: QueryFilter) -> Option<RayHit> {
        let dir = ray.dir.normalize()?;
        let rapier_ray = rp::Ray::new(na::Point3::from(vec_to_na(ray.origin)), vec_to_na(dir));
        let qf = self.rapier_filter(&filter);
        let (handle, intersection) = self.query_pipeline.cast_ray_and_get_normal(
            &self.bodies,
            &self.colliders,
            &rapier_ray,
            max_toi,
            true,
            qf,
        )?;
        let point = rapier_ray.point_at(intersection.time_of_impact);
        Some(RayHit {
            collider: col_to_handle(handle),
            toi: intersection.time_of_impact,
            point: point_to_vec(point),
            normal: na_to_vec(intersection.normal),
        })
    }

    fn build_query_shape(shape: &Shape) -> PhysicsResult<rp::SharedShape> {
        match shape {
            Shape::Ball { radius } => Ok(rp::SharedShape::ball(*radius)),
            Shape::Cuboid { half_extents } => Ok(rp::SharedShape::cuboid(
                half_extents.x,
                half_extents.y,
                half_extents.z,
            )),
            Shape::Capsule {
                half_height,
                radius,
            } => Ok(rp::SharedShape::capsule_y(*half_height, *radius)),
            Shape::ConvexHull { points } => {
                // parry panics (rather than returning None) on fewer than 4 points.
                if points.len() < 4 {
                    return Err(PhysicsError::InvalidShape {
                        context: "convex hull needs at least 4 points",
                    });
                }
                let pts: Vec<na::Point3<f32>> = points
                    .iter()
                    .map(|p| na::Point3::new(p.x, p.y, p.z))
                    .collect();
                rp::SharedShape::convex_hull(&pts).ok_or(PhysicsError::InvalidShape {
                    context: "degenerate convex hull point set",
                })
            }
        }
    }

    pub fn intersections_with_shape(
        &self,
        shape: &Shape,
        pose: Transform3D,
        filter: QueryFilter,
        out: &mut Vec<ColliderHandle>,
    ) {
        out.clear();
        let shape = match Self::build_query_shape(shape) {
            Ok(s) => s,
            Err(_) => return,
        };
        let iso = transform_to_iso(pose);
        let qf = self.rapier_filter(&filter);
        self.query_pipeline.intersections_with_shape(
            &self.bodies,
            &self.colliders,
            &iso,
            shape.as_ref(),
            qf,
            |handle| {
                out.push(col_to_handle(handle));
                true
            },
        );
    }
}
