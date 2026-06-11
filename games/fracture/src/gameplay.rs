use spawn_ecs::World;

use crate::resources::{Collisions, Contact};
use crate::{audio, ball, brick, paddle, powerup};

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
    brick::level_clear(world);
    powerup::tick_slow(world);
}
