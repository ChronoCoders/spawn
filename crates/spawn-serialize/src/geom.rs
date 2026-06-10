//! Geometry serialization over `spawn-core` types: bounded position quantization
//! and unit-quaternion smallest-three compression, both built on [`crate::pack`].

use std::f32::consts::FRAC_1_SQRT_2;

use spawn_core::{Quat, Vec3};

use crate::error::SerializeResult;
use crate::pack::serialize_quantized_f32;
use crate::stream::Stream;

/// Per-axis quantization bounds for a position.
#[derive(Debug, Clone, Copy)]
pub struct PositionBounds {
    /// Inclusive lower corner.
    pub min: Vec3,
    /// Inclusive upper corner.
    pub max: Vec3,
    /// Bits per axis (`x`, `y`, `z`); each in `1..=32`.
    pub bits: [u32; 3],
}

/// Serialize a position by quantizing each axis independently over its bound.
/// Round-trips to within one per-axis step.
pub fn serialize_position<S: Stream>(
    s: &mut S,
    value: &mut Vec3,
    bounds: PositionBounds,
) -> SerializeResult<()> {
    serialize_quantized_f32(s, &mut value.x, bounds.min.x, bounds.max.x, bounds.bits[0])?;
    serialize_quantized_f32(s, &mut value.y, bounds.min.y, bounds.max.y, bounds.bits[1])?;
    serialize_quantized_f32(s, &mut value.z, bounds.min.z, bounds.max.z, bounds.bits[2])
}

/// Serialize a unit quaternion via **smallest-three**: the largest-magnitude
/// component is dropped (its index sent in 2 bits, its sign folded so it is
/// non-negative), and the other three are quantized over `[-1/√2, 1/√2]` in `bits`
/// bits each (total `2 + 3·bits`).
///
/// On read the dropped component is reconstructed as
/// `sqrt(max(0, 1 − a² − b² − c²))` and the result is renormalized, so a
/// denormalized peer value can never yield a non-finite quaternion. The input is
/// assumed (and normalized) to be a unit quaternion.
pub fn serialize_unit_quat<S: Stream>(
    s: &mut S,
    value: &mut Quat,
    bits: u32,
) -> SerializeResult<()> {
    let mut index = 0u64;
    if s.is_writing() {
        let q = value.normalize().unwrap_or(Quat::IDENTITY);
        let comps = [q.x, q.y, q.z, q.w];
        let mut largest = 0usize;
        for i in 1..4 {
            if comps[i].abs() > comps[largest].abs() {
                largest = i;
            }
        }
        // Fold sign so the dropped component is non-negative (q and -q are the same
        // rotation), letting the reader reconstruct it as a positive square root.
        let sign = if comps[largest] < 0.0 { -1.0 } else { 1.0 };
        index = largest as u64;
        s.serialize_bits(&mut index, 2)?;
        for (i, &c) in comps.iter().enumerate() {
            if i != largest {
                let mut packed = c * sign;
                serialize_quantized_f32(s, &mut packed, -FRAC_1_SQRT_2, FRAC_1_SQRT_2, bits)?;
            }
        }
        Ok(())
    } else {
        s.serialize_bits(&mut index, 2)?;
        let dropped = index as usize;
        let mut comps = [0.0f32; 4];
        let mut sum_sq = 0.0f32;
        for (i, slot) in comps.iter_mut().enumerate() {
            if i != dropped {
                let mut packed = 0.0f32;
                serialize_quantized_f32(s, &mut packed, -FRAC_1_SQRT_2, FRAC_1_SQRT_2, bits)?;
                *slot = packed;
                sum_sq += packed * packed;
            }
        }
        comps[dropped] = (1.0 - sum_sq).max(0.0).sqrt();
        let q = Quat::from_xyzw(comps[0], comps[1], comps[2], comps[3])
            .normalize()
            .unwrap_or(Quat::IDENTITY);
        *value = q;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bits::{BitReader, BitWriter};
    use spawn_core::ApproxEq;

    fn pos_bounds() -> PositionBounds {
        PositionBounds {
            min: Vec3::splat(-256.0),
            max: Vec3::splat(256.0),
            bits: [18, 18, 14],
        }
    }

    #[test]
    fn position_round_trips_within_step() {
        let b = pos_bounds();
        let steps = [
            (b.max.x - b.min.x) / (((1u64 << b.bits[0]) - 1) as f32),
            (b.max.y - b.min.y) / (((1u64 << b.bits[1]) - 1) as f32),
            (b.max.z - b.min.z) / (((1u64 << b.bits[2]) - 1) as f32),
        ];
        let v = Vec3::new(12.5, -200.1, 77.0);
        let mut buf = [0u8; 16];
        let mut w = BitWriter::new(&mut buf);
        serialize_position(&mut w, &mut { v }, b).unwrap();
        let n = w.finish();
        let mut r = BitReader::new(&buf[..n]);
        let mut out = Vec3::ZERO;
        serialize_position(&mut r, &mut out, b).unwrap();
        assert!((out.x - v.x).abs() <= steps[0]);
        assert!((out.y - v.y).abs() <= steps[1]);
        assert!((out.z - v.z).abs() <= steps[2]);
    }

    fn quat_round_trip(q: Quat, bits: u32) -> Quat {
        let mut buf = [0u8; 16];
        let mut w = BitWriter::new(&mut buf);
        serialize_unit_quat(&mut w, &mut { q }, bits).unwrap();
        let n = w.finish();
        let mut r = BitReader::new(&buf[..n]);
        let mut out = Quat::IDENTITY;
        serialize_unit_quat(&mut r, &mut out, bits).unwrap();
        out
    }

    #[test]
    fn quat_round_trips_each_largest_component() {
        // Rotations whose largest component is, in turn, w, x, y, z.
        let cases = [
            Quat::IDENTITY,
            Quat::from_axis_angle(Vec3::X, 3.0).unwrap(),
            Quat::from_axis_angle(Vec3::Y, 3.0).unwrap(),
            Quat::from_axis_angle(Vec3::Z, 3.0).unwrap(),
            Quat::from_axis_angle(Vec3::new(1.0, 2.0, -1.0), 1.3).unwrap(),
        ];
        for q in cases {
            let out = quat_round_trip(q, 12);
            // q and out may differ by global sign (same rotation); compare via rotation.
            let v = Vec3::new(0.3, -1.0, 0.7);
            assert!(
                out.rotate(v).approx_eq(q.rotate(v), 1e-2),
                "rotation mismatch for {q:?} -> {out:?}"
            );
            assert!(out.is_finite() && out.is_normalized());
        }
    }

    #[test]
    fn denormalized_input_reads_back_finite_unit() {
        let denorm = Quat::from_xyzw(0.0, 0.0, 0.0, 0.0);
        let out = quat_round_trip(denorm, 10);
        assert!(out.is_finite());
        assert!(out.is_normalized());
    }
}
