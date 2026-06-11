use spawn_ecs::{Commands, EcsResult, Res, ResMut};
use spawn_engine::{InputFrame, KeyCode};

use crate::field;
use crate::resources::{InputSource, PaddleControl};

pub fn sample_input(
    input: Res<'_, InputFrame>,
    mut control: ResMut<'_, PaddleControl>,
    _commands: &mut Commands<'_>,
) -> EcsResult<()> {
    let keyboard = input.keyboard();
    let left = keyboard.is_pressed(KeyCode::ArrowLeft) || keyboard.is_pressed(KeyCode::A);
    let right = keyboard.is_pressed(KeyCode::ArrowRight) || keyboard.is_pressed(KeyCode::D);
    let intent = (i32::from(right) - i32::from(left)) as f32;
    control.key_intent = intent;
    control.launch = keyboard.just_pressed(KeyCode::Space);

    let mouse = input.mouse();
    if intent != 0.0 {
        control.source = InputSource::Keys;
    } else if mouse.delta().x != 0.0 {
        control.source = InputSource::Mouse;
    }
    control.mouse_x = (mouse.position().x / field::VIEWPORT_WIDTH - 0.5) * field::FIELD_WIDTH;
    Ok(())
}
