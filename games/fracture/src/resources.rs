use spawn_ecs::Resource;

pub const STARTING_LIVES: u8 = 3;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    Title,
    Playing,
    LevelComplete,
    GameOver,
    Won,
}

#[derive(Debug, Clone, Copy)]
pub struct GameState {
    pub score: u32,
    pub lives: u8,
    pub level: usize,
    pub phase: Phase,
}

impl Default for GameState {
    fn default() -> Self {
        Self {
            score: 0,
            lives: STARTING_LIVES,
            level: 0,
            phase: Phase::Title,
        }
    }
}

impl Resource for GameState {}
