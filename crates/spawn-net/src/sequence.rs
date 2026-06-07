//! Wrapping `u16` serial-number arithmetic (RFC 1982 half-range).

const HALF: u16 = 0x8000;

/// True iff `a` is "newer" than `b` under wrapping `u16` serial-number order.
///
/// Standard half-range test: a value within half the sequence space ahead of `b`
/// (modulo 65536) is considered greater. Reflexive comparison is false. This is the
/// only sanctioned ordering for sequence numbers; raw `<`/`>` must not be used on them.
pub fn sequence_greater_than(a: u16, b: u16) -> bool {
    ((a > b) && (a - b <= HALF)) || ((a < b) && (b - a > HALF))
}

/// Convenience inverse of [`sequence_greater_than`]: `sequence_greater_than(b, a)`.
pub fn sequence_less_than(a: u16, b: u16) -> bool {
    sequence_greater_than(b, a)
}

/// Signed wrapping distance from `b` to `a`, in the range `(-32768, 32768]`.
///
/// Positive when `a` is ahead of `b`. The half-range boundary maps to `+32768`.
pub fn sequence_diff(a: u16, b: u16) -> i32 {
    let d = a.wrapping_sub(b);
    if d > HALF {
        i32::from(d) - 65536
    } else {
        i32::from(d)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wraparound_ordering() {
        assert!(sequence_greater_than(0, 0xFFFF));
        assert!(!sequence_greater_than(0xFFFF, 0));
        assert!(sequence_less_than(0xFFFF, 0));
    }

    #[test]
    fn reflexive_is_false() {
        assert!(!sequence_greater_than(0, 0));
        assert!(!sequence_greater_than(42, 42));
        assert!(!sequence_less_than(42, 42));
    }

    #[test]
    fn basic_ordering() {
        assert!(sequence_greater_than(10, 5));
        assert!(!sequence_greater_than(5, 10));
    }

    #[test]
    fn half_range_boundary() {
        assert_eq!(sequence_diff(0x8000, 0), 0x8000);
        assert!(sequence_greater_than(0x8000, 0));
        // Exactly half range the other way is "behind".
        assert!(!sequence_greater_than(0, 0x8000));
    }

    #[test]
    fn diff_signs_and_wrap() {
        assert_eq!(sequence_diff(10, 5), 5);
        assert_eq!(sequence_diff(5, 10), -5);
        assert_eq!(sequence_diff(0, 0xFFFF), 1);
        assert_eq!(sequence_diff(0xFFFF, 0), -1);
        assert_eq!(sequence_diff(0, 0), 0);
    }
}
