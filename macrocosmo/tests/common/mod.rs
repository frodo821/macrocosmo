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
use macrocosmo::galaxy::{Anomalies, Planet, Sovereignty, StarSystem, SystemAttributes, SystemModifiers};
use macrocosmo::knowledge::*;
use macrocosmo::modifier::ModifiedValue;
use macrocosmo::condition::ScopedFlags;
use macrocosmo::player::{Empire, Faction, PlayerEmpire};
use macrocosmo::ship::*;
use macrocosmo::technology::{self, TechKnowledge};
use macrocosmo::time_system::{GameClock, GameSpeed};
use macrocosmo::visualization;

/// Create a BuildingRegistry populated with the standard 6 building definitions for tests.
///
/// #241: Uses the new `modifiers` field (target strings) to represent production
/// contributions. Buildings in tests are modelled as "automation" buildings —
/// they push directly into `colony.<resource>_per_hexadies` aggregators without
/// requiring pops to be assigned, so existing production-balance tests continue
/// to work. Real (Lua) buildings primarily grant job slots; see
/// `scripts/buildings/basic.lua`.
pub fn create_test_building_registry() -> macrocosmo::colony::BuildingRegistry {
    use macrocosmo::scripting::building_api::{BuildingDefinition, CapabilityParams};
    use macrocosmo::modifier::ParsedModifier;
    use std::collections::HashMap;
    let pm = |target: &str, base_add: f64| ParsedModifier {
        target: target.to_string(),
        base_add,
        multiplier: 0.0,
        add: 0.0,
    };
    let mut registry = macrocosmo::colony::BuildingRegistry::default();
    registry.insert(BuildingDefinition {
        id: "mine".into(), name: "Mine".into(), description: String::new(),
        minerals_cost: Amt::units(150), energy_cost: Amt::units(50), build_time: 10,
        maintenance: Amt::new(0, 200),
        production_bonus_minerals: Amt::ZERO, production_bonus_energy: Amt::ZERO,
        production_bonus_research: Amt::ZERO, production_bonus_food: Amt::ZERO,
        modifiers: vec![pm("colony.minerals_per_hexadies", 3.0)],
        is_system_building: false, capabilities: HashMap::new(),
        upgrade_to: Vec::new(), is_direct_buildable: true,
        prerequisites: None,
    });
    registry.insert(BuildingDefinition {
        id: "power_plant".into(), name: "PowerPlant".into(), description: String::new(),
        minerals_cost: Amt::units(50), energy_cost: Amt::units(150), build_time: 10,
        maintenance: Amt::ZERO,
        production_bonus_minerals: Amt::ZERO, production_bonus_energy: Amt::ZERO,
        production_bonus_research: Amt::ZERO, production_bonus_food: Amt::ZERO,
        modifiers: vec![pm("colony.energy_per_hexadies", 3.0)],
        is_system_building: false, capabilities: HashMap::new(),
        upgrade_to: Vec::new(), is_direct_buildable: true,
        prerequisites: None,
    });
    registry.insert(BuildingDefinition {
        id: "research_lab".into(), name: "ResearchLab".into(), description: String::new(),
        minerals_cost: Amt::units(100), energy_cost: Amt::units(100), build_time: 15,
        maintenance: Amt::new(0, 500),
        production_bonus_minerals: Amt::ZERO, production_bonus_energy: Amt::ZERO,
        production_bonus_research: Amt::ZERO, production_bonus_food: Amt::ZERO,
        modifiers: vec![pm("colony.research_per_hexadies", 2.0)],
        is_system_building: true, capabilities: HashMap::new(),
        upgrade_to: Vec::new(), is_direct_buildable: true,
        prerequisites: None,
    });
    let mut shipyard_caps = HashMap::new();
    shipyard_caps.insert("shipyard".to_string(), CapabilityParams {
        params: { let mut m = HashMap::new(); m.insert("concurrent_builds".to_string(), 1.0); m },
    });
    registry.insert(BuildingDefinition {
        id: "shipyard".into(), name: "Shipyard".into(), description: String::new(),
        minerals_cost: Amt::units(300), energy_cost: Amt::units(200), build_time: 30,
        maintenance: Amt::units(1),
        production_bonus_minerals: Amt::ZERO, production_bonus_energy: Amt::ZERO,
        production_bonus_research: Amt::ZERO, production_bonus_food: Amt::ZERO,
        modifiers: Vec::new(),
        is_system_building: true, capabilities: shipyard_caps,
        upgrade_to: Vec::new(), is_direct_buildable: true,
        prerequisites: None,
    });
    let mut port_caps = HashMap::new();
    port_caps.insert("port".to_string(), CapabilityParams {
        params: {
            let mut m = HashMap::new();
            m.insert("ftl_range_bonus".to_string(), 10.0);
            m.insert("travel_time_factor".to_string(), 0.8);
            m
        },
    });
    registry.insert(BuildingDefinition {
        id: "port".into(), name: "Port".into(), description: String::new(),
        minerals_cost: Amt::units(400), energy_cost: Amt::units(300), build_time: 40,
        maintenance: Amt::new(0, 500),
        production_bonus_minerals: Amt::ZERO, production_bonus_energy: Amt::ZERO,
        production_bonus_research: Amt::ZERO, production_bonus_food: Amt::ZERO,
        modifiers: Vec::new(),
        is_system_building: true, capabilities: port_caps,
        upgrade_to: Vec::new(), is_direct_buildable: true,
        prerequisites: None,
    });
    registry.insert(BuildingDefinition {
        id: "farm".into(), name: "Farm".into(), description: String::new(),
        minerals_cost: Amt::units(100), energy_cost: Amt::units(50), build_time: 20,
        maintenance: Amt::new(0, 300),
        production_bonus_minerals: Amt::ZERO, production_bonus_energy: Amt::ZERO,
        production_bonus_research: Amt::ZERO, production_bonus_food: Amt::ZERO,
        modifiers: vec![pm("colony.food_per_hexadies", 5.0)],
        is_system_building: false, capabilities: HashMap::new(),
        upgrade_to: Vec::new(), is_direct_buildable: true,
        prerequisites: None,
    });
    registry
}

