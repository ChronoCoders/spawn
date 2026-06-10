//! A shared replicated test component, used across module unit tests. Compiled only
//! under `#[cfg(test)]`.

use spawn_ecs::Component;
use spawn_serialize::{Serialize, SerializeResult, Stream};

use crate::registry::Replicate;

/// A small replicated component: two raw `f32`s (sent bit-exact so tests can assert
/// equality) and a bool — exercises both `serialize_bits` and `serialize_bool`.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct TPos {
    pub x: f32,
    pub y: f32,
    pub flag: bool,
}

impl Component for TPos {}

impl Serialize for TPos {
    fn serialize<S: Stream>(&mut self, s: &mut S) -> SerializeResult<()> {
        let mut bx = u64::from(self.x.to_bits());
        s.serialize_bits(&mut bx, 32)?;
        self.x = f32::from_bits(bx as u32);
        let mut by = u64::from(self.y.to_bits());
        s.serialize_bits(&mut by, 32)?;
        self.y = f32::from_bits(by as u32);
        s.serialize_bool(&mut self.flag)?;
        Ok(())
    }
}

impl Replicate for TPos {
    fn replicate_name() -> &'static str {
        "TPos"
    }
}
