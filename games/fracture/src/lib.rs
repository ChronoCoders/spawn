#![deny(warnings)]

use spawn_engine::{App, EngineConfig, EngineResult, SyncMode, WindowConfig};

pub const FIELD_WIDTH: f32 = 20.0;
pub const FIELD_HEIGHT: f32 = 24.0;

const WINDOW_SIZE: (u32, u32) = (720, 864);

fn config() -> EngineConfig {
    EngineConfig {
        sync_mode: SyncMode::Immediate,
        window: WindowConfig {
            title: "Fracture".to_string(),
            size: WINDOW_SIZE,
            ..WindowConfig::default()
        },
        ..EngineConfig::default()
    }
}

pub fn build() -> EngineResult<App> {
    let mut app = App::new();
    app.set_config(config());
    Ok(app)
}

pub fn run() -> EngineResult<()> {
    build()?.run()
}
