use bevy::prelude::*;
use bevy::input::mouse::AccumulatedMouseScroll;
use macrocosmo::colony::*;
use macrocosmo::communication::{self, CommandLog};
use macrocosmo::components::Position;
use macrocosmo::events::{EventLog, GameEvent};
use macrocosmo::galaxy::{Habitability, ResourceLevel, StarSystem, SystemAttributes};
use macrocosmo::knowledge::*;
use macrocosmo::ship::*;
use macrocosmo::technology::{self, TechTree};
use macrocosmo::time_system::{GameClock, GameSpeed};
use macrocosmo::visualization;

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
            process_settling,
            process_pending_ship_commands,
            process_command_queue,
        )
            .chain(),
    );
    app.add_systems(
        Update,
        (
            tick_production,
            tick_population_growth,
            tick_build_queue,
            tick_building_queue,
            advance_production_tick,
        )
            .chain(),
    );
    app.add_systems(Update, propagate_knowledge);
    app
}

/// Build a headless Bevy App with ALL game systems registered (including
/// visualization logic systems) so Bevy validates there are no Query
/// conflicts (B0001). Systems that require Gizmos are excluded since the
/// GizmoPlugin is not available in headless mode, but all other systems
/// are included -- they will simply early-return when their queries find
/// no matching entities.
pub fn full_test_app() -> App {
    let mut app = App::new();
    app.add_plugins(MinimalPlugins);

    // --- Core resources ---
    app.insert_resource(GameClock::new(0));
    app.insert_resource(GameSpeed::default());
    app.insert_resource(KnowledgeStore::default());
    app.insert_resource(CommandLog::default());
    app.insert_resource(LastProductionTick(0));
    app.insert_resource(EventLog::default());
    app.add_message::<GameEvent>();

    // --- Visualization resources ---
    app.insert_resource(visualization::SelectedSystem::default());
    app.insert_resource(visualization::SelectedShip::default());
    app.insert_resource(visualization::GalaxyView { scale: 5.0 });

    // --- Input resources (needed by visualization + time_system + player systems) ---
    app.insert_resource(ButtonInput::<KeyCode>::default());
    app.insert_resource(ButtonInput::<MouseButton>::default());
    app.insert_resource(AccumulatedMouseScroll::default());

    // --- Technology resources ---
    app.insert_resource(technology::create_initial_tech_tree());
    app.insert_resource(technology::ResearchQueue::default());
    app.insert_resource(technology::ResearchPool::default());
    app.insert_resource(technology::LastResearchTick(0));

    // --- Ship systems (from ShipPlugin) ---
    app.add_systems(
        Update,
        (
            sublight_movement_system,
            process_ftl_travel,
            process_surveys,
            process_settling,
            process_pending_ship_commands,
            process_command_queue,
        ),
    );

    // --- Colony systems (from ColonyPlugin) ---
    app.add_systems(
        Update,
        (
            tick_production,
            tick_population_growth,
            tick_build_queue,
            tick_building_queue,
            advance_production_tick,
        )
            .chain(),
    );
    app.add_systems(Update, update_sovereignty);

    // --- Knowledge system (from KnowledgePlugin) ---
    app.add_systems(Update, propagate_knowledge);

    // --- Communication systems (from CommunicationPlugin) ---
    app.add_systems(
        Update,
        (
            communication::process_messages,
            communication::process_courier_ships,
            communication::process_pending_commands,
        ),
    );

    // --- Technology systems (from TechnologyPlugin) ---
    app.add_systems(
        Update,
        (technology::collect_research, technology::tick_research).chain(),
    );

    // --- Events systems (from EventsPlugin) ---
    app.add_systems(
        Update,
        (
            macrocosmo::events::collect_events,
            macrocosmo::events::auto_pause_on_event,
        ),
    );

    // --- Time systems (from GameTimePlugin) ---
    app.add_systems(
        Update,
        (
            macrocosmo::time_system::advance_game_time,
            macrocosmo::time_system::handle_speed_controls,
        ),
    );

    // --- Player system (from PlayerPlugin, excluding Startup spawn_player) ---
    app.add_systems(Update, macrocosmo::player::log_player_info);

    // --- Visualization systems (excluding Gizmos-dependent ones) ---
    // These systems use standard Res/Query params and will early-return
    // when no matching entities exist. The key purpose is Bevy validating
    // their Query parameters don't conflict with each other.
    // NOTE: UI systems (egui) are excluded from tests because they require
    // EguiPlugin which is heavy and needs rendering context.
    app.add_systems(
        Update,
        (
            visualization::click_select_system,
            visualization::camera_controls,
            visualization::handle_ship_commands,
        ),
    );
    // NOTE: draw_galaxy_overlay and draw_ships are excluded because they
    // require the Gizmos system parameter which needs GizmoPlugin.

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
