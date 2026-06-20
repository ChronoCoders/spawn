//! Read/write-symmetric [`Serialize`] impls for the engine transform components.
//!
//! These live here, not in `spawn-ecs`, because the orphan rule requires the impl
//! to sit in the crate that owns the [`Serialize`] trait. Components are stored
//! losslessly (raw IEEE-754 bits per scalar) so a transform round-trips exactly,
//! independent of any quantization bounds.

use spawn_core::{Transform2D, Transform3D};

use crate::error::SerializeResult;
use crate::stream::{Serialize, Stream};

fn serialize_f32<S: Stream>(stream: &mut S, value: &mut f32) -> SerializeResult<()> {
    let mut bits = u64::from(value.to_bits());
    stream.serialize_bits(&mut bits, 32)?;
    *value = f32::from_bits(bits as u32);
    Ok(())
}

impl Serialize for Transform3D {
    fn serialize<S: Stream>(&mut self, s: &mut S) -> SerializeResult<()> {
        serialize_f32(s, &mut self.translation.x)?;
        serialize_f32(s, &mut self.translation.y)?;
        serialize_f32(s, &mut self.translation.z)?;
        serialize_f32(s, &mut self.rotation.x)?;
        serialize_f32(s, &mut self.rotation.y)?;
        serialize_f32(s, &mut self.rotation.z)?;
        serialize_f32(s, &mut self.rotation.w)?;
        serialize_f32(s, &mut self.scale.x)?;
        serialize_f32(s, &mut self.scale.y)?;
        serialize_f32(s, &mut self.scale.z)
    }
}

impl Serialize for Transform2D {
    fn serialize<S: Stream>(&mut self, s: &mut S) -> SerializeResult<()> {
        serialize_f32(s, &mut self.translation.x)?;
        serialize_f32(s, &mut self.translation.y)?;
        serialize_f32(s, &mut self.rotation)?;
        serialize_f32(s, &mut self.scale.x)?;
        serialize_f32(s, &mut self.scale.y)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bits::{BitReader, BitWriter};
    use spawn_core::{Quat, Vec2, Vec3};

    #[test]
    fn transform3d_round_trips_exactly() {
        let mut original = Transform3D {
            translation: Vec3::new(1.5, -2.25, 3.0),
            rotation: Quat::from_xyzw(0.1, 0.2, 0.3, 0.9),
            scale: Vec3::new(2.0, 0.5, 1.0),
        };
        let mut buf = [0u8; 64];
        let mut w = BitWriter::new(&mut buf);
        original.serialize(&mut w).unwrap();
        let n = w.finish();
        let mut decoded = Transform3D {
            translation: Vec3::default(),
            rotation: Quat::from_xyzw(0.0, 0.0, 0.0, 1.0),
            scale: Vec3::default(),
        };
        let mut r = BitReader::new(&buf[..n]);
        decoded.serialize(&mut r).unwrap();
        assert_eq!(decoded, original);
    }

    #[test]
    fn transform2d_round_trips_exactly() {
        let mut original = Transform2D {
            translation: Vec2::new(-4.0, 7.5),
            rotation: 1.25,
            scale: Vec2::new(3.0, 0.25),
        };
        let mut buf = [0u8; 32];
        let mut w = BitWriter::new(&mut buf);
        original.serialize(&mut w).unwrap();
        let n = w.finish();
        let mut decoded = Transform2D {
            translation: Vec2::default(),
            rotation: 0.0,
            scale: Vec2::default(),
        };
        let mut r = BitReader::new(&buf[..n]);
        decoded.serialize(&mut r).unwrap();
        assert_eq!(decoded, original);
    }
}
