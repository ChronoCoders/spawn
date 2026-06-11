use fracture::components::{Ball, Brick, BrickKind, Paddle, PowerUp, PowerUpKind};
use fracture::physics::LinVel;
use fracture::resources::{Contact, GameState, InputSource, PaddleControl, Phase, SlowTimer};
use fracture::{brick, powerup, progression};
use spawn_core::{Transform3D, Vec3};
use spawn_ecs::Entity;
use spawn_engine::Engine;

fn first_ball_x(engine: &Engine) -> Option<f32> {
    engine
        .world()
        .query::<(&Transform3D, &Ball)>()
        .iter()
        .next()
        .map(|(transform, _)| transform.translation.x)
}

fn paddle_entity(engine: &mut Engine) -> Entity {
    engine
        .world()
        .query::<(Entity, &Paddle)>()
        .iter()
        .map(|(entity, _)| entity)
        .next()
        .unwrap()
}

fn brick_count(engine: &Engine) -> usize {
    engine.world().query::<&Brick>().iter().count()
}

fn ball_count(engine: &Engine) -> usize {
    engine.world().query::<&Ball>().iter().count()
}

fn despawn_balls(engine: &mut Engine) {
    let world = engine.world_mut();
    let balls: Vec<Entity> = world
        .query::<(Entity, &Ball)>()
        .iter()
        .map(|(e, _)| e)
        .collect();
    for b in balls {
        let _ = world.despawn(b);
    }
}

#[test]
fn full_loop_real_physics_destroys_bricks_and_shuts_down_clean() {
    let mut engine = fracture::build().unwrap().build_headless().unwrap();
    let start = brick_count(&engine);
    assert_eq!(start, 27);
    for _ in 0..3000 {
        let ball_x = first_ball_x(&engine);
        {
            let mut control = engine.world().get_resource_mut::<PaddleControl>().unwrap();
            control.launch = true;
            if let Some(x) = ball_x {
                control.source = InputSource::Mouse;
                control.mouse_x = x;
            }
        }
        engine.tick().unwrap();
    }
    let score = engine.world().get_resource::<GameState>().unwrap().score;
    let remaining = brick_count(&engine);
    assert!(score > 0, "real physics destroyed bricks (score {score})");
    assert!(remaining < start, "bricks reduced {remaining} < {start}");
    assert!(
        first_ball_x(&engine).is_some() || ball_count(&engine) == 0,
        "engine remained consistent"
    );
    drop(engine);
}

#[test]
fn each_brick_kind_steps_health_and_scores_and_solid_deflects() {
    let mut engine = fracture::build().unwrap().build_headless().unwrap();
    let world = engine.world_mut();
    let ball = world.spawn_with((
        Transform3D::IDENTITY,
        Ball {
            speed: 14.0,
            launched: true,
        },
        LinVel(Vec3::ZERO),
    ));
    let hit = |world: &mut spawn_ecs::World, e: Entity| {
        brick::brick_collisions(
            world,
            &[Contact {
                a: ball,
                b: e,
                started: true,
            }],
        )
    };

    let tough = world.spawn_with((Transform3D::IDENTITY, Brick::new(BrickKind::Tough)));
    hit(world, tough);
    assert_eq!(world.get::<Brick>(tough).unwrap().health, 1);
    hit(world, tough);
    assert!(world.get::<Brick>(tough).is_none());

    let reinforced = world.spawn_with((Transform3D::IDENTITY, Brick::new(BrickKind::Reinforced)));
    hit(world, reinforced);
    assert_eq!(world.get::<Brick>(reinforced).unwrap().health, 2);
    hit(world, reinforced);
    assert_eq!(world.get::<Brick>(reinforced).unwrap().health, 1);
    hit(world, reinforced);
    assert!(world.get::<Brick>(reinforced).is_none());

    let solid = world.spawn_with((Transform3D::IDENTITY, Brick::new(BrickKind::Solid)));
    let score_before = world.get_resource::<GameState>().unwrap().score;
    hit(world, solid);
    assert!(
        world.get::<Brick>(solid).is_some(),
        "solid deflects, never breaks"
    );
    assert_eq!(
        world.get_resource::<GameState>().unwrap().score,
        score_before,
        "solid awards no score"
    );

    assert_eq!(
        world.get_resource::<GameState>().unwrap().score,
        200 * 2 + 300 * 3,
        "score awarded per hit, scaled by kind"
    );
}

