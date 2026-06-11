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

#[derive(Debug, Clone, Copy, Default)]
pub struct SlowTimer(pub f32);

impl Resource for SlowTimer {}

#[derive(Debug, Clone, Copy)]
pub struct GameRng {
    state: u64,
}

impl GameRng {
    pub fn seeded(seed: u64) -> Self {
        Self {
            state: seed ^ 0x2545_F491_4F6C_DD1D,
        }
    }

    pub fn next_u64(&mut self) -> u64 {
        self.state = self.state.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.state;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    pub fn next_unit(&mut self) -> f32 {
        (self.next_u64() >> 40) as f32 / (1u64 << 24) as f32
    }

    pub fn chance(&mut self, probability: f32) -> bool {
        self.next_unit() < probability
    }

    pub fn below(&mut self, bound: u32) -> u32 {
        if bound == 0 {
            0
        } else {
            (self.next_u64() % u64::from(bound)) as u32
        }
    }
}

impl Resource for GameRng {}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    Title,
    Playing,
    LevelComplete,
    GameOver,
    Won,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
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
