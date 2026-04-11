use bevy::prelude::*;
use bevy::input::mouse::AccumulatedMouseScroll;
use macrocosmo::amount::Amt;
use macrocosmo::colony::*;
use macrocosmo::scripting::building_api::BuildingId;
use macrocosmo::species;
use macrocosmo::communication::{self, CommandLog};
use macrocosmo::components::Position;
use macrocosmo::event_system::{EventBus, EventSystem};
use macrocosmo::events::{EventLog, GameEvent};
use macrocosmo::galaxy::{Anomalies, Habitability, Planet, ResourceLevel, Sovereignty, StarSystem, SystemAttributes, SystemModifiers};
use macrocosmo::knowledge::*;
use macrocosmo::modifier::ModifiedValue;
use macrocosmo::condition::ScopedFlags;
use macrocosmo::player::{Empire, PlayerEmpire};
use macrocosmo::ship::*;
use macrocosmo::technology::{self, TechKnowledge};
use macrocosmo::time_system::{GameClock, GameSpeed};
use macrocosmo::visualization;

/// Create a BuildingRegistry populated with the standard 6 building definitions for tests.
pub fn create_test_building_registry() -> macrocosmo::colony::BuildingRegistry {
    use macrocosmo::scripting::building_api::BuildingDefinition;
    use std::collections::HashMap;
    let mut registry = macrocosmo::colony::BuildingRegistry::default();
    registry.insert(BuildingDefinition {
        id: "mine".into(), name: "Mine".into(), description: String::new(),
        minerals_cost: Amt::units(150), energy_cost: Amt::units(50), build_time: 10,
        maintenance: Amt::new(0, 200),
        production_bonus_minerals: Amt::units(3), production_bonus_energy: Amt::ZERO,
        production_bonus_research: Amt::ZERO, production_bonus_food: Amt::ZERO,
        is_system_building: false, capabilities: HashMap::new(),
        upgrade_to: Vec::new(), is_direct_buildable: true,
    });
    registry.insert(BuildingDefinition {
        id: "power_plant".into(), name: "PowerPlant".into(), description: String::new(),
        minerals_cost: Amt::units(50), energy_cost: Amt::units(150), build_time: 10,
        maintenance: Amt::ZERO,
        production_bonus_minerals: Amt::ZERO, production_bonus_energy: Amt::units(3),
        production_bonus_research: Amt::ZERO, production_bonus_food: Amt::ZERO,
        is_system_building: false, capabilities: HashMap::new(),
        upgrade_to: Vec::new(), is_direct_buildable: true,
    });
    registry.insert(BuildingDefinition {
        id: "research_lab".into(), name: "ResearchLab".into(), description: String::new(),
        minerals_cost: Amt::units(100), energy_cost: Amt::units(100), build_time: 15,
        maintenance: Amt::new(0, 500),
        production_bonus_minerals: Amt::ZERO, production_bonus_energy: Amt::ZERO,
        production_bonus_research: Amt::units(2), production_bonus_food: Amt::ZERO,
        is_system_building: true, capabilities: HashMap::new(),
        upgrade_to: Vec::new(), is_direct_buildable: true,
    });
    registry.insert(BuildingDefinition {
        id: "shipyard".into(), name: "Shipyard".into(), description: String::new(),
        minerals_cost: Amt::units(300), energy_cost: Amt::units(200), build_time: 30,
        maintenance: Amt::units(1),
        production_bonus_minerals: Amt::ZERO, production_bonus_energy: Amt::ZERO,
        production_bonus_research: Amt::ZERO, production_bonus_food: Amt::ZERO,
        is_system_building: true, capabilities: HashMap::new(),
        upgrade_to: Vec::new(), is_direct_buildable: true,
    });
    registry.insert(BuildingDefinition {
        id: "port".into(), name: "Port".into(), description: String::new(),
        minerals_cost: Amt::units(400), energy_cost: Amt::units(300), build_time: 40,
        maintenance: Amt::new(0, 500),
        production_bonus_minerals: Amt::ZERO, production_bonus_energy: Amt::ZERO,
        production_bonus_research: Amt::ZERO, production_bonus_food: Amt::ZERO,
        is_system_building: true, capabilities: HashMap::new(),
        upgrade_to: Vec::new(), is_direct_buildable: true,
    });
    registry.insert(BuildingDefinition {
        id: "farm".into(), name: "Farm".into(), description: String::new(),
        minerals_cost: Amt::units(100), energy_cost: Amt::units(50), build_time: 20,
        maintenance: Amt::new(0, 300),
        production_bonus_minerals: Amt::ZERO, production_bonus_energy: Amt::ZERO,
        production_bonus_research: Amt::ZERO, production_bonus_food: Amt::units(5),
        is_system_building: false, capabilities: HashMap::new(),
        upgrade_to: Vec::new(), is_direct_buildable: true,
    });
    registry
}

