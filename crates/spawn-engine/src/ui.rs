use spawn_ecs::World;
use spawn_ui::{FontId, UiTree};

use crate::error::EngineResult;

/// The font id the wgpu backend registers its embedded monospace face under. Game
/// text nodes must `set_font` to this id (or inherit it) to be rendered.
pub const DEFAULT_FONT: FontId = FontId(1);

pub(crate) type UiSetup = Box<dyn FnOnce(&mut UiTree) -> EngineResult<()>>;
pub(crate) type UiUpdate = Box<dyn FnMut(&World, &mut UiTree) -> EngineResult<()>>;
