//! Opt-in client-side prediction and server reconciliation for the owning client's
//! `AutonomousProxy` (its pawn).
//!
//! The client tags each input with a `u16` sequence, applies it locally immediately,
//! and retains every still-unacked input. The server echoes the last input sequence it
//! processed (in the snapshot); reconciliation drops acked inputs and snaps the
//! predicted entity to the authoritative state the snapshot already wrote.
//!
//! **Snap-only is the Phase 2d baseline (decision 4):** reconciliation snaps to
//! authoritative without replaying inputs, until `spawn-ecs` change detection (and the
//! deterministic fixed-tick guarantee) lands. [`replay`] is provided for when replay is
//! enabled, it re-applies the unacked inputs on top of the authoritative state via a
//! game-supplied step, but it is not invoked by the snap-only baseline. The visual
//! error correction ([`PredictionSmoother`]) applies in both modes.

use std::collections::VecDeque;

use spawn_core::Vec3;
use spawn_ecs::{Component, Entity, World};
use spawn_net::sequence_greater_than;

/// Opt-in marker: the game inserts this on the owned entity it wants predicted. Only a
/// predicted entity is an [`AutonomousProxy`](crate::NetRole::AutonomousProxy).
pub struct Predicted;
impl Component for Predicted {}

/// Default position-divergence threshold (world units): beyond this, a correction snaps
/// rather than smooths.
pub const SNAP_EPSILON: f32 = 0.5;
/// Default per-frame decay factor for the smoothed error offset (`[0,1)`).
pub const SMOOTH_DECAY: f32 = 0.85;

/// A ring of the client's still-unacked inputs, each tagged with a wrapping `u16`
/// sequence. Generic over the game's input type `I`.
pub struct InputBuffer<I> {
    next_seq: u16,
    inputs: VecDeque<(u16, I)>,
}

impl<I> Default for InputBuffer<I> {
    fn default() -> Self {
        Self {
            next_seq: 0,
            inputs: VecDeque::new(),
        }
    }
}

impl<I> InputBuffer<I> {
    /// An empty buffer.
    pub fn new() -> Self {
        Self::default()
    }

    /// Record an input, assigning and returning its sequence number. The caller also
    /// applies it locally (prediction) and sends it.
    pub fn push(&mut self, input: I) -> u16 {
        let seq = self.next_seq;
        self.next_seq = self.next_seq.wrapping_add(1);
        self.inputs.push_back((seq, input));
        seq
    }

    /// Drop every input the server has processed (`seq <= through`, wrap-correct),
    /// leaving only the unacked tail to replay.
    pub fn ack(&mut self, through: u16) {
        while let Some((seq, _)) = self.inputs.front() {
            if sequence_greater_than(*seq, through) {
                break;
            }
            self.inputs.pop_front();
        }
    }

    /// The still-unacked inputs, oldest first (the replay order).
    pub fn unacked(&self) -> impl Iterator<Item = &(u16, I)> {
        self.inputs.iter()
    }

    /// Number of unacked inputs.
    pub fn len(&self) -> usize {
        self.inputs.len()
    }

    /// Whether there are no unacked inputs.
    pub fn is_empty(&self) -> bool {
        self.inputs.is_empty()
    }

    /// The sequence that will be assigned to the next [`push`](Self::push).
    pub fn next_sequence(&self) -> u16 {
        self.next_seq
    }
}

/// Re-apply the unacked inputs (oldest first) onto `entity` via the game's `step`,
/// rebuilding the predicted state on top of the authoritative snapshot. Used only when
/// replay reconciliation is enabled; the snap-only baseline does not call this.
pub fn replay<I, F>(buffer: &InputBuffer<I>, world: &mut World, entity: Entity, mut step: F)
where
    F: FnMut(&mut World, Entity, &I),
{
    for (_seq, input) in buffer.unacked() {
        step(world, entity, input);
    }
}

/// The visual error-correction state for a predicted entity: a decaying offset that
/// hides a small reconciliation correction, or snaps for a large one (spec §7).
#[derive(Debug, Clone, Copy, Default)]
pub struct PredictionSmoother {
    offset: Vec3,
}

