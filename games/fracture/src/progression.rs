use spawn_core::Vec3;
use spawn_ecs::{Component, Entity, World};

use crate::components::{Ball, Brick, PowerUp, Wall, WallSide};
use crate::physics::{self, LinVel};
use crate::resources::{Contact, GameState, PaddleControl, PaddleState, Phase};
use crate::{audio, field, levels, render};

fn despawn_all<C: Component>(world: &mut World) {
    let entities: Vec<Entity> = world
        .query::<(Entity, &C)>()
        .iter()
        .map(|(entity, _)| entity)
        .collect();
    for entity in entities {
        let _ = world.despawn(entity);
    }
}

fn spawn_ball_on_paddle(world: &mut World) {
    let paddle_x = world
        .get_resource::<PaddleState>()
        .map(|state| state.x)
        .unwrap_or(0.0);
    let position = Vec3::new(
        paddle_x,
        field::PADDLE_Y + field::PADDLE_HALF_HEIGHT + field::BALL_RADIUS,
        0.0,
    );
    let (rb, col) = physics::ball_bodies();
    world.spawn_with((
        physics::ball_transform(position),
        Ball {
            speed: 0.0,
            launched: false,
        },
        render::ball_renderable(),
        LinVel(Vec3::ZERO),
        rb,
        col,
    ));
}

fn ball_past_bottom(world: &World, contact: &Contact) -> Option<Entity> {
    let (ball, other) = if world.get::<Ball>(contact.a).is_some() {
        (contact.a, contact.b)
    } else if world.get::<Ball>(contact.b).is_some() {
        (contact.b, contact.a)
    } else {
        return None;
    };
    let is_bottom = world
        .get::<Wall>(other)
        .map(|wall| wall.side == WallSide::Bottom)
        .unwrap_or(false);
    is_bottom.then_some(ball)
}

pub fn ball_lost(world: &mut World, contacts: &[Contact]) {
    let mut lost: Vec<Entity> = Vec::new();
    for contact in contacts {
        if !contact.started {
            continue;
        }
        if let Some(ball) = ball_past_bottom(world, contact) {
            lost.push(ball);
        }
    }
    for ball in lost {
        let _ = world.despawn(ball);
    }
}

pub fn life_check(world: &mut World) {
    let playing = world
        .get_resource::<GameState>()
        .map(|state| state.phase == Phase::Playing)
        .unwrap_or(false);
    if !playing {
        return;
    }
    if world.query::<&Ball>().iter().count() > 0 {
        return;
    }
    let lives = world
        .get_resource::<GameState>()
        .map(|state| state.lives)
        .unwrap_or(0);
    if lives > 1 {
        if let Some(mut state) = world.get_resource_mut::<GameState>() {
            state.lives -= 1;
        }
        spawn_ball_on_paddle(world);
    } else {
        if let Some(mut state) = world.get_resource_mut::<GameState>() {
            state.lives = 0;
            state.phase = Phase::GameOver;
        }
        audio::play(world, audio::SoundEffect::GameOver);
    }
}

pub fn advance(world: &mut World) {
    let level_complete = world
        .get_resource::<GameState>()
        .map(|state| state.phase == Phase::LevelComplete)
        .unwrap_or(false);
    if !level_complete {
        return;
    }
    despawn_all::<Brick>(world);
    despawn_all::<Ball>(world);
    despawn_all::<PowerUp>(world);
    let level = world
        .get_resource::<GameState>()
        .map(|state| state.level)
        .unwrap_or(0);
    let next = level + 1;
    if next >= levels::LEVEL_COUNT {
        if let Some(mut state) = world.get_resource_mut::<GameState>() {
            state.phase = Phase::Won;
        }
    } else {
        if let Some(mut state) = world.get_resource_mut::<GameState>() {
            state.level = next;
            state.phase = Phase::Title;
        }
        levels::spawn_level(world, next);
        spawn_ball_on_paddle(world);
    }
}

fn restart(world: &mut World) {
    despawn_all::<Brick>(world);
    despawn_all::<Ball>(world);
    despawn_all::<PowerUp>(world);
    if let Some(mut state) = world.get_resource_mut::<GameState>() {
        *state = GameState::default();
    }
    levels::spawn_level(world, 0);
    spawn_ball_on_paddle(world);
}

pub fn start_on_launch(world: &mut World) {
    let launch = world
        .get_resource::<PaddleControl>()
        .map(|control| control.launch)
        .unwrap_or(false);
    if !launch {
        return;
    }
    let phase = world.get_resource::<GameState>().map(|state| state.phase);
    match phase {
        Some(Phase::Title) => {
            if let Some(mut state) = world.get_resource_mut::<GameState>() {
                state.phase = Phase::Playing;
            }
        }
        Some(Phase::GameOver) | Some(Phase::Won) => restart(world),
        _ => {}
    }
}
