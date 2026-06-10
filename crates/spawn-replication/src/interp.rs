//! Client-side snapshot interpolation for `SimulatedProxy` entities.
//!
//! Snapshots arrive at the snapshot rate; the client renders **in the past** by an
//! interpolation delay so it always has a snapshot behind and ahead of the render
//! time to interpolate between (default delay = 2 snapshot intervals). Position uses
//! **Hermite** interpolation with the transmitted linear velocity at the endpoints
//! (velocity-continuous); orientation uses **slerp**. Interpolation is presentation
//! only: it writes a separate [`InterpolatedTransform`] component (decision 6 — never
//! an override of the authoritative `Transform3D`).

use std::collections::HashMap;
use std::collections::VecDeque;

use spawn_core::{Quat, Vec3};
use spawn_ecs::Component;

use crate::id::ReplId;
use crate::snapshot::SNAPSHOT_INTERVAL;

/// The interpolated render transform for a simulated-proxy entity. Read by the
/// renderer; the authoritative `Transform3D` is never touched by interpolation.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct InterpolatedTransform {
    /// Interpolated position.
    pub translation: Vec3,
    /// Interpolated orientation (unit).
    pub rotation: Quat,
}

impl Component for InterpolatedTransform {}

/// Default interpolation delay in seconds: two snapshot intervals (decision 5).
pub fn default_interp_delay() -> f64 {
    2.0 * SNAPSHOT_INTERVAL.as_secs_f64()
}

/// One timestamped transform sample (a snapshot's state for one entity).
#[derive(Debug, Clone, Copy)]
struct Sample {
    time: f64,
    translation: Vec3,
    rotation: Quat,
    velocity: Vec3,
}

/// Per-entity bounded ring of recent samples, kept in ascending `time` order.
struct Samples {
    ring: VecDeque<Sample>,
}

const SAMPLE_CAP: usize = 16;

impl Samples {
    fn new() -> Self {
        Self {
            ring: VecDeque::with_capacity(SAMPLE_CAP),
        }
    }

    fn push(&mut self, s: Sample) {
        // Discard a stale or duplicate sample (not strictly newer than the latest),
        // keeping the ring strictly ascending. Snapshots arrive newest-wins, so a
        // not-newer sample is an out-of-order straggler.
        if self.ring.back().is_some_and(|b| b.time >= s.time) {
            return;
        }
        self.ring.push_back(s);
        while self.ring.len() > SAMPLE_CAP {
            self.ring.pop_front();
        }
    }
}

/// The interpolation buffer: per simulated-proxy entity, a ring of recent snapshot
/// samples, sampled at `now - delay`.
pub struct InterpBuffer {
    delay: f64,
    entities: HashMap<ReplId, Samples>,
}

impl InterpBuffer {
    /// A buffer with the given interpolation delay (seconds).
    pub fn new(delay: f64) -> Self {
        Self {
            delay,
            entities: HashMap::new(),
        }
    }

    /// A buffer with the [`default_interp_delay`].
    pub fn with_default_delay() -> Self {
        Self::new(default_interp_delay())
    }

    /// The interpolation delay (seconds).
    pub fn delay(&self) -> f64 {
        self.delay
    }

    /// Record a snapshot sample for `id` at logical `time` (seconds): its position,
    /// orientation, and the transmitted linear velocity used for Hermite endpoints.
    pub fn record(
        &mut self,
        id: ReplId,
        time: f64,
        translation: Vec3,
        rotation: Quat,
        velocity: Vec3,
    ) {
        self.entities
            .entry(id)
            .or_insert_with(Samples::new)
            .push(Sample {
                time,
                translation,
                rotation,
                velocity,
            });
    }

    /// Forget an entity (on despawn).
    pub fn remove(&mut self, id: ReplId) {
        self.entities.remove(&id);
    }

    /// The interpolated transform for `id` at wall time `now` (seconds), i.e. at render
    /// time `now - delay`. `None` if the entity has no samples. With a render time
    /// before the earliest sample the earliest is held; after the latest, the latest is
    /// held (no extrapolation).
    pub fn sample(&self, id: ReplId, now: f64) -> Option<InterpolatedTransform> {
        let samples = self.entities.get(&id)?;
        let ring = &samples.ring;
        let first = ring.front()?;
        let render = now - self.delay;

        if render <= first.time {
            return Some(InterpolatedTransform {
                translation: first.translation,
                rotation: first.rotation,
            });
        }
        let last = ring.back()?;
        if render >= last.time {
            return Some(InterpolatedTransform {
                translation: last.translation,
                rotation: last.rotation,
            });
        }
        // Find the bracket [a, b] with a.time <= render < b.time.
        let mut a = first;
        let mut b = first;
        for w in ring.iter() {
            if w.time <= render {
                a = w;
            } else {
                b = w;
                break;
            }
        }
        Some(hermite_slerp(a, b, render))
    }
}

