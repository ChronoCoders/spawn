//! Playback parameters and the pure, deterministic parameter math (no device,
//! no kira). These functions are the core unit-test target.

use std::f32::consts::FRAC_PI_2;

use spawn_core::Vec3;

use crate::bus::BusId;
use crate::spatial::{Attenuation, AttenuationModel, Listener};

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Spatial {
    pub position: Vec3,
    pub attenuation: Attenuation,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PlaybackParams {
    /// Linear amplitude, clamped to `0.0..=1.0` at play time.
    pub volume: f32,
    /// Playback-rate multiplier; `1.0` = original.
    pub pitch: f32,
    pub looping: bool,
    pub bus: BusId,
    /// `None` = non-spatial (2D) playback routed straight to its bus at
    /// `volume`; [`crate::AudioEngine::set_position`] on such a voice is a no-op.
    pub spatial: Option<Spatial>,
}

impl Default for PlaybackParams {
    fn default() -> Self {
        Self {
            volume: 1.0,
            pitch: 1.0,
            looping: false,
            bus: BusId::MASTER,
            spatial: None,
        }
    }
}

/// `10^(db/20)`.
pub fn db_to_amplitude(db: f32) -> f32 {
    10.0_f32.powf(db / 20.0)
}

/// `20*log10(amp)`; `amp <= 0` maps to [`f32::NEG_INFINITY`].
pub fn amplitude_to_db(amp: f32) -> f32 {
    if amp <= 0.0 {
        f32::NEG_INFINITY
    } else {
        20.0 * amp.log10()
    }
}

/// Distance gain in `0.0..=1.0`. Full inside `min_distance`; floored at
/// `max_distance`. Linear: `1 - (d-min)/(max-min)`. Inverse: `min/d`. Both
/// clamped and monotonic non-increasing in `d`.
pub fn attenuation_gain(att: Attenuation, distance: f32) -> f32 {
    let d = if distance.is_finite() {
        distance.max(0.0)
    } else {
        att.max_distance
    };
    if d <= att.min_distance {
        return 1.0;
    }
    if d >= att.max_distance {
        return match att.model {
            AttenuationModel::Linear => 0.0,
            AttenuationModel::Inverse => {
                if att.max_distance > 0.0 {
                    (att.min_distance / att.max_distance).clamp(0.0, 1.0)
                } else {
                    1.0
                }
            }
        };
    }
    match att.model {
        AttenuationModel::Linear => {
            let span = att.max_distance - att.min_distance;
            if span <= 0.0 {
                1.0
            } else {
                (1.0 - (d - att.min_distance) / span).clamp(0.0, 1.0)
            }
        }
        AttenuationModel::Inverse => {
            if d > 0.0 {
                (att.min_distance / d).clamp(0.0, 1.0)
            } else {
                1.0
            }
        }
    }
}

/// Listener-relative azimuth of `emitter`, mapped to `[-1, 1]` (left..right) as
/// the sine of the angle off the listener's forward axis. Co-located emitters
/// (or a degenerate orientation) return `0` (centered).
pub fn stereo_pan(listener: Listener, emitter: Vec3) -> f32 {
    let to_emitter = emitter - listener.position;
    let Some(dir) = to_emitter.normalize() else {
        return 0.0;
    };
    let right = listener.orientation.rotate(Vec3::X);
    let Some(right) = right.normalize() else {
        return 0.0;
    };
    dir.dot(right).clamp(-1.0, 1.0)
}

/// Equal-power stereo gains for `pan` in `[-1, 1]`. Returns `(left, right)`,
/// each in `0.0..=1.0`, with `left² + right² ≈ 1`.
pub fn equal_power_gains(pan: f32) -> (f32, f32) {
    let p = pan.clamp(-1.0, 1.0);
    let angle = (p + 1.0) * 0.5 * FRAC_PI_2;
    (angle.cos().max(0.0), angle.sin().max(0.0))
}

/// Clamps to `0.0..=1.0`; maps any non-finite value to `0.0`.
pub fn clamp_amplitude(amp: f32) -> f32 {
    if amp.is_finite() {
        amp.clamp(0.0, 1.0)
    } else {
        0.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use spawn_core::{ApproxEq, Quat};

    #[test]
    fn db_amplitude_roundtrip() {
        for db in [-40.0_f32, -6.0, 0.0, 3.0] {
            let amp = db_to_amplitude(db);
            assert!(amplitude_to_db(amp).approx_eq(db, 1e-3));
        }
        assert!(db_to_amplitude(0.0).approx_eq_default(1.0));
        assert!(amplitude_to_db(1.0).approx_eq_default(0.0));
    }

    #[test]
    fn amplitude_to_db_zero_is_neg_inf() {
        assert_eq!(amplitude_to_db(0.0), f32::NEG_INFINITY);
        assert_eq!(amplitude_to_db(-1.0), f32::NEG_INFINITY);
    }

    #[test]
    fn linear_attenuation_curve() {
        let a = Attenuation::new(AttenuationModel::Linear, 1.0, 11.0).unwrap();
        assert!(attenuation_gain(a, 0.0).approx_eq_default(1.0));
        assert!(attenuation_gain(a, 1.0).approx_eq_default(1.0));
        assert!(attenuation_gain(a, 6.0).approx_eq(0.5, 1e-5));
        assert!(attenuation_gain(a, 11.0).approx_eq_default(0.0));
        assert!(attenuation_gain(a, 20.0).approx_eq_default(0.0));
    }

    #[test]
    fn inverse_attenuation_curve() {
        let a = Attenuation::new(AttenuationModel::Inverse, 1.0, 10.0).unwrap();
        assert!(attenuation_gain(a, 0.5).approx_eq_default(1.0));
        assert!(attenuation_gain(a, 1.0).approx_eq_default(1.0));
        assert!(attenuation_gain(a, 2.0).approx_eq(0.5, 1e-5));
        assert!(attenuation_gain(a, 10.0).approx_eq(0.1, 1e-5));
        assert!(attenuation_gain(a, 100.0).approx_eq(0.1, 1e-5));
    }

    #[test]
    fn attenuation_monotonic_non_increasing() {
        for model in [AttenuationModel::Linear, AttenuationModel::Inverse] {
            let a = Attenuation::new(model, 1.0, 10.0).unwrap();
            let mut prev = attenuation_gain(a, 0.0);
            let mut d = 0.0;
            while d <= 15.0 {
                let g = attenuation_gain(a, d);
                assert!(g <= prev + 1e-6);
                prev = g;
                d += 0.25;
            }
        }
    }

    #[test]
    fn stereo_pan_sign() {
        let l = Listener::default();
        // Default listener faces -Z, right is +X.
        assert!(stereo_pan(l, Vec3::new(1.0, 0.0, 0.0)) > 0.9);
        assert!(stereo_pan(l, Vec3::new(-1.0, 0.0, 0.0)) < -0.9);
        assert!(stereo_pan(l, Vec3::new(0.0, 0.0, -1.0)).approx_eq(0.0, 1e-5));
        assert!(stereo_pan(l, Vec3::ZERO).approx_eq_default(0.0));
    }

    #[test]
    fn stereo_pan_respects_orientation() {
        let l = Listener {
            position: Vec3::ZERO,
            orientation: Quat::from_rotation_y(std::f32::consts::PI),
        };
        assert!(stereo_pan(l, Vec3::new(1.0, 0.0, 0.0)) < -0.9);
    }

    #[test]
    fn equal_power_unit_energy() {
        for i in -10..=10 {
            let pan = i as f32 / 10.0;
            let (lft, rgt) = equal_power_gains(pan);
            assert!((lft * lft + rgt * rgt).approx_eq(1.0, 1e-5));
            assert!((0.0..=1.0).contains(&lft));
            assert!((0.0..=1.0).contains(&rgt));
        }
        let (lft, rgt) = equal_power_gains(0.0);
        assert!(lft.approx_eq(rgt, 1e-5));
    }

    #[test]
    fn clamp_amplitude_range_and_nonfinite() {
        assert!(clamp_amplitude(-0.5).approx_eq_default(0.0));
        assert!(clamp_amplitude(0.5).approx_eq_default(0.5));
        assert!(clamp_amplitude(2.0).approx_eq_default(1.0));
        assert!(clamp_amplitude(f32::NAN).approx_eq_default(0.0));
        assert!(clamp_amplitude(f32::INFINITY).approx_eq_default(0.0));
    }

    #[test]
    fn playback_params_default() {
        let p = PlaybackParams::default();
        assert!(p.volume.approx_eq_default(1.0));
        assert!(p.pitch.approx_eq_default(1.0));
        assert!(!p.looping);
        assert_eq!(p.bus, BusId::MASTER);
        assert!(p.spatial.is_none());
    }
}