/// Spawn a player empire entity with all empire-level components.
/// Returns the empire entity.
pub fn spawn_test_empire(world: &mut World) -> Entity {
    world
        .spawn((
            Empire {
                name: "Test Empire".into(),
            },
            PlayerEmpire,
            technology::TechTree::default(),
            technology::ResearchQueue::default(),
            technology::ResearchPool::default(),
            technology::RecentlyResearched::default(),
            AuthorityParams::default(),
            ConstructionParams::default(),
            technology::EmpireModifiers::default(),
            technology::GameFlags::default(),
            technology::GlobalParams::default(),
            KnowledgeStore::default(),
            CommandLog::default(),
            ScopedFlags::default(),
        ))
        .id()
}

/// Build a headless Bevy App with game logic systems but no rendering.
pub fn test_app() -> App {
    let mut app = App::new();
    app.add_plugins(MinimalPlugins);
    app.insert_resource(GameClock::new(0));
    app.insert_resource(GameSpeed::default());
    app.insert_resource(LastProductionTick(0));
    app.insert_resource(EventLog::default());
    app.insert_resource(EventSystem::default());
    app.insert_resource(EventBus::default());
    app.insert_resource(technology::LastResearchTick(0));
    app.init_resource::<species::SpeciesRegistry>();
    app.init_resource::<species::JobRegistry>();
    app.init_resource::<AlertCooldowns>();
    app.insert_resource(create_test_building_registry());
    app.init_resource::<macrocosmo::ship_design::ModuleRegistry>();
    app.init_resource::<macrocosmo::ship_design::HullRegistry>();
    app.insert_resource(create_test_design_registry());
    app.add_message::<GameEvent>();
    // advance_game_time is a no-op in tests (we manually set clock.elapsed)
    // but must be registered because other systems use .after(advance_game_time)
    app.init_resource::<macrocosmo::ship::routing::RouteCalculationsPending>();
    app.add_systems(Update, macrocosmo::time_system::advance_game_time);
    app.add_systems(
        Update,
        (
            sync_ship_module_modifiers,
            sync_ship_hitpoints,
            tick_shield_regen,
            sublight_movement_system,
            process_ftl_travel,
            deliver_survey_results,
            process_surveys,
            process_settling,
            process_pending_ship_commands,
            process_command_queue,
            resolve_combat,
            tick_ship_repair,
        )
            .chain()
            .after(macrocosmo::time_system::advance_game_time)
            .before(advance_production_tick),
    );
    // #128: Poll route tasks after Commands from process_command_queue are flushed.
    app.add_systems(
        Update,
        (
            bevy::ecs::schedule::ApplyDeferred,
            macrocosmo::ship::routing::poll_pending_routes,
        )
            .chain()
            .after(process_command_queue)
            .after(macrocosmo::time_system::advance_game_time)
            .before(advance_production_tick),
    );
    app.add_systems(
        Update,
        (
            tick_timed_effects,
            tick_authority,
            sync_building_modifiers,
            sync_maintenance_modifiers,
            sync_food_consumption,
            tick_production,
            tick_maintenance,
            tick_population_growth,
            tick_build_queue,
            tick_building_queue,
            tick_colonization_queue,
            check_resource_alerts,
            advance_production_tick,
        )
            .chain()
            .after(macrocosmo::time_system::advance_game_time),
    );
    app.add_systems(
        Update,
        (
            species::sync_job_assignment,
            apply_pending_colonization_orders,
        ).after(macrocosmo::time_system::advance_game_time),
    );
    app.add_systems(
        Update,
        macrocosmo::event_system::tick_events
            .after(macrocosmo::time_system::advance_game_time)
            .after(tick_timed_effects),
    );
    app.add_systems(Update, propagate_knowledge);
    // #59: Player location tracking (after ship movement systems)
    app.add_systems(
        Update,
        macrocosmo::player::update_player_location
            .after(macrocosmo::time_system::advance_game_time)
            .after(sublight_movement_system)
            .after(process_ftl_travel),
    );

    // Spawn the empire entity
    spawn_test_empire(app.world_mut());

    app
}

