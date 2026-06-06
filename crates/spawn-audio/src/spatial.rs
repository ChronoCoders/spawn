//! Spatial audio primitives: the single [`Listener`] and the distance
//! [`Attenuation`] model. Gain and pan math live in [`crate::params`].

use spawn_core::{Quat, Vec3};

use crate::error::{AudioError, AudioResult};

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Listener {
    pub position: Vec3,
    pub orientation: Quat,
}

impl Default for Listener {
    fn default() -> Self {
        Self {
            position: Vec3::ZERO,
            orientation: Quat::IDENTITY,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AttenuationModel {
    Linear,
    Inverse,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Attenuation {
    pub model: AttenuationModel,
    pub min_distance: f32,
    pub max_distance: f32,
}

impl Attenuation {
    /// Returns [`AudioError::Backend`] if either distance is non-finite,
    /// negative, or if `min_distance > max_distance`.
    pub fn new(model: AttenuationModel, min_distance: f32, max_distance: f32) -> AudioResult<Self> {
        if !min_distance.is_finite()
            || !max_distance.is_finite()
            || min_distance < 0.0
            || max_distance < 0.0
            || min_distance > max_distance
        {
            return Err(AudioError::Backend {
                context: "invalid attenuation distances",
            });
        }
        Ok(Self {
            model,
            min_distance,
            max_distance,
        })
    }
}

impl Default for Attenuation {
    fn default() -> Self {
        Self {
            model: AttenuationModel::Inverse,
            min_distance: 1.0,
            max_distance: 100.0,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use spawn_core::ApproxEq;

    #[test]
    fn listener_default_is_origin_identity() {
        let l = Listener::default();
        assert!(l.position.approx_eq_default(Vec3::ZERO));
        assert!(l.orientation.approx_eq_default(Quat::IDENTITY));
    }

    #[test]
    fn attenuation_new_validates() {
        assert!(Attenuation::new(AttenuationModel::Linear, 1.0, 10.0).is_ok());
        assert!(Attenuation::new(AttenuationModel::Linear, 10.0, 1.0).is_err());
        assert!(Attenuation::new(AttenuationModel::Linear, -1.0, 10.0).is_err());
        assert!(Attenuation::new(AttenuationModel::Linear, f32::NAN, 10.0).is_err());
    }

    #[test]
    fn attenuation_default() {
        let a = Attenuation::default();
        assert_eq!(a.model, AttenuationModel::Inverse);
        assert!(a.min_distance.approx_eq_default(1.0));
        assert!(a.max_distance.approx_eq_default(100.0));
    }
}
