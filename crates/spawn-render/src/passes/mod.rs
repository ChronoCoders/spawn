//! Render passes: the unlit forward pass, the depth-only shadow caster, the lit
//! forward pass (Lambert + ambient + PCF shadow), the physically based forward
//! pass (Cook-Torrance), the alpha-blended transparent pass, and the fullscreen
//! post chain (tonemap).

pub mod forward_lit;
pub mod forward_opaque;
pub mod forward_pbr;
pub mod overlay;
pub mod post;
pub mod shadow_depth;
pub mod transparent;