/// Spawn a player empire entity with all empire-level components.
/// Returns the empire entity.
pub fn spawn_test_empire(world: &mut World) -> Entity {
    world
        .spawn((
            (
                Empire {
                    name: "Test Empire".into(),
                },
                PlayerEmpire,
                Faction {
                    id: "humanity_empire".into(),
                    name: "Test Empire".into(),
                },
                technology::TechTree::default(),
                technology::ResearchQueue::default(),
                technology::ResearchPool::default(),
                technology::RecentlyResearched::default(),
                AuthorityParams::default(),
                ConstructionParams::default(),
            ),
            (
                technology::EmpireModifiers::default(),
                technology::GameFlags::default(),
                technology::GlobalParams::default(),
                technology::PendingColonyTechModifiers::default(),
                KnowledgeStore::default(),
                CommandLog::default(),
                ScopedFlags::default(),
                macrocosmo::empire::CommsParams::default(),
            ),
        ))
        .id()
}

/// Test helper for #168: spawn passive hostile factions, seed Neutral/-100
/// relations against the test empire, and attach `FactionOwner` to every
/// existing `HostilePresence`. Tests that want combat to actually trigger
/// must call this after spawning their hostiles and ships.
///
/// Returns `(space_creature_faction, ancient_defense_faction)` entities.
pub fn setup_test_hostile_factions(world: &mut World) -> (Entity, Entity) {
    use macrocosmo::faction::{FactionOwner, FactionRelations, FactionView, HostileFactions, RelationState};
    use macrocosmo::galaxy::{HostilePresence, HostileType};

    // Find or create the player empire.
    let empire = {
        let mut q = world.query_filtered::<Entity, With<PlayerEmpire>>();
        q.iter(world).next()
    }
    .unwrap_or_else(|| spawn_test_empire(world));

    // Spawn faction entities. Idempotent: re-use if already present.
    let mut hf = world.resource::<HostileFactions>().clone();
    if hf.space_creature.is_none() {
        let e = world
            .spawn(Faction {
                id: "space_creature_faction".into(),
                name: "Space Creatures".into(),
            })
            .id();
        hf.space_creature = Some(e);
    }
    if hf.ancient_defense.is_none() {
        let e = world
            .spawn(Faction {
                id: "ancient_defense_faction".into(),
                name: "Ancient Defenses".into(),
            })
            .id();
        hf.ancient_defense = Some(e);
    }
    let space_creature = hf.space_creature.unwrap();
    let ancient_defense = hf.ancient_defense.unwrap();
    *world.resource_mut::<HostileFactions>() = hf;

    // Seed default hostile relations: Neutral + -100 standing both directions.
    {
        let mut rel = world.resource_mut::<FactionRelations>();
        rel.set(empire, space_creature, FactionView::new(RelationState::Neutral, -100.0));
        rel.set(space_creature, empire, FactionView::new(RelationState::Neutral, -100.0));
        rel.set(empire, ancient_defense, FactionView::new(RelationState::Neutral, -100.0));
        rel.set(ancient_defense, empire, FactionView::new(RelationState::Neutral, -100.0));
    }

    // Attach FactionOwner to every existing HostilePresence based on hostile_type.
    let assignments: Vec<(Entity, Entity)> = {
        let mut q = world.query::<(Entity, &HostilePresence)>();
        q.iter(world)
            .map(|(e, h)| {
                let owner = match h.hostile_type {
                    HostileType::SpaceCreature => space_creature,
                    HostileType::AncientDefense => ancient_defense,
                };
                (e, owner)
            })
            .collect()
    };
    for (entity, owner) in assignments {
        world.entity_mut(entity).insert(FactionOwner(owner));
    }

    // Re-home every Neutral ship onto the test empire so they participate in
    // combat under the new Faction-gated logic. Tests that explicitly want
    // unaffiliated ships should set `Owner::Neutral` *after* this call.
    let neutral_ships: Vec<Entity> = {
        let mut q = world.query::<(Entity, &macrocosmo::ship::Ship)>();
        q.iter(world)
            .filter(|(_, s)| matches!(s.owner, macrocosmo::ship::Owner::Neutral))
            .map(|(e, _)| e)
            .collect()
    };
    for e in neutral_ships {
        if let Some(mut ship) = world.get_mut::<macrocosmo::ship::Ship>(e) {
            ship.owner = macrocosmo::ship::Owner::Empire(empire);
        }
    }

    (space_creature, ancient_defense)
}

