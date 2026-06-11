use fracture::ball;
use fracture::components::{Brick, BrickKind};
use fracture::field;
use fracture::levels;
use fracture::resources::GameRng;
use spawn_core::Vec3;

fn close(a: f32, b: f32) -> bool {
    (a - b).abs() < 1e-3
}

#[test]
fn renormalize_sets_length_and_handles_zero() {
    let v = ball::renormalize(Vec3::new(3.0, 4.0, 0.0), 10.0);
    assert!(close(v.length(), 10.0), "{}", v.length());
    assert_eq!(ball::renormalize(Vec3::ZERO, 10.0), Vec3::ZERO);
}

#[test]
fn launch_velocity_is_upward_at_ball_speed() {
    let v = ball::launch_velocity();
    assert!(close(v.length(), field::BALL_SPEED));
    assert!(v.y > 0.0, "launches upward");
}

#[test]
fn maintain_speed_nudges_near_horizontal_off_axis() {
    let v = ball::maintain_speed(Vec3::new(1.0, 0.0, 0.0), 10.0);
    assert!(close(v.length(), 10.0));
    assert!(
        v.y.abs() >= 10.0 * 0.25 - 1e-3,
        "vertical nudge applied: {}",
        v.y
    );
}

#[test]
fn paddle_english_steers_toward_contact_offset() {
    let speed = 12.0;
    let right = ball::paddle_english(Vec3::new(0.0, -8.0, 0.0), 1.5, 0.0, speed);
    assert!(right.x > 0.0, "ball right of center steers right");
    assert!(right.y > 0.0, "bounces upward");
    assert!(close(right.length(), speed));
    let left = ball::paddle_english(Vec3::new(0.0, -8.0, 0.0), -1.5, 0.0, speed);
    assert!(left.x < 0.0, "ball left of center steers left");
}

#[test]
fn brick_kind_health_breakable_and_score() {
    assert_eq!(BrickKind::Normal.initial_health(), 1);
    assert_eq!(BrickKind::Tough.initial_health(), 2);
    assert_eq!(BrickKind::Reinforced.initial_health(), 3);
    assert_eq!(BrickKind::Solid.initial_health(), 0);
    assert!(BrickKind::Normal.is_breakable());
    assert!(!BrickKind::Solid.is_breakable());
    assert_eq!(BrickKind::Normal.score(), 100);
    assert_eq!(BrickKind::Reinforced.score(), 300);
    assert_eq!(BrickKind::Solid.score(), 0);
    assert_eq!(Brick::new(BrickKind::Tough).health, 2);
}

#[test]
fn level_layouts_and_drop_chance() {
    assert_eq!(levels::brick_specs(0).len(), 27);
    assert!(levels::brick_specs(0)
        .iter()
        .all(|(k, _)| *k == BrickKind::Normal));
    let staircase = levels::brick_specs(1).len();
    let fortress = levels::brick_specs(2).len();
    assert!(staircase > 0);
    assert!(
        fortress > staircase,
        "fortress {fortress} > staircase {staircase}"
    );
    for (_, position) in levels::brick_specs(2) {
        assert!(
            position.x.abs() < field::HALF_WIDTH,
            "brick within field: {}",
            position.x
        );
    }
    assert_eq!(levels::drop_chance(0), 0.2);
    assert!(levels::drop_chance(2) > levels::drop_chance(0));
}

#[test]
fn fortress_has_breakable_reinforced_and_unbreakable_solid() {
    let specs = levels::brick_specs(2);
    assert!(specs.iter().any(|(k, _)| *k == BrickKind::Reinforced));
    assert!(specs.iter().any(|(k, _)| *k == BrickKind::Solid));
    let breakable = specs.iter().filter(|(k, _)| k.is_breakable()).count();
    assert!(
        breakable > 0 && breakable < specs.len(),
        "some breakable, some not"
    );
}

#[test]
fn game_rng_is_deterministic_and_bounded() {
    let mut a = GameRng::seeded(5);
    let mut b = GameRng::seeded(5);
    for _ in 0..32 {
        assert_eq!(a.next_u64(), b.next_u64());
    }
    let mut rng = GameRng::seeded(9);
    for _ in 0..100 {
        let u = rng.next_unit();
        assert!((0.0..1.0).contains(&u), "unit in [0,1): {u}");
        assert!(rng.below(4) < 4);
    }
    assert_eq!(GameRng::seeded(1).below(0), 0);
    assert!(!GameRng::seeded(2).chance(0.0));
    assert!(GameRng::seeded(2).chance(1.0));
}