/// Like test_app() but also registers collect_events so GameEvents are
/// collected into EventLog. Needed for tests that check EventLog entries.
/// NOTE: Do not combine with tests that rely on EventSystem.fired_log timing,
/// because the extra MessageReader<GameEvent> system can alter scheduling.
pub fn test_app_with_event_log() -> App {
    let mut app = test_app();
    app.add_systems(
        Update,
        macrocosmo::events::collect_events
            .after(macrocosmo::time_system::advance_game_time),
    );
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
    app.insert_resource(LastProductionTick(0));
    app.insert_resource(EventLog::default());
    app.insert_resource(EventSystem::default());
    app.init_resource::<species::SpeciesRegistry>();
    app.init_resource::<species::JobRegistry>();
    app.init_resource::<AlertCooldowns>();
    app.insert_resource(create_test_building_registry());
    app.init_resource::<macrocosmo::ship_design::ModuleRegistry>();
    app.init_resource::<macrocosmo::ship_design::HullRegistry>();
    app.insert_resource(create_test_design_registry());
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

    // --- Technology resources (only LastResearchTick remains as a global resource) ---
    app.insert_resource(technology::LastResearchTick(0));

    // --- Routing resource ---
    app.init_resource::<macrocosmo::ship::routing::RouteCalculationsPending>();

    // --- Ship systems (from ShipPlugin) ---
    app.add_systems(
        Update,
        (
            sync_ship_module_modifiers,
            sync_ship_hitpoints,
            tick_shield_regen,
            sublight_movement_system,
            process_ftl_travel,
            deliver_survey_results,
            process_surveys,
            process_settling,
            process_pending_ship_commands,
            process_command_queue,
            resolve_combat,
            tick_ship_repair,
        ),
    );
    // #128: Poll route tasks after Commands from process_command_queue are flushed.
    app.add_systems(
        Update,
        (
            bevy::ecs::schedule::ApplyDeferred,
            macrocosmo::ship::routing::poll_pending_routes,
        )
            .chain()
            .after(process_command_queue),
    );

    // --- Colony systems (from ColonyPlugin) ---
    app.add_systems(
        Update,
        (
            tick_timed_effects,
            tick_authority,
            sync_building_modifiers,
            sync_maintenance_modifiers,
            sync_food_consumption,
            tick_production,
            tick_maintenance,
            tick_population_growth,
            tick_build_queue,
            tick_building_queue,
            tick_colonization_queue,
            check_resource_alerts,
            advance_production_tick,
        )
            .chain(),
    );
    app.add_systems(Update, (update_sovereignty, apply_pending_colonization_orders));

    // --- Species systems (from SpeciesPlugin) ---
    app.add_systems(Update, species::sync_job_assignment);

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
    app.add_systems(
        Update,
        (
            technology::propagate_tech_knowledge,
            technology::receive_tech_knowledge,
        )
            .chain()
            .after(technology::tick_research),
    );

    // --- Events systems (from EventsPlugin + EventSystemPlugin) ---
    app.add_systems(
        Update,
        (
            macrocosmo::events::collect_events,
            macrocosmo::events::auto_pause_on_event,
        ),
    );
    app.add_systems(
        Update,
        macrocosmo::event_system::tick_events,
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
    app.add_systems(Update, macrocosmo::player::update_player_location);

    // --- Visualization systems (excluding Gizmos-dependent ones) ---
    app.add_systems(
        Update,
        (
            visualization::camera_controls,
        ),
    );

    // Spawn the empire entity
    spawn_test_empire(app.world_mut());

    app
}

/// Advance the game clock by `hexadies` and run one update cycle.
pub fn advance_time(app: &mut App, hexadies: i64) {
    app.world_mut().resource_mut::<GameClock>().elapsed += hexadies;
    app.update();
}

/// Spawn a star system entity with the given attributes.
/// Also spawns a default planet. Returns the star system entity.
/// Use `spawn_test_system_with_planet` to get both entities.
pub fn spawn_test_system(
    world: &mut World,
    name: &str,
    pos: [f64; 3],
    hab: Habitability,
    surveyed: bool,
    _colonized: bool,
) -> Entity {
    let (sys, _planet) = spawn_test_system_with_planet(world, name, pos, hab, surveyed);
    sys
}

/// Spawn a star system with a default planet. Returns (system_entity, planet_entity).
pub fn spawn_test_system_with_planet(
    world: &mut World,
    name: &str,
    pos: [f64; 3],
    hab: Habitability,
    surveyed: bool,
) -> (Entity, Entity) {
    let sys = world
        .spawn((
            StarSystem {
                name: name.to_string(),
                surveyed,
                is_capital: false,
                star_type: "default".to_string(),
            },
            Position::from(pos),
            Sovereignty::default(),
            TechKnowledge::default(),
            SystemModifiers::default(),
            Anomalies::default(),
        ))
        .id();

    let planet = world
        .spawn((
            Planet {
                name: format!("{} I", name),
                system: sys,
                planet_type: "default".to_string(),
            },
            SystemAttributes {
                habitability: hab,
                mineral_richness: ResourceLevel::Moderate,
                energy_potential: ResourceLevel::Moderate,
                research_potential: ResourceLevel::Moderate,
                max_building_slots: 4,
            },
            Position::from(pos),
        ))
        .id();

    (sys, planet)
}

/// Spawn a colony with all required components.
/// `system_or_planet` can be either a StarSystem entity (will auto-find first planet)
/// or a Planet entity directly.
pub fn spawn_test_colony(
    world: &mut World,
    system_or_planet: Entity,
    minerals: Amt,
    energy: Amt,
    buildings: Vec<Option<BuildingId>>,
) -> Entity {
    // Check if the entity is a Planet or a StarSystem; find the planet entity accordingly
    let (planet, system) = if world.get::<Planet>(system_or_planet).is_some() {
        let p = world.get::<Planet>(system_or_planet).unwrap();
        let sys = p.system;
        (system_or_planet, sys)
    } else {
        // It's a system entity; find its first planet
        let planet = find_planet(world, system_or_planet);
        (planet, system_or_planet)
    };

    // Known system building ids
    let system_building_ids = ["shipyard", "research_lab", "port"];

    // Separate buildings into planet and system buildings
    let mut planet_buildings = Vec::new();
    let mut system_building_slots: Vec<Option<BuildingId>> = vec![None; DEFAULT_SYSTEM_BUILDING_SLOTS];
    let mut sys_slot_idx = 0;
    for b in &buildings {
        if let Some(bid) = b {
            if system_building_ids.contains(&bid.as_str()) {
                if sys_slot_idx < system_building_slots.len() {
                    system_building_slots[sys_slot_idx] = Some(bid.clone());
                    sys_slot_idx += 1;
                }
            } else {
                planet_buildings.push(Some(bid.clone()));
            }
        } else {
            planet_buildings.push(None);
        }
    }

    // Add ResourceStockpile and ResourceCapacity to the StarSystem if not already present
    if world.get::<ResourceStockpile>(system).is_none() {
        world.entity_mut(system).insert((
            ResourceStockpile {
                minerals,
                energy,
                research: Amt::ZERO,
                food: Amt::units(100),
                authority: Amt::ZERO,
            },
            ResourceCapacity::default(),
        ));
    }

    // Add SystemBuildings and SystemBuildingQueue to the StarSystem if not already present
    if world.get::<SystemBuildings>(system).is_none() {
        world.entity_mut(system).insert((
            SystemBuildings { slots: system_building_slots },
            SystemBuildingQueue::default(),
        ));
    }

    world
        .spawn((
            Colony {
                planet,
                population: 100.0,
                growth_rate: 0.01,
            },
            Production {
                minerals_per_hexadies: ModifiedValue::new(Amt::units(5)),
                energy_per_hexadies: ModifiedValue::new(Amt::units(5)),
                research_per_hexadies: ModifiedValue::new(Amt::units(1)),
                food_per_hexadies: ModifiedValue::new(Amt::ZERO),
            },
            BuildQueue {
                queue: Vec::new(),
            },
            Buildings { slots: planet_buildings },
            BuildingQueue::default(),
            ProductionFocus::default(),
            MaintenanceCost::default(),
            FoodConsumption::default(),
        ))
        .id()
}

/// Find the first planet entity belonging to a star system.
/// Useful in tests when you only have the system entity.
pub fn find_planet(world: &mut World, system: Entity) -> Entity {
    let mut query = world.query::<(Entity, &Planet)>();
    let result: Option<Entity> = {
        let mut found = None;
        for (entity, planet) in query.iter(world) {
            if planet.system == system {
                found = Some(entity);
                break;
            }
        }
        found
    };
    result.unwrap_or_else(|| panic!("No planet found for system {:?}", system))
}

/// Find the player empire entity in the world.
pub fn empire_entity(world: &mut World) -> Entity {
    let mut query = world.query_filtered::<Entity, With<PlayerEmpire>>();
    query.single(world).expect("No player empire found in test world")
}

/// Create a ShipDesignRegistry populated with the standard 4 ship designs for tests.
pub fn create_test_design_registry() -> macrocosmo::ship_design::ShipDesignRegistry {
    use macrocosmo::ship_design::{ShipDesignDefinition, ShipDesignRegistry};
    let mut registry = ShipDesignRegistry::default();
    registry.insert(ShipDesignDefinition {
        id: "explorer_mk1".to_string(),
        name: "Explorer Mk.I".to_string(),
        description: String::new(),
        hull_id: "corvette".to_string(),
        modules: Vec::new(),
        can_survey: true,
        can_colonize: false,
        maintenance: Amt::new(0, 500),
        build_cost_minerals: Amt::units(200),
        build_cost_energy: Amt::units(100),
        build_time: 60,
        hp: 50.0,
        sublight_speed: 0.75,
        ftl_range: 10.0,
    });
    registry.insert(ShipDesignDefinition {
        id: "colony_ship_mk1".to_string(),
        name: "Colony Ship Mk.I".to_string(),
        description: String::new(),
        hull_id: "frigate".to_string(),
        modules: Vec::new(),
        can_survey: false,
        can_colonize: true,
        maintenance: Amt::units(1),
        build_cost_minerals: Amt::units(500),
        build_cost_energy: Amt::units(300),
        build_time: 120,
        hp: 100.0,
        sublight_speed: 0.5,
        ftl_range: 15.0,
    });
    registry.insert(ShipDesignDefinition {
        id: "courier_mk1".to_string(),
        name: "Courier Mk.I".to_string(),
        description: String::new(),
        hull_id: "courier_hull".to_string(),
        modules: Vec::new(),
        can_survey: false,
        can_colonize: false,
        maintenance: Amt::new(0, 300),
        build_cost_minerals: Amt::units(100),
        build_cost_energy: Amt::units(50),
        build_time: 30,
        hp: 35.0,
        sublight_speed: 0.80,
        ftl_range: 0.0,
    });
    registry.insert(ShipDesignDefinition {
        id: "scout_mk1".to_string(),
        name: "Scout Mk.I".to_string(),
        description: String::new(),
        hull_id: "scout_hull".to_string(),
        modules: Vec::new(),
        can_survey: true,
        can_colonize: false,
        maintenance: Amt::new(0, 400),
        build_cost_minerals: Amt::units(150),
        build_cost_energy: Amt::units(80),
        build_time: 45,
        hp: 40.0,
        sublight_speed: 0.85,
        ftl_range: 10.0,
    });
    registry
}

/// Spawn a ship with all standard components at the given system.
pub fn spawn_test_ship(
    world: &mut World,
    name: &str,
    design_id: &str,
    system: Entity,
    pos: [f64; 3],
) -> Entity {
    let design_registry = create_test_design_registry();
    let design = design_registry.get(design_id).expect(&format!("unknown test design: {}", design_id));
    let hull_hp = design.hp;
    world
        .spawn((
            Ship {
                name: name.to_string(),
                design_id: design.id.clone(),
                hull_id: design.hull_id.clone(),
                modules: Vec::new(),
                owner: Owner::Neutral,
                sublight_speed: design.sublight_speed,
                ftl_range: design.ftl_range,
                player_aboard: false,
                home_port: system,
            },
            ShipState::Docked { system },
            Position::from(pos),
            ShipHitpoints {
                hull: hull_hp,
                hull_max: hull_hp,
                armor: 0.0,
                armor_max: 0.0,
                shield: 0.0,
                shield_max: 0.0,
                shield_regen: 0.0,
            },
            CommandQueue::default(),
            Cargo::default(),
            ShipModifiers::default(),
            RulesOfEngagement::default(),
        ))
        .id()
}
