//! Render passes: the unlit forward pass, the depth-only shadow caster, and the
//! lit forward pass (Lambert + ambient + PCF shadow).

pub mod forward_lit;
pub mod forward_opaque;
pub mod overlay;
pub mod shadow_depth;
