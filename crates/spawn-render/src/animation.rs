//! Skeletal animation: per-joint keyframe tracks sampled (binary search + linear
//! interpolation, slerp for rotation) into a local pose, and a clip player that
//! advances time. The pose feeds [`crate::skeleton::Skeleton::skin_matrices`].

use spawn_core::{Quat, Transform3D, Vec3};

use crate::error::{RenderError, RenderResult};

/// A local pose: one [`Transform3D`] per joint, in skeleton order.
pub type Pose = Vec<Transform3D>;

/// One joint's keyframe track. The four arrays are parallel: keyframe `i` is at
/// `times[i]` with `translations[i]` / `rotations[i]` / `scales[i]`.
#[derive(Debug, Clone)]
pub struct JointTrack {
    pub times: Vec<f32>,
    pub translations: Vec<Vec3>,
    pub rotations: Vec<Quat>,
    pub scales: Vec<Vec3>,
}

impl JointTrack {
    fn validate(&self) -> RenderResult<()> {
        let n = self.times.len();
        if n == 0 {
            return Err(RenderError::AnimationInvalid {
                context: "animation track has no keyframes",
            });
        }
        if self.translations.len() != n || self.rotations.len() != n || self.scales.len() != n {
            return Err(RenderError::AnimationInvalid {
                context: "animation track keyframe arrays have mismatched lengths",
            });
        }
        let mut prev = f32::NEG_INFINITY;
        for &t in &self.times {
            if !t.is_finite() || t < prev {
                return Err(RenderError::AnimationInvalid {
                    context: "animation track times must be finite and non-decreasing",
                });
            }
            prev = t;
        }
        let finite_v = |v: &Vec3| v.x.is_finite() && v.y.is_finite() && v.z.is_finite();
        let finite_q =
            |q: &Quat| q.x.is_finite() && q.y.is_finite() && q.z.is_finite() && q.w.is_finite();
        if !self.translations.iter().all(finite_v)
            || !self.scales.iter().all(finite_v)
            || !self.rotations.iter().all(finite_q)
        {
            return Err(RenderError::AnimationInvalid {
                context: "animation track has a non-finite keyframe value",
            });
        }
        Ok(())
    }

    /// Samples the track at `time`, clamping to the endpoints outside the keyframe
    /// range. Assumes [`JointTrack::validate`] passed (non-empty, sorted times).
    fn sample(&self, time: f32) -> Transform3D {
        let n = self.times.len();
        if time <= self.times[0] {
            return self.key(0);
        }
        if time >= self.times[n - 1] {
            return self.key(n - 1);
        }
        // Last index whose time is <= `time` (segment start); `time` is strictly
        // inside the range here, so `i` is in `[0, n-2]`.
        let i = self.times.partition_point(|&t| t <= time) - 1;
        let t0 = self.times[i];
        let t1 = self.times[i + 1];
        let span = t1 - t0;
        let alpha = if span > f32::EPSILON {
            (time - t0) / span
        } else {
            0.0
        };
        Transform3D {
            translation: self.translations[i].lerp(self.translations[i + 1], alpha),
            rotation: self.rotations[i].slerp(self.rotations[i + 1], alpha),
            scale: self.scales[i].lerp(self.scales[i + 1], alpha),
        }
    }

    fn key(&self, i: usize) -> Transform3D {
        Transform3D {
            translation: self.translations[i],
            rotation: self.rotations[i],
            scale: self.scales[i],
        }
    }
}

/// A skeletal animation clip: one [`JointTrack`] per joint and a total duration.
#[derive(Debug, Clone)]
pub struct AnimationClip {
    tracks: Vec<JointTrack>,
    duration: f32,
}

impl AnimationClip {
    /// Validates and builds a clip. `Err(AnimationInvalid)` if `duration` is not a
    /// positive finite number, there are no tracks, or any track is invalid (empty,
    /// mismatched array lengths, non-finite, or non-monotonic times).
    pub fn new(tracks: Vec<JointTrack>, duration: f32) -> RenderResult<Self> {
        if !duration.is_finite() || duration <= 0.0 {
            return Err(RenderError::AnimationInvalid {
                context: "clip duration must be a positive finite number",
            });
        }
        if tracks.is_empty() {
            return Err(RenderError::AnimationInvalid {
                context: "clip has no tracks",
            });
        }
        for track in &tracks {
            track.validate()?;
        }
        Ok(Self { tracks, duration })
    }

    pub fn duration(&self) -> f32 {
        self.duration
    }

    pub fn joint_count(&self) -> usize {
        self.tracks.len()
    }

