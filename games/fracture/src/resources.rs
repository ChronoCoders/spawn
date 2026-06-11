use spawn_ecs::{Entity, Resource};

pub const STARTING_LIVES: u8 = 3;

#[derive(Debug, Clone, Copy)]
pub struct Contact {
    pub a: Entity,
    pub b: Entity,
    pub started: bool,
}

#[derive(Debug, Default)]
pub struct Collisions {
    pub contacts: Vec<Contact>,
}

impl Resource for Collisions {}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum InputSource {
    #[default]
    Keys,
    Mouse,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct PaddleControl {
    pub key_intent: f32,
    pub mouse_x: f32,
    pub source: InputSource,
    pub launch: bool,
}

impl Resource for PaddleControl {}

#[derive(Debug, Clone, Copy, Default)]
pub struct PaddleState {
    pub x: f32,
}

impl Resource for PaddleState {}

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
