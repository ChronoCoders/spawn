use spawn_asset::AssetId;
use spawn_ecs::Component;

#[derive(Debug, Clone, Copy)]
pub struct Renderable {
    pub mesh: AssetId,
    pub material: AssetId,
}

impl Component for Renderable {}

#[derive(Debug, Clone, Copy)]
pub struct Ball {
    pub speed: f32,
    pub launched: bool,
}

impl Component for Ball {}

#[derive(Debug, Clone, Copy)]
pub struct Paddle {
    pub half_width: f32,
    pub min_x: f32,
    pub max_x: f32,
}

impl Component for Paddle {}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum BrickKind {
    Normal,
    Tough,
    Reinforced,
    Solid,
}

impl BrickKind {
    pub fn initial_health(self) -> u8 {
        match self {
            BrickKind::Normal => 1,
            BrickKind::Tough => 2,
            BrickKind::Reinforced => 3,
            BrickKind::Solid => 0,
        }
    }

    pub fn is_breakable(self) -> bool {
        !matches!(self, BrickKind::Solid)
    }
}

#[derive(Debug, Clone, Copy)]
pub struct Brick {
    pub health: u8,
    pub kind: BrickKind,
}

impl Brick {
    pub fn new(kind: BrickKind) -> Self {
        Self {
            health: kind.initial_health(),
            kind,
        }
    }
}

impl Component for Brick {}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WallSide {
    Left,
    Right,
    Top,
    Bottom,
}

#[derive(Debug, Clone, Copy)]
pub struct Wall {
    pub side: WallSide,
}

impl Component for Wall {}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum PowerUpKind {
    WidenPaddle,
    MultiBall,
    SlowBall,
    ExtraLife,
}

#[derive(Debug, Clone, Copy)]
pub struct PowerUp {
    pub kind: PowerUpKind,
}

impl Component for PowerUp {}

#[derive(Debug, Clone, Copy)]
pub struct BackPlane;

impl Component for BackPlane {}