fn hermite_slerp(a: &Sample, b: &Sample, t: f64) -> InterpolatedTransform {
    let h = (b.time - a.time).max(1e-9);
    let s = (((t - a.time) / h) as f32).clamp(0.0, 1.0);
    let s2 = s * s;
    let s3 = s2 * s;
    // Cubic Hermite basis.
    let h00 = 2.0 * s3 - 3.0 * s2 + 1.0;
    let h10 = s3 - 2.0 * s2 + s;
    let h01 = -2.0 * s3 + 3.0 * s2;
    let h11 = s3 - s2;
    let hf = h as f32;
    let translation = a.translation * h00
        + (a.velocity * hf) * h10
        + b.translation * h01
        + (b.velocity * hf) * h11;
    let rotation = a.rotation.slerp(b.rotation, s);
    InterpolatedTransform {
        translation,
        rotation,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use spawn_core::ApproxEq;

    fn buf() -> InterpBuffer {
        InterpBuffer::new(0.1) // 100ms delay
    }

    #[test]
    fn default_delay_is_two_snapshot_intervals() {
        assert!((default_interp_delay() - 0.1).abs() < 1e-9);
    }

    #[test]
    fn linear_velocity_reproduces_a_straight_line_midpoint() {
        // With endpoint velocities equal to the secant slope, Hermite == linear.
        let mut b = buf();
        let p0 = Vec3::new(0.0, 0.0, 0.0);
        let p1 = Vec3::new(10.0, 0.0, 0.0);
        let dt = 0.5;
        let v = (p1 - p0) * (1.0 / dt as f32); // secant velocity
        b.record(ReplId(1), 0.0, p0, Quat::IDENTITY, v);
        b.record(ReplId(1), dt, p1, Quat::IDENTITY, v);
        // render = now - delay; pick now so render = 0.25 (midpoint of [0, 0.5]).
        let now = 0.25 + b.delay();
        let out = b.sample(ReplId(1), now).unwrap();
        assert!(
            out.translation.approx_eq(Vec3::new(5.0, 0.0, 0.0), 1e-4),
            "got {:?}",
            out.translation
        );
    }

    #[test]
    fn zero_velocity_is_smoothstep_midpoint() {
        let mut b = buf();
        b.record(ReplId(1), 0.0, Vec3::ZERO, Quat::IDENTITY, Vec3::ZERO);
        b.record(
            ReplId(1),
            1.0,
            Vec3::new(8.0, 0.0, 0.0),
            Quat::IDENTITY,
            Vec3::ZERO,
        );
        // render = 0.5 → smoothstep at s=0.5 is 0.5 → midpoint 4.0.
        let out = b.sample(ReplId(1), 0.5 + b.delay()).unwrap();
        assert!(out.translation.approx_eq(Vec3::new(4.0, 0.0, 0.0), 1e-4));
    }

    #[test]
    fn renders_in_the_past_and_holds_at_the_ends() {
        let mut b = buf();
        b.record(
            ReplId(1),
            1.0,
            Vec3::new(1.0, 0.0, 0.0),
            Quat::IDENTITY,
            Vec3::ZERO,
        );
        b.record(
            ReplId(1),
            2.0,
            Vec3::new(2.0, 0.0, 0.0),
            Quat::IDENTITY,
            Vec3::ZERO,
        );
        // now small → render before the first sample → hold the earliest.
        let early = b.sample(ReplId(1), 0.5).unwrap();
        assert!(early.translation.approx_eq(Vec3::new(1.0, 0.0, 0.0), 1e-5));
        // now large → render after the last sample → hold the latest (no extrapolation).
        let late = b.sample(ReplId(1), 100.0).unwrap();
        assert!(late.translation.approx_eq(Vec3::new(2.0, 0.0, 0.0), 1e-5));
    }

    #[test]
    fn slerp_orientation_at_midpoint() {
        let mut b = buf();
        let q0 = Quat::IDENTITY;
        let q1 = Quat::from_rotation_y(std::f32::consts::FRAC_PI_2);
        b.record(ReplId(1), 0.0, Vec3::ZERO, q0, Vec3::ZERO);
        b.record(ReplId(1), 1.0, Vec3::ZERO, q1, Vec3::ZERO);
        let out = b.sample(ReplId(1), 0.5 + b.delay()).unwrap();
        let expected = q0.slerp(q1, 0.5);
        // Compare via the rotation they apply to a vector.
        let v = Vec3::new(1.0, 0.0, 0.0);
        assert!(out.rotation.rotate(v).approx_eq(expected.rotate(v), 1e-4));
    }

    #[test]
    fn out_of_order_samples_are_discarded() {
        let mut b = buf();
        b.record(
            ReplId(1),
            2.0,
            Vec3::new(2.0, 0.0, 0.0),
            Quat::IDENTITY,
            Vec3::ZERO,
        );
        // A stale sample at t=1 (< last) is dropped.
        b.record(
            ReplId(1),
            1.0,
            Vec3::new(99.0, 0.0, 0.0),
            Quat::IDENTITY,
            Vec3::ZERO,
        );
        // Only the t=2 sample remains; held at any render time.
        let out = b.sample(ReplId(1), 100.0).unwrap();
        assert!(out.translation.approx_eq(Vec3::new(2.0, 0.0, 0.0), 1e-5));
    }

    #[test]
    fn unknown_entity_samples_to_none() {
        let b = buf();
        assert!(b.sample(ReplId(7), 1.0).is_none());
    }
}
