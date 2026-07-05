//! Frame-pacing primitives: the render sync mode and the ordered schedule labels.

/// How far the render backend is allowed to lag the simulation frontend.
///
/// On the inline executor both modes are synchronous, so frames-in-flight is
/// always zero. On the render thread the ownership-passing proxy transport bounds
/// the lag to at most one frame (Finding 1: bound the pipeline, never unbounded).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SyncMode {
    /// Render the proxies extracted this frame: zero frames in flight, lowest
    /// latency. The default.
    #[default]
    Immediate,
    /// On the render thread, render the previous frame's proxies: at most one
    /// frame in flight, never more. Collapses to `Immediate` on the inline
    /// executor.
    Pipelined,
}

/// The ordered stages of both the variable-rate and fixed-rate schedules. A
/// system registered against a label runs in that stage; stages run `First` then
/// `Update` then `Last`, each preserving spawn-ecs's within-stage determinism.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScheduleLabel {
    First,
    Update,
    Last,
}

impl ScheduleLabel {
    /// Dense index into the engine's per-schedule stage array.
    pub(crate) const fn index(self) -> usize {
        match self {
            ScheduleLabel::First => 0,
            ScheduleLabel::Update => 1,
            ScheduleLabel::Last => 2,
        }
    }

    /// Stable stage name handed to spawn-ecs `Stage::new`.
    pub(crate) const fn name(self) -> &'static str {
        match self {
            ScheduleLabel::First => "first",
            ScheduleLabel::Update => "update",
            ScheduleLabel::Last => "last",
        }
    }

    pub(crate) const ALL: [ScheduleLabel; 3] = [
        ScheduleLabel::First,
        ScheduleLabel::Update,
        ScheduleLabel::Last,
    ];
}
