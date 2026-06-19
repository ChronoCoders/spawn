//! Frame-granular change-detection ticks.

/// A monotonically increasing frame counter backing [`Added`](crate::Added) /
/// [`Changed`](crate::Changed) detection.
///
/// `u64` is wide enough that wrap is not a practical concern, so there is no
/// periodic wrap-clamp pass. [`Tick::ZERO`] is the "before any frame" sentinel
/// used by a fresh reader cursor and by direct (non-system) world queries, which
/// therefore observe every populated row as both added and changed on first read.
#[repr(transparent)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Tick(u64);

impl Tick {
    pub(crate) const ZERO: Tick = Tick(0);

    pub(crate) const fn next(self) -> Self {
        Tick(self.0 + 1)
    }
}

impl Default for Tick {
    fn default() -> Self {
        Tick::ZERO
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const _: () = assert!(std::mem::size_of::<Tick>() == 8);

    #[test]
    fn next_is_monotonic_and_zero_is_minimum() {
        let a = Tick::ZERO;
        let b = a.next();
        assert!(b > a);
        assert!(a < b.next());
        assert_eq!(Tick::default(), Tick::ZERO);
    }
}
