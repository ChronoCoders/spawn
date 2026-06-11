use spawn_core::Transform3D;
use spawn_ecs::World;

use crate::components::Paddle;
use crate::field;
use crate::resources::{InputSource, PaddleControl, PaddleState};

pub fn paddle_tracking(world: &mut World) {
    let control = match world.get_resource::<PaddleControl>() {
        Some(control) => *control,
        None => return,
    };
    let mut moved_x = None;
    for (transform, paddle) in world.query_mut::<(&mut Transform3D, &Paddle)>().iter_mut() {
        let mut x = transform.translation.x;
        match control.source {
            InputSource::Keys => {
                x += control.key_intent * field::PADDLE_SPEED * field::FIXED_TIMESTEP;
            }
            InputSource::Mouse => x = control.mouse_x,
        }
        x = x.clamp(paddle.min_x, paddle.max_x);
        transform.translation.x = x;
        moved_x = Some(x);
    }
    if let (Some(x), Some(mut state)) = (moved_x, world.get_resource_mut::<PaddleState>()) {
        state.x = x;
    }
}
