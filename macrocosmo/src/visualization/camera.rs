use bevy::input::mouse::AccumulatedMouseScroll;
use bevy::prelude::*;

use super::{EguiWantsPointer, GalaxyView};
use crate::components::Position;
use crate::galaxy::StarSystem;

pub fn setup_camera(mut commands: Commands) {
    commands.spawn((
        Camera2d,
        Camera {
            clear_color: ClearColorConfig::Custom(Color::srgb(0.02, 0.02, 0.05)),
            ..default()
        },
    ));
}

pub fn center_camera_on_capital(
    mut camera_q: Query<&mut Transform, With<Camera2d>>,
    capitals: Query<(&StarSystem, &Position)>,
    view: Res<GalaxyView>,
) {
    for (star, pos) in &capitals {
        if star.is_capital {
            for mut transform in &mut camera_q {
                transform.translation.x = pos.x as f32 * view.scale;
                transform.translation.y = pos.y as f32 * view.scale;
            }
            break;
        }
    }
}

pub fn camera_controls(
    keys: Res<ButtonInput<KeyCode>>,
    keybindings: Option<Res<crate::input::KeybindingRegistry>>,
    time: Res<Time>,
    mut camera_q: Query<(&mut Transform, &mut Projection), With<Camera2d>>,
    scroll: Res<AccumulatedMouseScroll>,
    egui_wants_pointer: Option<Res<EguiWantsPointer>>,
) {
    let Ok((mut transform, mut projection)) = camera_q.single_mut() else {
        return;
    };

    let current_scale = if let Projection::Orthographic(ref ortho) = *projection {
        ortho.scale
    } else {
        1.0
    };

    let pan_speed = 300.0 * current_scale * time.delta_secs();

    // #347: WASD + arrow-key panning routed through the keybinding registry.
    // Headless tests without `KeybindingPlugin` skip pan/zoom-by-key but
    // mouse-scroll zoom still works (it doesn't need keybindings).
    if let Some(keybindings) = keybindings.as_deref() {
        use crate::input::actions;
        let pressed = |id: &str| keybindings.is_pressed(id, &keys);
        if pressed(actions::CAMERA_PAN_UP) || pressed(actions::CAMERA_PAN_UP_ALT) {
            transform.translation.y += pan_speed;
        }
        if pressed(actions::CAMERA_PAN_DOWN) || pressed(actions::CAMERA_PAN_DOWN_ALT) {
            transform.translation.y -= pan_speed;
        }
        if pressed(actions::CAMERA_PAN_LEFT) || pressed(actions::CAMERA_PAN_LEFT_ALT) {
            transform.translation.x -= pan_speed;
        }
        if pressed(actions::CAMERA_PAN_RIGHT) || pressed(actions::CAMERA_PAN_RIGHT_ALT) {
            transform.translation.x += pan_speed;
        }
        if keybindings.is_just_pressed(actions::CAMERA_RECENTER, &keys) {
            transform.translation.x = 0.0;
            transform.translation.y = 0.0;
            if let Projection::Orthographic(ref mut ortho) = *projection {
                ortho.scale = 1.0;
            }
        }
    }

    let egui_wants_input = egui_wants_pointer.is_some_and(|r| r.0);

    if scroll.delta.y != 0.0 && !egui_wants_input {
        let zoom_delta = -scroll.delta.y * 0.1;
        if let Projection::Orthographic(ref mut ortho) = *projection {
            ortho.scale = (ortho.scale + zoom_delta).clamp(0.2, 10.0);
        }
    }
}
