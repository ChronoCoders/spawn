use spawn_ecs::World;

use crate::resources::{Collisions, Contact};
use crate::{audio, ball, brick, paddle, powerup, progression};

pub fn gameplay(world: &mut World) {
    paddle::paddle_tracking(world);
    ball::ball_launch_and_speed(world);

    let contacts: Vec<Contact> = world
        .get_resource::<Collisions>()
        .map(|collisions| collisions.contacts.clone())
        .unwrap_or_default();

    ball::ball_response(world, &contacts);
    audio::hit_cues(world, &contacts);
    brick::brick_collisions(world, &contacts);
    powerup::powerup_collection(world, &contacts);
    progression::ball_lost(world, &contacts);
    brick::level_clear(world);
    progression::life_check(world);
    progression::advance(world);
    progression::start_on_launch(world);
    powerup::tick_slow(world);
}
