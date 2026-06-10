//! Integer, signed (zig-zag), bounded-integer, and quantized-float helpers, all
//! generic over [`Stream`] so they compose inside a [`Serialize`](crate::Serialize)
//! implementation. Each helper has a single symmetric form: on a writer it encodes
//! `*value`, on a reader it decodes into `*value`.

use std::cmp::Ordering;

use crate::error::{SerializeError, SerializeResult};
use crate::stream::Stream;

/// Bits needed to store `count` distinct values (`0..count`). Zero for `count ≤ 1`
/// (a single possible value carries no information).
fn bits_for_count(count: u64) -> u32 {
    if count <= 1 {
        0
    } else {
        64 - (count - 1).leading_zeros()
    }
}

fn zigzag_encode(v: i64) -> u64 {
    ((v << 1) ^ (v >> 63)) as u64
}

fn zigzag_decode(u: u64) -> i64 {
    ((u >> 1) as i64) ^ -((u & 1) as i64)
}

/// Serialize an unsigned integer in exactly `width` bits (`width ∈ 1..=64`).
/// `OutOfRange` on a writer if `*value` does not fit `width`.
pub fn serialize_uint<S: Stream>(s: &mut S, value: &mut u64, width: u32) -> SerializeResult<()> {
    s.serialize_bits(value, width)
}

/// Serialize a signed integer in `width` bits via zig-zag mapping (small-magnitude
/// values stay small). `OutOfRange` on a writer if the mapped value exceeds `width`.
pub fn serialize_int<S: Stream>(s: &mut S, value: &mut i64, width: u32) -> SerializeResult<()> {
    if s.is_writing() {
        let mut u = zigzag_encode(*value);
        s.serialize_bits(&mut u, width)
    } else {
        let mut u = 0u64;
        s.serialize_bits(&mut u, width)?;
        *value = zigzag_decode(u);
        Ok(())
    }
}

/// Serialize `*value ∈ [min, max]` in the minimum bits for the range
/// (`ceil(log2(max - min + 1))`). A degenerate `min == max` range carries zero bits.
/// `OutOfRange` if `max < min`, or on a writer if `*value` is outside `[min, max]`.
pub fn serialize_bounded<S: Stream>(
    s: &mut S,
    value: &mut i64,
    min: i64,
    max: i64,
) -> SerializeResult<()> {
    if max < min {
        return Err(SerializeError::OutOfRange {
            context: "serialize_bounded: max < min",
        });
    }
    let span = (max as i128) - (min as i128);
    let width = if span as u128 == u64::MAX as u128 {
        64
    } else {
        bits_for_count(span as u64 + 1)
    };

    if s.is_writing() {
        if *value < min || *value > max {
            return Err(SerializeError::OutOfRange {
                context: "serialize_bounded: value outside [min, max]",
            });
        }
        if width > 0 {
            let mut u = ((*value as i128) - (min as i128)) as u64;
            s.serialize_bits(&mut u, width)?;
        }
    } else {
        let mut u = 0u64;
        if width > 0 {
            s.serialize_bits(&mut u, width)?;
        }
        *value = ((min as i128) + (u as i128)) as i64;
    }
    Ok(())
}

/// Largest quantization width (keeps `1 << bits` within `u64` and the step exact).
const MAX_QUANT_BITS: u32 = 32;

