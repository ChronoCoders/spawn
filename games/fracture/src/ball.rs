use spawn_core::{Transform3D, Vec3};
use spawn_ecs::{Entity, World};

use crate::components::{Ball, Paddle};
use crate::field;
use crate::physics::LinVel;
use crate::resources::{Contact, PaddleControl, PaddleState};

const MIN_VERTICAL_FRACTION: f32 = 0.25;
const ENGLISH_STRENGTH: f32 = 8.0;

pub fn renormalize(velocity: Vec3, speed: f32) -> Vec3 {
    velocity.normalize_or_zero() * speed
}

pub fn maintain_speed(velocity: Vec3, speed: f32) -> Vec3 {
    let scaled = renormalize(velocity, speed);
    let min_vertical = speed * MIN_VERTICAL_FRACTION;
    if scaled.y.abs() >= min_vertical {
        return scaled;
    }
    let sign_y = if scaled.y >= 0.0 { 1.0 } else { -1.0 };
    let sign_x = if scaled.x >= 0.0 { 1.0 } else { -1.0 };
    let vy = sign_y * min_vertical;
    let vx = sign_x * (speed * speed - vy * vy).max(0.0).sqrt();
    Vec3::new(vx, vy, 0.0)
}

pub fn launch_velocity() -> Vec3 {
    renormalize(Vec3::new(0.3, 1.0, 0.0), field::BALL_SPEED)
}

pub fn paddle_english(velocity: Vec3, ball_x: f32, paddle_x: f32, speed: f32) -> Vec3 {
    let offset = ((ball_x - paddle_x) / field::PADDLE_HALF_WIDTH).clamp(-1.0, 1.0);
    let steered = Vec3::new(
        velocity.x + offset * ENGLISH_STRENGTH,
        velocity.y.abs(),
        0.0,
    );
    renormalize(steered, speed)
}

pub fn ball_launch_and_speed(world: &mut World) {
    let control = match world.get_resource::<PaddleControl>() {
        Some(control) => *control,
        None => return,
    };
    let paddle_x = world
        .get_resource::<PaddleState>()
        .map(|state| state.x)
        .unwrap_or(0.0);
    let ball_y = field::PADDLE_Y + field::PADDLE_HALF_HEIGHT + field::BALL_RADIUS;
    for (transform, ball, velocity) in world
        .query_mut::<(&mut Transform3D, &mut Ball, &mut LinVel)>()
        .iter_mut()
    {
        if ball.launched {
            velocity.0 = maintain_speed(velocity.0, ball.speed);
        } else {
            transform.translation = Vec3::new(paddle_x, ball_y, 0.0);
            velocity.0 = Vec3::ZERO;
            ball.speed = field::BALL_SPEED;
            if control.launch {
                ball.launched = true;
                velocity.0 = launch_velocity();
            }
        }
    }
}

fn paddle_entity(world: &World) -> Option<Entity> {
    world
        .query::<(Entity, &Paddle)>()
        .iter()
        .map(|(entity, _)| entity)
        .next()
}

pub fn ball_response(world: &mut World, contacts: &[Contact]) {
    let Some(paddle) = paddle_entity(world) else {
        return;
    };
    let paddle_x = world
        .get_resource::<PaddleState>()
        .map(|state| state.x)
        .unwrap_or(0.0);
    for contact in contacts {
        if !contact.started {
            continue;
        }
        let ball = if contact.b == paddle && world.get::<Ball>(contact.a).is_some() {
            Some(contact.a)
        } else if contact.a == paddle && world.get::<Ball>(contact.b).is_some() {
            Some(contact.b)
        } else {
            None
        };
        let Some(ball) = ball else { continue };
        let ball_x = world.get::<Transform3D>(ball).map(|t| t.translation.x);
        let speed = world.get::<Ball>(ball).map(|b| b.speed);
        let velocity = world.get::<LinVel>(ball).map(|v| v.0);
        if let (Some(ball_x), Some(speed), Some(velocity)) = (ball_x, speed, velocity) {
            let steered = paddle_english(velocity, ball_x, paddle_x, speed);
            if let Some(linvel) = world.get_mut::<LinVel>(ball) {
                linvel.0 = steered;
            }
        }
    }
}
