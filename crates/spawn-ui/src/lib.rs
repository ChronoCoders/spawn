#![deny(warnings)]

//! Retained-mode UI layer for the Spawn engine.
//!
//! A [`UiTree`] of styled nodes is laid out with a flexbox-subset algorithm
//! ([`compute_layout`](UiTree::compute_layout)), routed pointer input
//! ([`update_input`](UiTree::update_input)) producing a drained [`UiEvent`]
//! queue, and rendered into a renderer-agnostic [`DrawList`]
//! ([`build_draw_list`](UiTree::build_draw_list)). The crate is renderer- and
//! window-agnostic: callers bridge `spawn-input` into [`UiInputState`] and feed
//! the produced [`DrawList`] to a renderer in a later phase.

pub mod draw;
pub mod error;
pub mod input;
pub mod layout;
pub mod style;
pub mod tree;
pub mod widgets;

pub use draw::{DrawCommand, DrawList, FontId, TextureId, UiImage};
pub use error::{UiError, UiResult};
pub use input::{MouseButton, PointerButton, UiEvent, UiInputState};
pub use layout::TextMeasure;
pub use style::{
    AlignItems, Border, Dimension, Display, Edges, FlexDirection, JustifyContent, Size, Style,
};
pub use tree::{NodeId, UiTree};
pub use widgets::{Button, Checkbox, DragValue, Label, Panel};
