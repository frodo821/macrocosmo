use bevy::prelude::*;
use macrocosmo::colony::*;
use macrocosmo::communication::CommandLog;
use macrocosmo::components::Position;
use macrocosmo::events::{EventLog, GameEvent};
use macrocosmo::galaxy::{Habitability, ResourceLevel, StarSystem, SystemAttributes};
use macrocosmo::knowledge::*;
use macrocosmo::ship::*;
use macrocosmo::time_system::{GameClock, GameSpeed};

/// Build a headless Bevy App with game logic systems but no rendering.
pub fn test_app() -> App {
    let mut app = App::new();
    app.add_plugins(MinimalPlugins);
    app.insert_resource(GameClock::new(0));
    app.insert_resource(GameSpeed::default());
    app.insert_resource(KnowledgeStore::default());
    app.insert_resource(CommandLog::default());
    app.insert_resource(LastProductionTick(0));
    app.insert_resource(EventLog::default());
    app.add_message::<GameEvent>();
    // Register Update systems in correct order
    app.add_systems(
        Update,
        (
            sublight_movement_system,
            process_ftl_travel,
            process_surveys,
            handle_colony_ship_arrival,
        )
            .chain(),
    );
    app.add_systems(
        Update,
        (
            tick_production,
            tick_population_growth,
            tick_build_queue,
            advance_production_tick,
        )
            .chain(),
    );
    app.add_systems(Update, propagate_knowledge);
    app
}

/// Advance the game clock by `sexadies` and run one update cycle.
pub fn advance_time(app: &mut App, sexadies: i64) {
    app.world_mut().resource_mut::<GameClock>().elapsed += sexadies;
    app.update();
}

/// Spawn a star system entity with the given attributes.
pub fn spawn_test_system(
    world: &mut World,
    name: &str,
    pos: [f64; 3],
    hab: Habitability,
    surveyed: bool,
    colonized: bool,
) -> Entity {
    world
        .spawn((
            StarSystem {
                name: name.to_string(),
                surveyed,
                colonized,
                is_capital: false,
            },
            Position::from(pos),
            SystemAttributes {
                habitability: hab,
                mineral_richness: ResourceLevel::Moderate,
                energy_potential: ResourceLevel::Moderate,
                research_potential: ResourceLevel::Moderate,
                max_building_slots: 4,
            },
        ))
        .id()
}