    /// Samples every track at `time`, producing the local pose (one entry per
    /// track, in joint order).
    pub fn sample(&self, time: f32) -> Pose {
        self.tracks.iter().map(|t| t.sample(time)).collect()
    }
}

/// Plays one [`AnimationClip`]: advances `time` by `dt * speed`, wrapping when
/// `looping` (else clamping to `[0, duration]`), and samples a [`Pose`].
#[derive(Debug, Clone, Copy)]
pub struct ClipPlayer {
    pub time: f32,
    pub speed: f32,
    pub looping: bool,
}

impl Default for ClipPlayer {
    fn default() -> Self {
        Self {
            time: 0.0,
            speed: 1.0,
            looping: true,
        }
    }
}

impl ClipPlayer {
    /// Advances the play head by `dt` seconds (scaled by `speed`) within `clip`'s
    /// duration, wrapping if `looping`, otherwise clamping to the end.
    pub fn advance(&mut self, clip: &AnimationClip, dt: f32) {
        let duration = clip.duration();
        self.time += dt * self.speed;
        if self.looping {
            self.time = self.time.rem_euclid(duration);
        } else {
            self.time = self.time.clamp(0.0, duration);
        }
    }

    /// Samples the clip at the current play head.
    pub fn sample(&self, clip: &AnimationClip) -> Pose {
        clip.sample(self.time)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn track() -> JointTrack {
        JointTrack {
            times: vec![0.0, 1.0, 2.0],
            translations: vec![
                Vec3::new(0.0, 0.0, 0.0),
                Vec3::new(10.0, 0.0, 0.0),
                Vec3::new(20.0, 0.0, 0.0),
            ],
            rotations: vec![Quat::IDENTITY, Quat::IDENTITY, Quat::IDENTITY],
            scales: vec![Vec3::ONE, Vec3::ONE, Vec3::ONE],
        }
    }

    #[test]
    fn samples_endpoints_midpoints_and_out_of_range() {
        let clip = AnimationClip::new(vec![track()], 2.0).unwrap();
        assert!((clip.sample(0.0)[0].translation.x - 0.0).abs() < 1e-6);
        assert!((clip.sample(2.0)[0].translation.x - 20.0).abs() < 1e-6);
        // Midpoint of the first segment.
        assert!((clip.sample(0.5)[0].translation.x - 5.0).abs() < 1e-6);
        // Before the first / after the last keyframe clamps.
        assert!((clip.sample(-5.0)[0].translation.x - 0.0).abs() < 1e-6);
        assert!((clip.sample(99.0)[0].translation.x - 20.0).abs() < 1e-6);
    }

    #[test]
    fn single_key_track_returns_that_key() {
        let single = JointTrack {
            times: vec![0.5],
            translations: vec![Vec3::new(3.0, 0.0, 0.0)],
            rotations: vec![Quat::IDENTITY],
            scales: vec![Vec3::ONE],
        };
        let clip = AnimationClip::new(vec![single], 1.0).unwrap();
        assert!((clip.sample(0.0)[0].translation.x - 3.0).abs() < 1e-6);
        assert!((clip.sample(10.0)[0].translation.x - 3.0).abs() < 1e-6);
    }

    #[test]
    fn nan_and_empty_tracks_are_rejected() {
        let mut bad = track();
        bad.translations[1].x = f32::NAN;
        assert!(matches!(
            AnimationClip::new(vec![bad], 2.0),
            Err(RenderError::AnimationInvalid { .. })
        ));
        let empty = JointTrack {
            times: vec![],
            translations: vec![],
            rotations: vec![],
            scales: vec![],
        };
        assert!(matches!(
            AnimationClip::new(vec![empty], 2.0),
            Err(RenderError::AnimationInvalid { .. })
        ));
    }

    #[test]
    fn nonpositive_duration_is_rejected() {
        assert!(matches!(
            AnimationClip::new(vec![track()], 0.0),
            Err(RenderError::AnimationInvalid { .. })
        ));
    }

    #[test]
    fn player_loops_and_clamps() {
        let clip = AnimationClip::new(vec![track()], 2.0).unwrap();
        let mut looping = ClipPlayer {
            time: 0.0,
            speed: 1.0,
            looping: true,
        };
        looping.advance(&clip, 2.5);
        assert!((looping.time - 0.5).abs() < 1e-6, "wraps past duration");

        let mut clamped = ClipPlayer {
            time: 0.0,
            speed: 1.0,
            looping: false,
        };
        clamped.advance(&clip, 5.0);
        assert!((clamped.time - 2.0).abs() < 1e-6, "clamps to duration");
    }
}