/// Serialize `*value` quantized over `[min, max]` into `bits` bits (`bits ∈ 1..=32`).
/// The value is clamped into the interval on write; it round-trips to within one
/// step `(max - min) / (2^bits - 1)`. Bounds and `bits` are agreed out of band and
/// never transmitted. `InvalidWidth` if `bits` is `0` or `> 32`; `OutOfRange` if
/// `max <= min`.
pub fn serialize_quantized_f32<S: Stream>(
    s: &mut S,
    value: &mut f32,
    min: f32,
    max: f32,
    bits: u32,
) -> SerializeResult<()> {
    if bits == 0 || bits > MAX_QUANT_BITS {
        return Err(SerializeError::InvalidWidth { width: bits });
    }
    // `Some(Greater)` only when `max > min`; `None` (a NaN bound) also errors.
    if max.partial_cmp(&min) != Some(Ordering::Greater) {
        return Err(SerializeError::OutOfRange {
            context: "serialize_quantized_f32: max <= min",
        });
    }
    let max_q = ((1u64 << bits) - 1) as f32;
    let span = max - min;

    if s.is_writing() {
        let clamped = value.clamp(min, max);
        let t = ((clamped - min) / span).clamp(0.0, 1.0);
        let mut q = (t * max_q + 0.5).floor() as u64;
        s.serialize_bits(&mut q, bits)
    } else {
        let mut q = 0u64;
        s.serialize_bits(&mut q, bits)?;
        *value = min + (q as f32 / max_q) * span;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bits::{BitReader, BitWriter};

    fn round_trip<T: PartialEq + core::fmt::Debug + Copy>(
        write: impl FnOnce(&mut BitWriter),
        read: impl FnOnce(&mut BitReader) -> T,
        expect: T,
    ) {
        let mut buf = [0u8; 16];
        let mut w = BitWriter::new(&mut buf);
        write(&mut w);
        let written = w.bits_written();
        let n = w.finish();
        let mut r = BitReader::new(&buf[..n]);
        assert_eq!(read(&mut r), expect);
        assert_eq!(r.bits_read(), written);
    }

    #[test]
    fn bits_for_count_basic() {
        assert_eq!(bits_for_count(0), 0);
        assert_eq!(bits_for_count(1), 0);
        assert_eq!(bits_for_count(2), 1);
        assert_eq!(bits_for_count(256), 8);
        assert_eq!(bits_for_count(257), 9);
    }

    #[test]
    fn zigzag_round_trips() {
        for v in [0i64, -1, 1, -1234, 1234, i32::MIN as i64, i32::MAX as i64] {
            assert_eq!(zigzag_decode(zigzag_encode(v)), v);
        }
    }

    #[test]
    fn int_round_trips_small_and_negative() {
        for v in [0i64, -1, 7, -1000, 30000] {
            round_trip(
                |w| serialize_int(w, &mut { v }, 17).unwrap(),
                |r| {
                    let mut out = 0i64;
                    serialize_int(r, &mut out, 17).unwrap();
                    out
                },
                v,
            );
        }
    }

    #[test]
    fn bounded_round_trips_including_degenerate() {
        for (v, min, max) in [(5i64, 0, 10), (0, 0, 0), (-50, -100, -40), (255, 0, 255)] {
            round_trip(
                |w| serialize_bounded(w, &mut { v }, min, max).unwrap(),
                |r| {
                    let mut out = 0i64;
                    serialize_bounded(r, &mut out, min, max).unwrap();
                    out
                },
                v,
            );
        }
    }

    #[test]
    fn bounded_rejects_bad_inputs() {
        let mut buf = [0u8; 8];
        let mut w = BitWriter::new(&mut buf);
        assert!(matches!(
            serialize_bounded(&mut w, &mut 0, 10, 0),
            Err(SerializeError::OutOfRange { .. })
        ));
        assert!(matches!(
            serialize_bounded(&mut w, &mut 99, 0, 10),
            Err(SerializeError::OutOfRange { .. })
        ));
    }

    #[test]
    fn degenerate_bounded_writes_no_bits() {
        let mut buf = [0u8; 8];
        let mut w = BitWriter::new(&mut buf);
        serialize_bounded(&mut w, &mut 7, 7, 7).unwrap();
        assert_eq!(w.bits_written(), 0);
    }

    #[test]
    fn quantized_round_trips_within_one_step() {
        let (min, max, bits) = (-256.0f32, 256.0f32, 16u32);
        let step = (max - min) / (((1u64 << bits) - 1) as f32);
        for v in [-256.0f32, -100.3, 0.0, 42.7, 255.9, 256.0] {
            let mut buf = [0u8; 8];
            let mut w = BitWriter::new(&mut buf);
            serialize_quantized_f32(&mut w, &mut { v }, min, max, bits).unwrap();
            let n = w.finish();
            let mut r = BitReader::new(&buf[..n]);
            let mut out = 0.0f32;
            serialize_quantized_f32(&mut r, &mut out, min, max, bits).unwrap();
            assert!((out - v).abs() <= step, "v={v} out={out} step={step}");
        }
    }

    #[test]
    fn quantized_clamps_out_of_range() {
        let (min, max, bits) = (0.0f32, 1.0f32, 12u32);
        let mut buf = [0u8; 8];
        let mut w = BitWriter::new(&mut buf);
        serialize_quantized_f32(&mut w, &mut 5.0, min, max, bits).unwrap();
        let n = w.finish();
        let mut r = BitReader::new(&buf[..n]);
        let mut out = 0.0f32;
        serialize_quantized_f32(&mut r, &mut out, min, max, bits).unwrap();
        assert!((out - 1.0).abs() < 1e-3);
    }

    #[test]
    fn quantized_is_deterministic() {
        let mut a = [0u8; 8];
        let mut b = [0u8; 8];
        let mut wa = BitWriter::new(&mut a);
        let mut wb = BitWriter::new(&mut b);
        serialize_quantized_f32(&mut wa, &mut 0.333, -1.0, 1.0, 20).unwrap();
        serialize_quantized_f32(&mut wb, &mut 0.333, -1.0, 1.0, 20).unwrap();
        assert_eq!(a, b);
    }

    #[test]
    fn quantized_rejects_bad_width_and_range() {
        let mut buf = [0u8; 8];
        let mut w = BitWriter::new(&mut buf);
        assert!(matches!(
            serialize_quantized_f32(&mut w, &mut 0.0, 0.0, 1.0, 0),
            Err(SerializeError::InvalidWidth { .. })
        ));
        assert!(matches!(
            serialize_quantized_f32(&mut w, &mut 0.0, 1.0, 1.0, 8),
            Err(SerializeError::OutOfRange { .. })
        ));
    }
}
