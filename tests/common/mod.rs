use bevy::prelude::*;
use bevy::input::mouse::AccumulatedMouseScroll;
use macrocosmo::colony::*;
use macrocosmo::communication::{self, CommandLog};
use macrocosmo::components::Position;
use macrocosmo::events::{EventLog, GameEvent};
use macrocosmo::galaxy::{Habitability, ResourceLevel, Sovereignty, StarSystem, SystemAttributes};
use macrocosmo::knowledge::*;
use macrocosmo::ship::*;
use macrocosmo::technology::{self};
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
    app.insert_resource(technology::GlobalParams::default());
    app.add_message::<GameEvent>();
    // advance_game_time is a no-op in tests (we manually set clock.elapsed)
    // but must be registered because other systems use .after(advance_game_time)
    app.add_systems(Update, macrocosmo::time_system::advance_game_time);
    app.add_systems(
        Update,
        (
            sublight_movement_system,
            process_ftl_travel,
            process_surveys,
            process_settling,
            process_pending_ship_commands,
            process_command_queue,
            resolve_combat,
        )
            .chain()
            .after(macrocosmo::time_system::advance_game_time),
    );
    app.add_systems(
        Update,
        (
            tick_production,
            tick_maintenance,
            tick_population_growth,
            tick_build_queue,
            tick_building_queue,
            advance_production_tick,
        )
            .chain()
            .after(macrocosmo::time_system::advance_game_time),
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
    app.insert_resource(visualization::ContextMenu::default());
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
    app.insert_resource(technology::GlobalParams::default());
    app.insert_resource(technology::GameFlags::default());

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
            resolve_combat,
        ),
    );

    // --- Colony systems (from ColonyPlugin) ---
    app.add_systems(
        Update,
        (
            tick_production,
            tick_maintenance,
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
        (
            technology::emit_research,
            technology::receive_research,
            technology::tick_research,
            technology::flush_research,
        )
            .chain(),
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
    app.add_systems(
        Update,
        (
            visualization::camera_controls,
        ),
    );

    app
}

/// Advance the game clock by `hexadies` and run one update cycle.
pub fn advance_time(app: &mut App, hexadies: i64) {
    app.world_mut().resource_mut::<GameClock>().elapsed += hexadies;
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
            Sovereignty::default(),
        ))
        .id()
}

/// Spawn a colony with all required components at the given system entity.
pub fn spawn_test_colony(
    world: &mut World,
    system: Entity,
    minerals: f64,
    energy: f64,
    buildings: Vec<Option<BuildingType>>,
) -> Entity {
    world
        .spawn((
            Colony {
                system,
                population: 100.0,
                growth_rate: 0.01,
            },
            ResourceStockpile {
                minerals,
                energy,
                research: 0.0,
            },
            Production {
                minerals_per_hexadies: 5.0,
                energy_per_hexadies: 5.0,
                research_per_hexadies: 1.0,
            },
            BuildQueue {
                queue: Vec::new(),
            },
            Buildings { slots: buildings },
            BuildingQueue::default(),
            ProductionFocus::default(),
        ))
        .id()
}

/// Spawn a ship with all standard components at the given system.
pub fn spawn_test_ship(
    world: &mut World,
    name: &str,
    ship_type: ShipType,
    system: Entity,
    pos: [f64; 3],
) -> Entity {
    let hp = ship_type.default_hp();
    let combat_stats = ship_type.default_combat_stats();
    world
        .spawn((
            Ship {
                name: name.to_string(),
                ship_type,
                owner: Owner::Player,
                sublight_speed: ship_type.default_sublight_speed(),
                ftl_range: ship_type.default_ftl_range(),
                hp,
                max_hp: hp,
                player_aboard: false,
                home_port: system,
            },
            ShipState::Docked { system },
            Position::from(pos),
            CombatStats {
                attack: combat_stats.attack,
                defense: combat_stats.defense,
            },
            CommandQueue::default(),
            Cargo::default(),
        ))
        .id()
}
