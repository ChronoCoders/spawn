use spawn_core::Vec3;
use spawn_ecs::{Entity, World};

use crate::components::{Ball, Brick, BrickKind, PowerUpKind};
use crate::field;
use crate::physics;
use crate::powerup;
use crate::resources::{Contact, DropChance, GameRng, GameState, Phase};

pub fn spawn_brick(world: &mut World, kind: BrickKind, center: Vec3) -> Entity {
    let (rb, col) = physics::brick_bodies();
    let half = Vec3::new(
        field::BRICK_HALF_WIDTH,
        field::BRICK_HALF_HEIGHT,
        field::BRICK_HALF_DEPTH,
    );
    world.spawn_with((
        physics::box_transform(center, half),
        Brick::new(kind),
        rb,
        col,
    ))
}

fn brick_in_contact(world: &World, contact: &Contact) -> Option<Entity> {
    let a_ball = world.get::<Ball>(contact.a).is_some();
    let b_ball = world.get::<Ball>(contact.b).is_some();
    if a_ball && world.get::<Brick>(contact.b).is_some() {
        Some(contact.b)
    } else if b_ball && world.get::<Brick>(contact.a).is_some() {
        Some(contact.a)
    } else {
        None
    }
}

fn kind_from_roll(roll: u32) -> PowerUpKind {
    match roll % 4 {
        0 => PowerUpKind::WidenPaddle,
        1 => PowerUpKind::MultiBall,
        2 => PowerUpKind::SlowBall,
        _ => PowerUpKind::ExtraLife,
    }
}

pub fn brick_collisions(world: &mut World, contacts: &[Contact]) {
    let mut destroyed: Vec<Vec3> = Vec::new();
    let mut score_gain: u32 = 0;
    for contact in contacts {
        if !contact.started {
            continue;
        }
        let Some(brick_entity) = brick_in_contact(world, contact) else {
            continue;
        };
        let Some(brick) = world.get::<Brick>(brick_entity).copied() else {
            continue;
        };
        if !brick.kind.is_breakable() {
            continue;
        }
        score_gain += brick.kind.score();
        let health = brick.health.saturating_sub(1);
        if health == 0 {
            if let Some(transform) = world.get::<spawn_core::Transform3D>(brick_entity) {
                destroyed.push(transform.translation);
            }
            let _ = world.despawn(brick_entity);
        } else if let Some(brick) = world.get_mut::<Brick>(brick_entity) {
            brick.health = health;
        }
    }

    if score_gain > 0 {
        if let Some(mut state) = world.get_resource_mut::<GameState>() {
            state.score += score_gain;
        }
        crate::audio::play(world, crate::audio::SoundEffect::Break);
    }

    let drop_chance = world
        .get_resource::<DropChance>()
        .map(|chance| chance.0)
        .unwrap_or(0.0);
    for position in destroyed {
        let drop = world
            .get_resource_mut::<GameRng>()
            .map(|mut rng| (rng.chance(drop_chance), rng.next_u64() as u32));
        if let Some((true, roll)) = drop {
            powerup::spawn_powerup(world, kind_from_roll(roll), position);
        }
    }
}

pub fn level_clear(world: &mut World) {
    let playing = world
        .get_resource::<GameState>()
        .map(|state| state.phase == Phase::Playing)
        .unwrap_or(false);
    if !playing {
        return;
    }
    let breakable = world
        .query::<&Brick>()
        .iter()
        .filter(|brick| brick.kind.is_breakable())
        .count();
    if breakable == 0 {
        if let Some(mut state) = world.get_resource_mut::<GameState>() {
            state.phase = Phase::LevelComplete;
        }
        crate::audio::play(world, crate::audio::SoundEffect::LevelComplete);
    }
}