impl PredictionSmoother {
    /// A smoother with no pending correction.
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a correction: `corrected` is the new authoritative (and, if replaying,
    /// post-replay) position; `previous_render` is where the entity was being drawn. A
    /// jump beyond `epsilon` **snaps** (offset cleared); a smaller one is **smoothed**
    /// (the offset starts at the error and is decayed to zero over subsequent frames).
    pub fn on_correction(&mut self, corrected: Vec3, previous_render: Vec3, epsilon: f32) {
        let error = previous_render - corrected;
        self.offset = if error.length() > epsilon {
            Vec3::ZERO
        } else {
            error
        };
    }

    /// Decay the smoothed offset toward zero by `factor` (`[0,1)`), once per frame.
    pub fn decay(&mut self, factor: f32) {
        self.offset *= factor;
    }

    /// The position to render: the authoritative `corrected` plus the (decaying) offset.
    pub fn render(&self, corrected: Vec3) -> Vec3 {
        corrected + self.offset
    }

    /// The current offset magnitude (diagnostics / tests).
    pub fn offset(&self) -> Vec3 {
        self.offset
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testcomp::TPos;
    use spawn_core::ApproxEq;

    #[test]
    fn input_buffer_sequences_and_acks() {
        let mut b: InputBuffer<u32> = InputBuffer::new();
        for i in 0..5 {
            assert_eq!(b.push(i), i as u16);
        }
        assert_eq!(b.len(), 5);
        b.ack(2); // server processed through seq 2
        let remaining: Vec<u16> = b.unacked().map(|(s, _)| *s).collect();
        assert_eq!(remaining, vec![3, 4]);
        assert_eq!(b.next_sequence(), 5);
    }

    #[test]
    fn ack_is_wrap_correct() {
        let mut b: InputBuffer<u8> = InputBuffer::new();
        b.next_seq = u16::MAX - 1;
        let s0 = b.push(0); // 65534
        let s1 = b.push(1); // 65535
        let s2 = b.push(2); // 0 (wrapped)
        assert_eq!((s0, s1, s2), (65534, 65535, 0));
        b.ack(65535); // acks 65534 and 65535, not the wrapped 0
        let remaining: Vec<u16> = b.unacked().map(|(s, _)| *s).collect();
        assert_eq!(remaining, vec![0]);
    }

    #[test]
    fn replay_reapplies_only_unacked_inputs() {
        // The predicted entity's x is rebuilt by replaying unacked +1 inputs.
        let mut w = World::new();
        w.register::<TPos>();
        let e = w.spawn_with((TPos::default(),));
        let mut b: InputBuffer<f32> = InputBuffer::new();
        for _ in 0..5 {
            b.push(1.0); // each input adds 1 to x
        }
        b.ack(1); // seqs 0,1 acked -> 3 unacked remain (2,3,4)

        // Authoritative snap: server says x = 2 (it processed 2 inputs).
        w.get_mut::<TPos>(e).unwrap().x = 2.0;
        replay(&b, &mut w, e, |w, e, &dx| {
            w.get_mut::<TPos>(e).unwrap().x += dx;
        });
        assert!(
            (w.get::<TPos>(e).unwrap().x - 5.0).abs() < 1e-6,
            "2 authoritative + 3 replayed"
        );
    }

    #[test]
    fn large_correction_snaps_small_one_smooths() {
        let corrected = Vec3::new(0.0, 0.0, 0.0);

        // Large divergence (> epsilon) -> snap: render at the authoritative position.
        let mut s = PredictionSmoother::new();
        s.on_correction(corrected, Vec3::new(10.0, 0.0, 0.0), SNAP_EPSILON);
        assert!(s.offset().approx_eq(Vec3::ZERO, 1e-6));
        assert!(s.render(corrected).approx_eq(corrected, 1e-6));

        // Small divergence (< epsilon) -> smooth: render starts at the old position and
        // decays toward the authoritative one.
        let mut s = PredictionSmoother::new();
        let previous = Vec3::new(0.2, 0.0, 0.0);
        s.on_correction(corrected, previous, SNAP_EPSILON);
        assert!(
            s.render(corrected).approx_eq(previous, 1e-6),
            "starts at the old render"
        );
        for _ in 0..50 {
            s.decay(SMOOTH_DECAY);
        }
        assert!(
            s.render(corrected).approx_eq(corrected, 1e-3),
            "decays to authoritative"
        );
    }
}