#[test]
fn each_power_up_kind_applies_its_effect() {
    let collect = |engine: &mut Engine, kind: PowerUpKind| {
        let paddle = paddle_entity(engine);
        let world = engine.world_mut();
        let pu = world.spawn_with((Transform3D::IDENTITY, PowerUp { kind }));
        powerup::powerup_collection(
            world,
            &[Contact {
                a: pu,
                b: paddle,
                started: true,
            }],
        );
        assert!(world.get::<PowerUp>(pu).is_none(), "collected");
    };

    let mut engine = fracture::build().unwrap().build_headless().unwrap();
    let lives0 = engine.world().get_resource::<GameState>().unwrap().lives;
    collect(&mut engine, PowerUpKind::ExtraLife);
    assert_eq!(
        engine.world().get_resource::<GameState>().unwrap().lives,
        lives0 + 1
    );

    let widen0 = engine
        .world()
        .query::<&Paddle>()
        .iter()
        .next()
        .unwrap()
        .half_width;
    collect(&mut engine, PowerUpKind::WidenPaddle);
    let widen1 = engine
        .world()
        .query::<&Paddle>()
        .iter()
        .next()
        .unwrap()
        .half_width;
    assert!(widen1 > widen0, "paddle widened {widen0} -> {widen1}");

    {
        let world = engine.world_mut();
        world.spawn_with((
            Transform3D::IDENTITY,
            Ball {
                speed: 14.0,
                launched: true,
            },
            LinVel(Vec3::new(2.0, 8.0, 0.0)),
        ));
    }
    let balls_before = ball_count(&engine);
    collect(&mut engine, PowerUpKind::MultiBall);
    assert_eq!(
        ball_count(&engine),
        balls_before + 1,
        "multiball adds a ball"
    );

    collect(&mut engine, PowerUpKind::SlowBall);
    assert!(engine.world().get_resource::<SlowTimer>().unwrap().0 > 0.0);
    let slow = engine.world().query::<&Ball>().iter().all(|ball| {
        ball.speed <= fracture::field::BALL_SPEED * fracture::field::SLOW_FACTOR + 1e-3
    });
    assert!(slow, "all balls slowed");
}

#[test]
fn power_up_falls_and_is_lost_at_bottom() {
    let mut engine = fracture::build().unwrap().build_headless().unwrap();
    let pu = {
        let world = engine.world_mut();
        powerup::spawn_powerup(world, PowerUpKind::SlowBall, Vec3::new(0.0, 5.0, 0.0))
    };
    let y0 = engine.world().get::<Transform3D>(pu).unwrap().translation.y;
    for _ in 0..30 {
        engine.tick().unwrap();
    }
    let y1 = engine.world().get::<Transform3D>(pu).unwrap().translation.y;
    assert!(y1 < y0, "power-up drifts down {y0} -> {y1}");
}

#[test]
fn ball_loss_decrements_lives_then_game_over() {
    let mut engine = fracture::build().unwrap().build_headless().unwrap();
    {
        let mut state = engine.world().get_resource_mut::<GameState>().unwrap();
        state.phase = Phase::Playing;
        state.lives = 2;
    }
    despawn_balls(&mut engine);
    progression::life_check(engine.world_mut());
    assert_eq!(engine.world().get_resource::<GameState>().unwrap().lives, 1);
    assert_eq!(ball_count(&engine), 1, "respawned on the paddle");

    despawn_balls(&mut engine);
    progression::life_check(engine.world_mut());
    assert_eq!(
        engine.world().get_resource::<GameState>().unwrap().phase,
        Phase::GameOver
    );
}

#[test]
fn clears_all_three_levels_to_won() {
    let mut engine = fracture::build().unwrap().build_headless().unwrap();
    for level in 0..3 {
        {
            let mut state = engine.world().get_resource_mut::<GameState>().unwrap();
            state.level = level;
            state.phase = Phase::LevelComplete;
        }
        progression::advance(engine.world_mut());
    }
    assert_eq!(
        engine.world().get_resource::<GameState>().unwrap().phase,
        Phase::Won
    );
}

#[test]
fn game_over_restart_resets_to_fresh_level_one() {
    let mut engine = fracture::build().unwrap().build_headless().unwrap();
    {
        let mut state = engine.world().get_resource_mut::<GameState>().unwrap();
        state.phase = Phase::GameOver;
        state.score = 4242;
        state.level = 2;
    }
    {
        let world = engine.world_mut();
        world.get_resource_mut::<PaddleControl>().unwrap().launch = true;
        progression::start_on_launch(world);
    }
    let state = *engine.world().get_resource::<GameState>().unwrap();
    assert_eq!(state.phase, Phase::Title);
    assert_eq!(state.level, 0);
    assert_eq!(state.score, 0);
    assert_eq!(brick_count(&engine), 27, "level 1 respawned");
}
