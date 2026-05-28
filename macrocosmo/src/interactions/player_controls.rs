use bevy::prelude::*;

pub struct PlayerControlsPlugin;

impl Plugin for PlayerControlsPlugin {
    fn build(&self, app: &mut App) {
        use crate::observer::not_in_observer_mode;

        app.add_systems(Update, log_player_info.run_if(not_in_observer_mode));
    }
}

pub fn log_player_info(
    keys: Res<ButtonInput<KeyCode>>,
    keybindings: Option<Res<crate::input::KeybindingRegistry>>,
    player_q: Query<&crate::player::StationedAt, With<crate::player::Player>>,
    systems: Query<(&crate::galaxy::StarSystem, &crate::components::Position)>,
    all_systems: Query<(
        Entity,
        &crate::galaxy::StarSystem,
        &crate::components::Position,
    )>,
) {
    let pressed = match keybindings.as_deref() {
        Some(kb) => kb.is_just_pressed(crate::input::actions::DEBUG_LOG_PLAYER_INFO, &keys),
        None => keys.just_pressed(KeyCode::KeyI),
    };
    if !pressed {
        return;
    }

    if let Ok(stationed) = player_q.single() {
        if let Ok((current, current_pos)) = systems.get(stationed.system) {
            info!("=== Player Location: {} ===", current.name);
            info!(
                "Position: ({:.1}, {:.1}, {:.1}) ly",
                current_pos.x, current_pos.y, current_pos.z
            );

            info!("--- Nearby Systems ---");
            let mut nearby: Vec<(String, f64, bool)> = Vec::new();
            for (_entity, sys, sys_pos) in &all_systems {
                if sys.name == current.name {
                    continue;
                }
                let dist = crate::physics::distance_ly(current_pos, sys_pos);
                nearby.push((sys.name.clone(), dist, sys.surveyed));
            }
            nearby.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap());

            for (name, dist, surveyed) in nearby.iter().take(10) {
                let survey_mark = if *surveyed { "+" } else { "?" };
                let delay_sd = crate::physics::light_delay_hexadies(*dist);
                info!(
                    "  [{}] {} - {:.1} ly (light delay: {} sd / {:.1} yr)",
                    survey_mark, name, dist, delay_sd, dist
                );
            }
        }
    }
}
