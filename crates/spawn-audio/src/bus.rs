//! Mixer bus identity and routing.
//!
//! Routing is one level deep: every named bus routes directly to
//! [`BusId::MASTER`], which routes to the device. Effective voice volume is
//! `voice_volume * bus_volume * master_volume`, all linear amplitude clamped
//! per stage.

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct BusId(pub &'static str);

impl BusId {
    pub const MASTER: BusId = BusId("master");
}
