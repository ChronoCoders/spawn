use spawn_core::Vec3;
use spawn_ecs::World;

use crate::brick;
use crate::components::BrickKind;
use crate::field;
use crate::resources::{DropChance, GameRng};

pub const LEVEL_COUNT: usize = 3;

const COLS: usize = 9;
const COL_SPACING: f32 = 2.0;
const ROW_SPACING: f32 = 1.2;
const TOP_ROW_Y: f32 = field::HALF_HEIGHT - 3.0;

const LEVELS: [&[&str]; LEVEL_COUNT] = [
    &["NNNNNNNNN", "NNNNNNNNN", "NNNNNNNNN"],
    &[
        ".S.....S.",
        "...TNT...",
        "..TNNNT..",
        ".TNNNNNT.",
        "SNNNNNNNS",
    ],
    &[
        "SSSSSSSSS",
        "SRRRRRRRS",
        "SRR.R.RRS",
        "SRRRRRRRS",
        "SR.RRR.RS",
        "SSSSSSSSS",
    ],
];

fn cell_kind(cell: char) -> Option<BrickKind> {
    match cell {
        'N' => Some(BrickKind::Normal),
        'T' => Some(BrickKind::Tough),
        'R' => Some(BrickKind::Reinforced),
        'S' => Some(BrickKind::Solid),
        _ => None,
    }
}

fn cell_position(col: usize, row: usize) -> Vec3 {
    let x = (col as f32 - (COLS as f32 - 1.0) * 0.5) * COL_SPACING;
    let y = TOP_ROW_Y - row as f32 * ROW_SPACING;
    Vec3::new(x, y, 0.0)
}

pub fn drop_chance(level: usize) -> f32 {
    match level % LEVEL_COUNT {
        0 => 0.2,
        1 => 0.35,
        _ => 0.5,
    }
}

pub fn brick_specs(level: usize) -> Vec<(BrickKind, Vec3)> {
    let grid = LEVELS[level % LEVEL_COUNT];
    let mut specs = Vec::new();
    for (row, line) in grid.iter().enumerate() {
        for (col, cell) in line.chars().enumerate() {
            if let Some(kind) = cell_kind(cell) {
                specs.push((kind, cell_position(col, row)));
            }
        }
    }
    specs
}

pub fn spawn_level(world: &mut World, level: usize) {
    if let Some(mut chance) = world.get_resource_mut::<DropChance>() {
        chance.0 = drop_chance(level);
    }
    if let Some(mut rng) = world.get_resource_mut::<GameRng>() {
        *rng = GameRng::seeded(level as u64);
    }
    for (kind, position) in brick_specs(level) {
        brick::spawn_brick(world, kind, position);
    }
}
