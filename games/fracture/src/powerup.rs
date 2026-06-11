use spawn_core::{Transform3D, Vec3};
use spawn_ecs::{Entity, World};

use crate::ball;
use crate::components::{Ball, Paddle, PowerUp, PowerUpKind, Wall, WallSide};
use crate::field;
use crate::physics::{self, LinVel};
use crate::resources::{Contact, GameState, SlowTimer};

pub fn spawn_powerup(world: &mut World, kind: PowerUpKind, center: Vec3) -> Entity {
    let (rb, col) = physics::powerup_bodies();
    let half = Vec3::new(
        field::POWERUP_HALF,
        field::POWERUP_HALF,
        field::POWERUP_HALF,
    );
    world.spawn_with((
        physics::box_transform(center, half),
        PowerUp { kind },
        crate::render::powerup_renderable(kind),
        rb,
        col,
    ))
}

enum Catcher {
    Paddle,
    BottomWall,
    Other,
}

fn powerup_in_contact(world: &World, contact: &Contact) -> Option<(Entity, Catcher)> {
    let (powerup, other) = if world.get::<PowerUp>(contact.a).is_some() {
        (contact.a, contact.b)
    } else if world.get::<PowerUp>(contact.b).is_some() {
        (contact.b, contact.a)
    } else {
        return None;
    };
    let catcher = if world.get::<Paddle>(other).is_some() {
        Catcher::Paddle
    } else if world
        .get::<Wall>(other)
        .map(|wall| wall.side == WallSide::Bottom)
        .unwrap_or(false)
    {
        Catcher::BottomWall
    } else {
        Catcher::Other
    };
    Some((powerup, catcher))
}

pub fn powerup_collection(world: &mut World, contacts: &[Contact]) {
    let mut collected: Vec<PowerUpKind> = Vec::new();
    let mut remove: Vec<Entity> = Vec::new();
    for contact in contacts {
        if !contact.started {
            continue;
        }
        let Some((powerup, catcher)) = powerup_in_contact(world, contact) else {
            continue;
        };
        match catcher {
            Catcher::Paddle => {
                if let Some(kind) = world.get::<PowerUp>(powerup).map(|p| p.kind) {
                    collected.push(kind);
                }
                remove.push(powerup);
            }
            Catcher::BottomWall => remove.push(powerup),
            Catcher::Other => {}
        }
    }
    for entity in remove {
        let _ = world.despawn(entity);
    }
    for kind in collected {
        apply_effect(world, kind);
    }
}

fn apply_effect(world: &mut World, kind: PowerUpKind) {
    match kind {
        PowerUpKind::ExtraLife => {
            if let Some(mut state) = world.get_resource_mut::<GameState>() {
                state.lives = state.lives.saturating_add(1);
            }
        }
        PowerUpKind::SlowBall => slow_ball(world),
        PowerUpKind::MultiBall => multi_ball(world),
        PowerUpKind::WidenPaddle => widen_paddle(world),
    }
}

fn slow_ball(world: &mut World) {
    if let Some(mut timer) = world.get_resource_mut::<SlowTimer>() {
        timer.0 = field::SLOW_DURATION;
    }
    for ball in world.query_mut::<&mut Ball>().iter_mut() {
        ball.speed = field::BALL_SPEED * field::SLOW_FACTOR;
    }
}

fn multi_ball(world: &mut World) {
    let source = world
        .query::<(&Transform3D, &Ball, &LinVel)>()
        .iter()
        .find(|(_, ball, _)| ball.launched)
        .map(|(transform, ball, velocity)| (transform.translation, ball.speed, velocity.0));
    let Some((position, speed, velocity)) = source else {
        return;
    };
    let mirrored = Vec3::new(-velocity.x, velocity.y.abs().max(1.0), 0.0);
    let (rb, col) = physics::ball_bodies();
    world.spawn_with((
        physics::ball_transform(position),
        Ball {
            speed,
            launched: true,
        },
        crate::render::ball_renderable(),
        LinVel(ball::renormalize(mirrored, speed)),
        rb,
        col,
    ));
}

fn widen_paddle(world: &mut World) {
    let paddle = world
        .query::<(Entity, &Transform3D, &Paddle)>()
        .iter()
        .next()
        .map(|(entity, transform, paddle)| (entity, transform.translation, *paddle));
    let Some((entity, position, paddle)) = paddle else {
        return;
    };
    let new_half = (paddle.half_width * field::WIDEN_FACTOR).min(field::PADDLE_MAX_HALF_WIDTH);
    if new_half <= paddle.half_width {
        return;
    }
    let _ = world.despawn(entity);
    let half = Vec3::new(
        new_half,
        field::PADDLE_HALF_HEIGHT,
        field::PADDLE_HALF_DEPTH,
    );
    let (rb, col) = physics::paddle_bodies_with(new_half);
    world.spawn_with((
        physics::box_transform(position, half),
        Paddle {
            half_width: new_half,
            min_x: -field::HALF_WIDTH + new_half,
            max_x: field::HALF_WIDTH - new_half,
        },
        crate::render::paddle_renderable(),
        rb,
        col,
    ));
}

pub fn tick_slow(world: &mut World) {
    let expired = match world.get_resource_mut::<SlowTimer>() {
        Some(mut timer) if timer.0 > 0.0 => {
            timer.0 -= field::FIXED_TIMESTEP;
            timer.0 <= 0.0
        }
        _ => false,
    };
    if expired {
        for ball in world.query_mut::<&mut Ball>().iter_mut() {
            ball.speed = field::BALL_SPEED;
        }
    }
}