/// Build a headless Bevy App with game logic systems but no rendering.
pub fn test_app() -> App {
    let mut app = App::new();
    app.add_plugins(MinimalPlugins);
    app.insert_resource(GameClock::new(0));
    app.insert_resource(GameSpeed::default());
    // GameClock is inserted above so AiPlugin's Startup schema::declare_all
    // (which does not yet use GameClock, but AiBusWriter SystemParams rely
    // on it at runtime) always observes the test clock.
    app.add_plugins(macrocosmo::ai::AiPlugin);
    app.insert_resource(LastProductionTick(0));
    app.insert_resource(EventLog::default());
    app.insert_resource(EventSystem::default());
    app.insert_resource(EventBus::default());
    app.insert_resource(technology::LastResearchTick(0));
    app.init_resource::<species::SpeciesRegistry>();
    app.init_resource::<species::JobRegistry>();
    app.init_resource::<AlertCooldowns>();
    app.insert_resource(create_test_building_registry());
    app.insert_resource(create_test_module_registry());
    app.insert_resource(create_test_hull_registry());
    app.insert_resource(create_test_design_registry());
    app.init_resource::<macrocosmo::faction::FactionRelations>();
    app.init_resource::<macrocosmo::faction::HostileFactions>();
    // #160: Scriptable balance constants resource (defaults mirror hardcoded
    // values so tests exercise the same baseline behaviour).
    app.init_resource::<technology::GameBalance>();
    app.add_message::<GameEvent>();
    // #233: Notification pipeline resources consumed by detect_hostiles_system
    // and friends. Instantiated without the full NotificationsPlugin because
    // the plugin registers egui-coupled systems that tests don't want.
    app.init_resource::<macrocosmo::knowledge::PendingFactQueue>();
    app.init_resource::<macrocosmo::knowledge::RelayNetwork>();
    app.insert_resource(macrocosmo::notifications::NotificationQueue::new());
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
            process_refitting,
            process_pending_ship_commands,
            tick_courier_routes,
            // #223: Deliverable ops run before process_command_queue so any
            // injected MoveTo reaches the router in the same frame.
            macrocosmo::ship::deliverable_ops::process_deliverable_commands,
            process_command_queue,
            // #217: Scout observation + report. Chained after
            // process_command_queue so a Scout that began transitioning to
            // Scouting this tick doesn't get double-processed.
            macrocosmo::ship::scout::tick_scout_observation,
            macrocosmo::ship::scout::process_scout_report,
            resolve_combat,
            tick_ship_repair,
            macrocosmo::ship::pursuit::detect_hostiles_system,
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
            species::sync_job_assignment,
            sync_species_modifiers,
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
        apply_pending_colonization_orders.after(macrocosmo::time_system::advance_game_time),
    );
    app.add_systems(
        Update,
        macrocosmo::event_system::tick_events
            .after(macrocosmo::time_system::advance_game_time)
            .after(tick_timed_effects),
    );
    app.add_systems(Update, propagate_knowledge);
    app.add_systems(Update, macrocosmo::knowledge::snapshot_production_knowledge);
    // #118: Sensor Buoy detection
    app.init_resource::<macrocosmo::deep_space::StructureRegistry>();
    app.add_systems(
        Update,
        (
            macrocosmo::deep_space::sensor_buoy_detect_system,
            macrocosmo::deep_space::verify_relay_pairings_system,
            macrocosmo::deep_space::relay_knowledge_propagate_system,
            macrocosmo::deep_space::tick_platform_upgrade,
            macrocosmo::deep_space::tick_scrapyard_despawn,
        )
            .after(macrocosmo::time_system::advance_game_time)
            .after(sublight_movement_system)
            .after(process_ftl_travel),
    );
    // #59: Player location tracking (after ship movement systems)
    app.add_systems(
        Update,
        macrocosmo::player::update_player_location
            .after(macrocosmo::time_system::advance_game_time)
            .after(sublight_movement_system)
            .after(process_ftl_travel),
    );

    // #171: Light-speed delayed diplomatic actions (drains arrived
    // PendingDiplomaticAction entities into FactionRelations).
    app.add_systems(
        Update,
        macrocosmo::faction::tick_diplomatic_actions
            .after(macrocosmo::time_system::advance_game_time),
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
            .after(macrocosmo::time_system::advance_game_time)
            .after(macrocosmo::ship::pursuit::detect_hostiles_system),
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
    // AI integration plugin (#203) — AiBusResource + ordered AiTickSet sets.
    // Added here after GameClock is inserted so AiBusWriter SystemParams can
    // read it, and so `full_test_app` can detect Query conflicts (B0001)
    // introduced by AI systems at CI time.
    app.add_plugins(macrocosmo::ai::AiPlugin);
    app.insert_resource(LastProductionTick(0));
    app.insert_resource(EventLog::default());
    app.insert_resource(EventSystem::default());
    app.init_resource::<species::SpeciesRegistry>();
    app.init_resource::<species::JobRegistry>();
    app.init_resource::<AlertCooldowns>();
    app.insert_resource(create_test_building_registry());
    app.insert_resource(create_test_module_registry());
    app.insert_resource(create_test_hull_registry());
    app.insert_resource(create_test_design_registry());
    app.init_resource::<macrocosmo::faction::FactionRelations>();
    app.init_resource::<macrocosmo::faction::HostileFactions>();
    // #160: Scriptable balance constants resource.
    app.init_resource::<technology::GameBalance>();
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

    // --- #233 Notification pipeline resources ---
    app.init_resource::<macrocosmo::knowledge::PendingFactQueue>();
    app.init_resource::<macrocosmo::knowledge::RelayNetwork>();
    app.insert_resource(macrocosmo::notifications::NotificationQueue::new());

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
            process_refitting,
            process_pending_ship_commands,
            tick_courier_routes,
            macrocosmo::ship::deliverable_ops::process_deliverable_commands,
            process_command_queue,
            // #217: Scout observation + delivery.
            macrocosmo::ship::scout::tick_scout_observation,
            macrocosmo::ship::scout::process_scout_report,
            resolve_combat,
            tick_ship_repair,
            macrocosmo::ship::pursuit::detect_hostiles_system,
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
            species::sync_job_assignment,
            sync_species_modifiers,
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

    // --- Knowledge system (from KnowledgePlugin) ---
    app.add_systems(Update, propagate_knowledge);
    app.add_systems(Update, macrocosmo::knowledge::snapshot_production_knowledge);

    // --- Deep space (from DeepSpacePlugin) ---
    app.init_resource::<macrocosmo::deep_space::StructureRegistry>();
    app.add_systems(
        Update,
        (
            macrocosmo::deep_space::sensor_buoy_detect_system,
            macrocosmo::deep_space::verify_relay_pairings_system,
            macrocosmo::deep_space::relay_knowledge_propagate_system,
            macrocosmo::deep_space::tick_platform_upgrade,
            macrocosmo::deep_space::tick_scrapyard_despawn,
        ),
    );

    // --- Communication systems (from CommunicationPlugin) ---
    app.add_systems(
        Update,
        (
            communication::process_messages,
            communication::process_courier_ships,
            communication::process_pending_commands,
        ),
    );

    // --- Technology resources ---
    app.init_resource::<technology::TechEffectsLog>();

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
    // apply_tech_effects requires ScriptEngine which is not available in headless tests;
    // it will early-return. Registered here for query-conflict detection.
    app.add_systems(
        Update,
        technology::apply_tech_effects
            .after(technology::tick_research),
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

    // --- Faction systems (#171) ---
    app.add_systems(Update, macrocosmo::faction::tick_diplomatic_actions);

    // Spawn the empire entity
    spawn_test_empire(app.world_mut());

    app
}

/// Advance the game clock by `hexadies` and run one update cycle.
///
/// **#168 — Auto faction migration.** Before running the update, ensure that
/// any `HostilePresence` in the world is paired with a `FactionOwner` and that
/// the test empire/hostile factions have default Neutral/-100 relations. This
/// preserves the pre-#168 behavior of legacy combat tests without forcing
/// every test to explicitly call `setup_test_hostile_factions`. Tests that
/// want to verify the un-migrated behavior should run their own `app.update()`
/// directly instead of using `advance_time`.
pub fn advance_time(app: &mut App, hexadies: i64) {
    let needs_migration = {
        let mut q = app
            .world_mut()
            .query_filtered::<Entity, (
                With<macrocosmo::galaxy::HostilePresence>,
                Without<macrocosmo::faction::FactionOwner>,
            )>();
        q.iter(app.world()).next().is_some()
    };
    if needs_migration {
        setup_test_hostile_factions(app.world_mut());
    }

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
    hab: f64,
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
    hab: f64,
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
                mineral_richness: 0.5,
                energy_potential: 0.5,
                research_potential: 0.5,
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

/// #236: Test fixture builders for hull + module registries that mirror the
/// Lua preset content. Designs are built from these via `design_derived` so
/// the test registry always reflects the canonical derivation formula.
pub fn create_test_hull_registry() -> macrocosmo::ship_design::HullRegistry {
    use macrocosmo::ship_design::{HullDefinition, HullRegistry, HullSlot, ModuleModifier};
    let mut hulls = HullRegistry::default();
    let slot = |t: &str, c: u32| HullSlot { slot_type: t.to_string(), count: c };
    hulls.insert(HullDefinition {
        id: "corvette".into(), name: "Corvette".into(), description: String::new(),
        base_hp: 50.0, base_speed: 0.75, base_evasion: 30.0,
        slots: vec![slot("ftl", 1), slot("sublight", 1), slot("weapon", 2), slot("defense", 1), slot("utility", 1), slot("power", 1)],
        build_cost_minerals: Amt::units(200), build_cost_energy: Amt::units(100),
        build_time: 60, maintenance: Amt::new(0, 500),
        modifiers: vec![], prerequisites: None,
    });
    hulls.insert(HullDefinition {
        id: "frigate".into(), name: "Frigate".into(), description: String::new(),
        base_hp: 120.0, base_speed: 0.5, base_evasion: 15.0,
        slots: vec![slot("ftl", 1), slot("sublight", 1), slot("weapon", 3), slot("defense", 2), slot("utility", 2), slot("power", 1), slot("command", 1)],
        build_cost_minerals: Amt::units(400), build_cost_energy: Amt::units(200),
        build_time: 120, maintenance: Amt::units(1),
        modifiers: vec![], prerequisites: None,
    });
    hulls.insert(HullDefinition {
        id: "scout_hull".into(), name: "Scout Hull".into(), description: String::new(),
        base_hp: 40.0, base_speed: 0.85, base_evasion: 35.0,
        slots: vec![slot("ftl", 1), slot("sublight", 1), slot("utility", 2), slot("weapon", 1), slot("power", 1)],
        build_cost_minerals: Amt::units(150), build_cost_energy: Amt::units(80),
        build_time: 45, maintenance: Amt::new(0, 400),
        modifiers: vec![
            ModuleModifier { target: "ship.survey_speed".into(), base_add: 0.0, multiplier: 1.3, add: 0.0 },
            ModuleModifier { target: "ship.speed".into(), base_add: 0.0, multiplier: 1.15, add: 0.0 },
        ],
        prerequisites: None,
    });
    hulls.insert(HullDefinition {
        id: "courier_hull".into(), name: "Courier Hull".into(), description: String::new(),
        base_hp: 35.0, base_speed: 0.80, base_evasion: 25.0,
        slots: vec![slot("ftl", 1), slot("sublight", 1), slot("utility", 2), slot("power", 1)],
        build_cost_minerals: Amt::units(100), build_cost_energy: Amt::units(50),
        build_time: 30, maintenance: Amt::new(0, 300),
        modifiers: vec![
            ModuleModifier { target: "ship.cargo_capacity".into(), base_add: 0.0, multiplier: 1.5, add: 0.0 },
            ModuleModifier { target: "ship.ftl_range".into(), base_add: 0.0, multiplier: 1.2, add: 0.0 },
        ],
        prerequisites: None,
    });
    hulls
}

pub fn create_test_module_registry() -> macrocosmo::ship_design::ModuleRegistry {
    use macrocosmo::ship_design::{ModuleDefinition, ModuleRegistry, ModuleModifier};
    let mut modules = ModuleRegistry::default();
    modules.insert(ModuleDefinition {
        id: "ftl_drive".into(), name: "FTL Drive".into(), description: String::new(),
        slot_type: "ftl".into(),
        modifiers: vec![ModuleModifier { target: "ship.ftl_range".into(), base_add: 15.0, multiplier: 0.0, add: 0.0 }],
        weapon: None, cost_minerals: Amt::units(100), cost_energy: Amt::units(50),
        prerequisites: None, upgrade_to: Vec::new(),
    });
    modules.insert(ModuleDefinition {
        id: "afterburner".into(), name: "Afterburner".into(), description: String::new(),
        slot_type: "sublight".into(),
        modifiers: vec![ModuleModifier { target: "ship.speed".into(), base_add: 0.0, multiplier: 0.2, add: 0.0 }],
        weapon: None, cost_minerals: Amt::units(60), cost_energy: Amt::units(40),
        prerequisites: None, upgrade_to: Vec::new(),
    });
    modules.insert(ModuleDefinition {
        id: "survey_equipment".into(), name: "Survey Equipment".into(), description: String::new(),
        slot_type: "utility".into(),
        modifiers: vec![ModuleModifier { target: "ship.survey_speed".into(), base_add: 1.0, multiplier: 0.0, add: 0.0 }],
        weapon: None, cost_minerals: Amt::units(60), cost_energy: Amt::units(40),
        prerequisites: None, upgrade_to: Vec::new(),
    });
    modules.insert(ModuleDefinition {
        id: "colony_module".into(), name: "Colony Module".into(), description: String::new(),
        slot_type: "utility".into(),
        modifiers: vec![ModuleModifier { target: "ship.colonize_speed".into(), base_add: 1.0, multiplier: 0.0, add: 0.0 }],
        weapon: None, cost_minerals: Amt::units(300), cost_energy: Amt::units(200),
        prerequisites: None, upgrade_to: Vec::new(),
    });
    modules.insert(ModuleDefinition {
        id: "cargo_bay".into(), name: "Cargo Bay".into(), description: String::new(),
        slot_type: "utility".into(),
        modifiers: vec![ModuleModifier { target: "ship.cargo_capacity".into(), base_add: 500.0, multiplier: 0.0, add: 0.0 }],
        weapon: None, cost_minerals: Amt::units(30), cost_energy: Amt::ZERO,
        prerequisites: None, upgrade_to: Vec::new(),
    });
    modules
}

/// Build a ShipDesignDefinition from hull + module IDs, with derived stats
/// computed via `design_derived`. Used by the test fixture.
fn build_derived_design(
    id: &str,
    name: &str,
    hull_id: &str,
    module_assignments: &[(&str, &str)],
    hulls: &macrocosmo::ship_design::HullRegistry,
    modules: &macrocosmo::ship_design::ModuleRegistry,
) -> macrocosmo::ship_design::ShipDesignDefinition {
    use macrocosmo::ship_design::{DesignSlotAssignment, ShipDesignDefinition};
    let assignments: Vec<DesignSlotAssignment> = module_assignments
        .iter()
        .map(|(s, m)| DesignSlotAssignment { slot_type: s.to_string(), module_id: m.to_string() })
        .collect();
    let mut def = ShipDesignDefinition {
        id: id.into(),
        name: name.into(),
        description: String::new(),
        hull_id: hull_id.into(),
        modules: assignments,
        can_survey: false, can_colonize: false,
        maintenance: Amt::ZERO,
        build_cost_minerals: Amt::ZERO, build_cost_energy: Amt::ZERO,
        build_time: 0, hp: 0.0, sublight_speed: 0.0, ftl_range: 0.0,
        revision: 0,
    };
    macrocosmo::ship_design::apply_derived_to_definition(&mut def, hulls, modules);
    def
}

/// Create a ShipDesignRegistry populated with the standard ship designs for
/// tests. #236: All stats are derived from `create_test_hull_registry` +
/// `create_test_module_registry` via `design_derived` — never hand-authored.
pub fn create_test_design_registry() -> macrocosmo::ship_design::ShipDesignRegistry {
    use macrocosmo::ship_design::ShipDesignRegistry;
    let hulls = create_test_hull_registry();
    let modules = create_test_module_registry();
    let mut registry = ShipDesignRegistry::default();

    registry.insert(build_derived_design(
        "explorer_mk1", "Explorer Mk.I", "corvette",
        &[("ftl", "ftl_drive"), ("utility", "survey_equipment")],
        &hulls, &modules,
    ));
    registry.insert(build_derived_design(
        "colony_ship_mk1", "Colony Ship Mk.I", "frigate",
        &[("ftl", "ftl_drive"), ("utility", "colony_module")],
        &hulls, &modules,
    ));
    registry.insert(build_derived_design(
        "courier_mk1", "Courier Mk.I", "courier_hull",
        &[("ftl", "ftl_drive"), ("sublight", "afterburner"), ("utility", "cargo_bay")],
        &hulls, &modules,
    ));
    registry.insert(build_derived_design(
        "scout_mk1", "Scout Mk.I", "scout_hull",
        &[("ftl", "ftl_drive"), ("utility", "survey_equipment")],
        &hulls, &modules,
    ));
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
                design_revision: 0,
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
            macrocosmo::ship::ShipStats::default(),
            RulesOfEngagement::default(),
        ))
        .id()
}
