//! #281: Integration tests for the `macrocosmo:building_built` event and the
//! `on_built` / `on_upgraded` definition-level hooks.
//!
//! The tests come in two flavours:
//!
//! 1. **Event-firing tests** drive an actual `test_app()` through the
//!    building / system-building / platform-upgrade tick systems and assert
//!    that `EventSystem.fired_log` now contains a `macrocosmo:building_built`
//!    entry with the expected payload.
//! 2. **Dispatch tests** exercise the Lua-side auto-subscription machinery
//!    (`register_building_built_hooks`) by building a minimal `World` with
//!    `ScriptEngine + EventSystem + BuildingRegistry`, running the hook
//!    registration, pushing a synthetic `FiredEvent`, and calling
//!    `dispatch_event_handlers` — so the test verifies the same code path a
//!    production game uses without having to stand up a shipyard / resources
//!    pipeline for every permutation.

mod common;

use bevy::prelude::*;
use common::{
    advance_time, find_planet, spawn_test_colony, spawn_test_system, spawn_test_system_with_planet,
    test_app,
};
use macrocosmo::amount::Amt;
use macrocosmo::colony::{
    BuildingId, BuildingOrder, BuildingQueue, Buildings, SystemBuildingQueue, SystemBuildings,
    UpgradeOrder,
};
use macrocosmo::event_system::{
    BUILDING_BUILT_EVENT, EventSystem, FiredEvent, LuaDefinedEventContext, LuaFunctionRef,
};
use macrocosmo::scripting::building_api::{BuildingDefinition, BuildingRegistry, CapabilityParams};
use std::collections::HashMap;

// ============================================================================
// Helpers
// ============================================================================

/// Insert a ready-to-complete construction order on the planet-level
/// `BuildingQueue`. All costs are pre-paid and `build_time_remaining = 0` so a
/// single tick advances the order through the completion branch.
fn seed_planet_construction(app: &mut App, colony: Entity, building_id: &str, slot: usize) {
    let mut bq = app.world_mut().get_mut::<BuildingQueue>(colony).unwrap();
    bq.push_build_order(BuildingOrder {
        order_id: 0,
        building_id: BuildingId::new(building_id),
        target_slot: slot,
        minerals_remaining: Amt::ZERO,
        energy_remaining: Amt::ZERO,
        build_time_remaining: 0,
    });
}

/// Same as `seed_planet_construction` but on the StarSystem-level
/// `SystemBuildingQueue`.
fn seed_system_construction(app: &mut App, system: Entity, building_id: &str, slot: usize) {
    let mut sbq = app
        .world_mut()
        .get_mut::<SystemBuildingQueue>(system)
        .unwrap();
    sbq.push_build_order(BuildingOrder {
        order_id: 0,
        building_id: BuildingId::new(building_id),
        target_slot: slot,
        minerals_remaining: Amt::ZERO,
        energy_remaining: Amt::ZERO,
        build_time_remaining: 0,
    });
}

/// Pre-populate a slot on a colony's `Buildings` component so an upgrade order
/// has a source to replace. Expands the slots vec if needed.
fn place_planet_building(app: &mut App, colony: Entity, slot: usize, building_id: &str) {
    let mut bldgs = app.world_mut().get_mut::<Buildings>(colony).unwrap();
    while bldgs.slots.len() <= slot {
        bldgs.slots.push(None);
    }
    bldgs.slots[slot] = Some(BuildingId::new(building_id));
}

fn seed_planet_upgrade(app: &mut App, colony: Entity, slot: usize, target_id: &str) {
    let mut bq = app.world_mut().get_mut::<BuildingQueue>(colony).unwrap();
    bq.push_upgrade_order(UpgradeOrder {
        order_id: 0,
        slot_index: slot,
        target_id: BuildingId::new(target_id),
        minerals_remaining: Amt::ZERO,
        energy_remaining: Amt::ZERO,
        build_time_remaining: 0,
    });
}

fn find_building_built(
    event_system: &EventSystem,
    building_id: &str,
    cause: &str,
) -> Option<FiredEvent> {
    event_system
        .fired_log
        .iter()
        .find(|fe| {
            fe.event_id == BUILDING_BUILT_EVENT
                && fe
                    .payload
                    .as_ref()
                    .and_then(|p| p.payload_get("building_id"))
                    .as_deref()
                    == Some(building_id)
                && fe
                    .payload
                    .as_ref()
                    .and_then(|p| p.payload_get("cause"))
                    .as_deref()
                    == Some(cause)
        })
        .cloned()
}

/// Look up a string-shaped payload value on a fired event's EventContext.
/// Replaces the pre-#288 `evt.payload.as_ref().unwrap().get(key).map(String::as_str)`
/// idiom used throughout the payload-carrying assertions below.
fn payload_str<'a>(evt: &'a FiredEvent, key: &str) -> Option<std::borrow::Cow<'a, str>> {
    evt.payload.as_ref().and_then(|p| p.payload_get(key))
}

// ============================================================================
// Event firing — required test #1 / #2 / #4
// ============================================================================

#[test]
fn test_building_built_event_fired_on_planet_construction_complete() {
    let mut app = test_app();
    let sys = spawn_test_system(app.world_mut(), "Alpha", [0.0, 0.0, 0.0], 1.0, true, true);
    let colony = spawn_test_colony(
        app.world_mut(),
        sys,
        Amt::units(100),
        Amt::units(100),
        vec![None, None],
    );
    seed_planet_construction(&mut app, colony, "mine", 0);

    advance_time(&mut app, 1);

    let es = app.world().resource::<EventSystem>();
    let evt = find_building_built(es, "mine", "construction")
        .expect("planet construction must fire macrocosmo:building_built (cause=construction)");
    // Slot 0 completion writes the building into the slot.
    let bldgs = app.world().get::<Buildings>(colony).unwrap();
    assert_eq!(bldgs.slots[0].as_ref().unwrap().as_str(), "mine");
    assert_eq!(payload_str(&evt, "slot").as_deref(), Some("0"));
}

#[test]
fn test_building_built_event_fired_on_system_building_complete() {
    let mut app = test_app();
    let sys = spawn_test_system(app.world_mut(), "Beta", [0.0, 0.0, 0.0], 1.0, true, true);
    let _colony = spawn_test_colony(
        app.world_mut(),
        sys,
        Amt::units(100),
        Amt::units(100),
        vec![None],
    );
    seed_system_construction(&mut app, sys, "shipyard", 0);

    advance_time(&mut app, 1);

    let es = app.world().resource::<EventSystem>();
    let evt = find_building_built(es, "shipyard", "construction")
        .expect("system construction must fire macrocosmo:building_built");
    assert_eq!(payload_str(&evt, "slot").as_deref(), Some("0"));
    // System buildings have no `colony` key — the building is attached to the
    // StarSystem entity itself.
    assert!(payload_str(&evt, "colony").is_none());
    // Verify the system building was completed (station ship with SlotAssignment spawned).
    let sys_bldgs = app.world().get::<SystemBuildings>(sys).unwrap();
    assert!(sys_bldgs.max_slots > 0);
}

#[test]
fn test_building_built_event_fired_on_structure_complete() {
    // Build a tiny world that only exercises the deep-space platform upgrade
    // path. We don't need a full colony / shipyard chain — just a
    // ConstructionPlatform entity with sufficient accumulated resources and a
    // StructureRegistry that describes the platform + target.
    use macrocosmo::components::Position;
    use macrocosmo::deep_space::{
        CapabilityParams as DsCapabilityParams, ConstructionPlatform, DeepSpaceStructure,
        DeliverableMetadata, LifetimeCost, ResourceCost, StructureDefinition, StructureRegistry,
        UpgradeEdge, tick_platform_upgrade,
    };
    use macrocosmo::events::GameEvent;
    use macrocosmo::ship::Owner;
    use macrocosmo::time_system::GameClock;

    let mut app = App::new();
    app.add_plugins(MinimalPlugins);
    app.insert_resource(GameClock::new(0));
    app.insert_resource(EventSystem::default());
    app.add_message::<GameEvent>();
    app.init_resource::<macrocosmo::knowledge::PendingFactQueue>();
    app.init_resource::<macrocosmo::knowledge::RelayNetwork>();
    app.init_resource::<macrocosmo::knowledge::NextEventId>();
    app.init_resource::<macrocosmo::knowledge::NotifiedEventIds>();
    app.insert_resource(macrocosmo::notifications::NotificationQueue::new());

    let mut registry = StructureRegistry::default();
    registry.insert(StructureDefinition {
        id: "kit".into(),
        name: "Kit".into(),
        description: String::new(),
        max_hp: 10.0,
        energy_drain: Amt::ZERO,
        capabilities: HashMap::from([(
            "construction_platform".to_string(),
            DsCapabilityParams::default(),
        )]),
        prerequisites: None,
        deliverable: Some(DeliverableMetadata {
            cost: ResourceCost::default(),
            build_time: 1,
            cargo_size: 1,
            scrap_refund: 0.0,
            spawns_as_ship: None,
        }),
        upgrade_to: vec![UpgradeEdge {
            target_id: "outpost".into(),
            cost: ResourceCost::default(),
            build_time: 1,
        }],
        upgrade_from: None,
        on_built: None,
        on_upgraded: None,
    });
    registry.insert(StructureDefinition {
        id: "outpost".into(),
        name: "Outpost".into(),
        description: String::new(),
        max_hp: 100.0,
        energy_drain: Amt::ZERO,
        capabilities: HashMap::new(),
        prerequisites: None,
        deliverable: None,
        upgrade_to: Vec::new(),
        upgrade_from: None,
        on_built: None,
        on_upgraded: None,
    });
    registry.rebuild_effective_edges();
    app.insert_resource(registry);

    let platform_entity = app
        .world_mut()
        .spawn((
            DeepSpaceStructure {
                definition_id: "kit".into(),
                name: "Kit".into(),
                owner: Owner::Neutral,
            },
            Position::from([0.0, 0.0, 0.0]),
            LifetimeCost(ResourceCost::default()),
            macrocosmo::deep_space::StructureHitpoints {
                current: 10.0,
                max: 10.0,
            },
            ConstructionPlatform {
                target_id: Some("outpost".into()),
                accumulated: ResourceCost::default(),
            },
        ))
        .id();

    app.add_systems(Update, tick_platform_upgrade);
    app.update();

    let es = app.world().resource::<EventSystem>();
    let evt = find_building_built(es, "outpost", "construction")
        .expect("deep-space platform completion must fire macrocosmo:building_built");
    assert_eq!(
        payload_str(&evt, "previous_id").as_deref(),
        Some("kit"),
        "previous_id must carry the platform/kit definition id"
    );
    // Sanity check: the structure identity was flipped.
    let structure = app
        .world()
        .get::<DeepSpaceStructure>(platform_entity)
        .unwrap();
    assert_eq!(structure.definition_id, "outpost");
}

#[test]
fn test_building_built_event_carries_correct_payload() {
    let mut app = test_app();
    let (sys, planet) =
        spawn_test_system_with_planet(app.world_mut(), "Gamma", [0.0, 0.0, 0.0], 1.0, true);
    let colony = spawn_test_colony(
        app.world_mut(),
        planet,
        Amt::units(100),
        Amt::units(100),
        vec![None, None, None],
    );
    seed_planet_construction(&mut app, colony, "power_plant", 2);

    advance_time(&mut app, 1);

    let es = app.world().resource::<EventSystem>();
    let evt = find_building_built(es, "power_plant", "construction").unwrap();
    assert_eq!(payload_str(&evt, "cause").as_deref(), Some("construction"));
    assert_eq!(
        payload_str(&evt, "building_id").as_deref(),
        Some("power_plant")
    );
    assert_eq!(payload_str(&evt, "slot").as_deref(), Some("2"));
    // System + colony entities are serialised as `Entity::to_bits` decimal
    // strings — round-trip them to confirm they identify the right entities.
    let system_bits: u64 = payload_str(&evt, "system").unwrap().parse().unwrap();
    let colony_bits: u64 = payload_str(&evt, "colony").unwrap().parse().unwrap();
    assert_eq!(Entity::from_bits(system_bits), sys);
    assert_eq!(Entity::from_bits(colony_bits), colony);
    // `previous_id` is absent on construction.
    assert!(payload_str(&evt, "previous_id").is_none());
}

#[test]
fn test_on_upgraded_carries_previous_id() {
    let mut app = test_app();
    let sys = spawn_test_system(app.world_mut(), "Delta", [0.0, 0.0, 0.0], 1.0, true, true);
    // Put a mine already in slot 0 so the upgrade order has something to
    // replace.
    let colony = spawn_test_colony(
        app.world_mut(),
        sys,
        Amt::units(100),
        Amt::units(100),
        vec![None, None],
    );
    place_planet_building(&mut app, colony, 0, "mine");
    seed_planet_upgrade(&mut app, colony, 0, "advanced_mine");

    advance_time(&mut app, 1);

    let es = app.world().resource::<EventSystem>();
    let evt = find_building_built(es, "advanced_mine", "upgrade")
        .expect("upgrade completion must fire cause=upgrade");
    assert_eq!(payload_str(&evt, "previous_id").as_deref(), Some("mine"));
    // Planet Buildings slot now holds the upgraded id.
    let bldgs = app.world().get::<Buildings>(colony).unwrap();
    assert_eq!(bldgs.slots[0].as_ref().unwrap().as_str(), "advanced_mine");
    // Regression: the legacy `building_upgraded` event is still queued
    // alongside. (It's queued into `pending`, so it may drain next tick
    // rather than immediately — advance one more hexadies to flush it.)
    advance_time(&mut app, 1);
    let es = app.world().resource::<EventSystem>();
    assert!(
        es.fired_log
            .iter()
            .any(|fe| fe.event_id == "building_upgraded"),
        "legacy building_upgraded event must still fire"
    );
    // Planet sanity.
    let _ = find_planet(app.world_mut(), sys);
}

// ============================================================================
// Hook dispatch — required test #5 / #6 / #7 / #9
// ============================================================================
//
// These mirror the pattern used by `scripting::lifecycle` dispatch tests:
// build a minimal `World`, register hooks via the same production path as
// startup (`register_building_built_hooks`), push a synthetic `FiredEvent`,
// and drive `dispatch_event_handlers` directly. This isolates the hook
// dispatch logic from the ECS-system plumbing that the event-firing tests
// above already cover.

use macrocosmo::condition::ScopedFlags;
use macrocosmo::deep_space::StructureRegistry;
use macrocosmo::player::{Empire, PlayerEmpire};
use macrocosmo::scripting::lifecycle::dispatch_event_handlers;
use macrocosmo::scripting::{ScriptEngine, register_building_built_hooks};
use macrocosmo::technology::{GameFlags, TechTree};
use macrocosmo::time_system::GameClock;

fn hook_test_world() -> World {
    let mut world = World::new();
    world.insert_resource(GameClock::new(1));
    world.insert_resource(EventSystem::default());
    world.insert_resource(ScriptEngine::new().unwrap());
    world.insert_resource(BuildingRegistry::default());
    world.insert_resource(StructureRegistry::default());
    world.spawn((
        Empire { name: "E".into() },
        PlayerEmpire,
        TechTree::default(),
        GameFlags::default(),
        ScopedFlags::default(),
    ));
    world
}

fn bare_building_def(id: &str) -> BuildingDefinition {
    BuildingDefinition {
        id: id.to_string(),
        name: id.to_string(),
        description: String::new(),
        minerals_cost: Amt::ZERO,
        energy_cost: Amt::ZERO,
        build_time: 1,
        maintenance: Amt::ZERO,
        production_bonus_minerals: Amt::ZERO,
        production_bonus_energy: Amt::ZERO,
        production_bonus_research: Amt::ZERO,
        production_bonus_food: Amt::ZERO,
        modifiers: Vec::new(),
        is_system_building: false,
        capabilities: HashMap::<String, CapabilityParams>::new(),
        upgrade_to: Vec::new(),
        is_direct_buildable: true,
        prerequisites: None,
        on_built: None,
        on_upgraded: None,
        dismantlable: true,
        ship_design_id: None,
    }
}

/// Compile a Lua function snippet and wrap it as a `LuaFunctionRef` so it
/// can be stored on a `BuildingDefinition::on_built` / `on_upgraded` field.
/// The `lua_src` MUST be a `return function(evt) ... end` expression.
fn compile_hook(world: &mut World, lua_src: &str) -> LuaFunctionRef {
    let engine = world.resource::<ScriptEngine>();
    let lua = engine.lua();
    let func: mlua::Function = lua.load(lua_src).eval().unwrap();
    LuaFunctionRef::from_function(lua, func).unwrap()
}

fn push_build_event(world: &mut World, building_id: &str, cause: &str, previous_id: Option<&str>) {
    let mut payload = HashMap::new();
    payload.insert("cause".to_string(), cause.to_string());
    payload.insert("building_id".to_string(), building_id.to_string());
    if let Some(p) = previous_id {
        payload.insert("previous_id".to_string(), p.to_string());
    }
    let ctx = LuaDefinedEventContext::new(BUILDING_BUILT_EVENT, payload);
    let mut es = world.resource_mut::<EventSystem>();
    es.fired_log.push(FiredEvent {
        event_id: BUILDING_BUILT_EVENT.to_string(),
        target: None,
        fired_at: 1,
        payload: Some(std::sync::Arc::new(ctx)),
    });
}

#[test]
fn test_on_built_hook_invoked_for_matching_building() {
    let mut world = hook_test_world();
    {
        let engine = world.resource::<ScriptEngine>();
        engine
            .lua()
            .load(r#"_shipyard_called = false; _shipyard_cause = nil"#)
            .exec()
            .unwrap();
    }
    let hook = compile_hook(
        &mut world,
        r#"return function(evt)
              _shipyard_called = true
              _shipyard_cause = evt.cause
              _shipyard_id = evt.building_id
          end"#,
    );
    {
        let mut reg = world.resource_mut::<BuildingRegistry>();
        let mut def = bare_building_def("shipyard");
        def.on_built = Some(hook);
        reg.insert(def);
    }

    // Register hooks exactly the way startup does.
    {
        let mut sys_state: bevy::ecs::system::SystemState<(
            Res<ScriptEngine>,
            Res<BuildingRegistry>,
            Res<StructureRegistry>,
        )> = bevy::ecs::system::SystemState::new(&mut world);
        let (engine, br, sr) = sys_state.get(&world);
        register_building_built_hooks(engine, br, sr);
    }

    push_build_event(&mut world, "shipyard", "construction", None);
    dispatch_event_handlers(&mut world);

    let engine = world.resource::<ScriptEngine>();
    let called: bool = engine.lua().globals().get("_shipyard_called").unwrap();
    assert!(called, "on_built hook must fire for its own building");
    let cause: String = engine.lua().globals().get("_shipyard_cause").unwrap();
    assert_eq!(cause, "construction");
    let id: String = engine.lua().globals().get("_shipyard_id").unwrap();
    assert_eq!(id, "shipyard");
}

#[test]
fn test_on_built_hook_not_invoked_for_other_buildings() {
    let mut world = hook_test_world();
    {
        let engine = world.resource::<ScriptEngine>();
        engine
            .lua()
            .load(r#"_mine_called = false; _shipyard_called = false"#)
            .exec()
            .unwrap();
    }
    let mine_hook = compile_hook(
        &mut world,
        r#"return function(evt) _mine_called = true end"#,
    );
    let shipyard_hook = compile_hook(
        &mut world,
        r#"return function(evt) _shipyard_called = true end"#,
    );
    {
        let mut reg = world.resource_mut::<BuildingRegistry>();
        let mut m = bare_building_def("mine");
        m.on_built = Some(mine_hook);
        reg.insert(m);
        let mut s = bare_building_def("shipyard");
        s.on_built = Some(shipyard_hook);
        reg.insert(s);
    }
    {
        let mut sys_state: bevy::ecs::system::SystemState<(
            Res<ScriptEngine>,
            Res<BuildingRegistry>,
            Res<StructureRegistry>,
        )> = bevy::ecs::system::SystemState::new(&mut world);
        let (engine, br, sr) = sys_state.get(&world);
        register_building_built_hooks(engine, br, sr);
    }

    // Fire only for "mine".
    push_build_event(&mut world, "mine", "construction", None);
    dispatch_event_handlers(&mut world);

    let engine = world.resource::<ScriptEngine>();
    let mine_called: bool = engine.lua().globals().get("_mine_called").unwrap();
    let shipyard_called: bool = engine.lua().globals().get("_shipyard_called").unwrap();
    assert!(mine_called, "mine hook must fire");
    assert!(
        !shipyard_called,
        "shipyard hook must NOT fire when only mine was built"
    );
}

#[test]
fn test_on_upgraded_hook_separate_from_on_built() {
    let mut world = hook_test_world();
    {
        let engine = world.resource::<ScriptEngine>();
        engine
            .lua()
            .load(
                r#"_built_called = false
                   _upgraded_called = false
                   _upgraded_cause = nil"#,
            )
            .exec()
            .unwrap();
    }
    let built = compile_hook(
        &mut world,
        r#"return function(evt) _built_called = true end"#,
    );
    let upgraded = compile_hook(
        &mut world,
        r#"return function(evt)
              _upgraded_called = true
              _upgraded_cause = evt.cause
           end"#,
    );
    {
        let mut reg = world.resource_mut::<BuildingRegistry>();
        let mut def = bare_building_def("mine");
        def.on_built = Some(built);
        def.on_upgraded = Some(upgraded);
        reg.insert(def);
    }
    {
        let mut sys_state: bevy::ecs::system::SystemState<(
            Res<ScriptEngine>,
            Res<BuildingRegistry>,
            Res<StructureRegistry>,
        )> = bevy::ecs::system::SystemState::new(&mut world);
        let (engine, br, sr) = sys_state.get(&world);
        register_building_built_hooks(engine, br, sr);
    }

    push_build_event(&mut world, "mine", "upgrade", Some("old_mine"));
    dispatch_event_handlers(&mut world);

    let engine = world.resource::<ScriptEngine>();
    let built_called: bool = engine.lua().globals().get("_built_called").unwrap();
    let upgraded_called: bool = engine.lua().globals().get("_upgraded_called").unwrap();
    let cause: String = engine.lua().globals().get("_upgraded_cause").unwrap();
    assert!(
        !built_called,
        "on_built must NOT fire for cause=upgrade (filtered by cause)"
    );
    assert!(upgraded_called, "on_upgraded must fire for cause=upgrade");
    assert_eq!(cause, "upgrade");
}

#[test]
fn test_definition_hook_co_exists_with_external_subscription() {
    let mut world = hook_test_world();
    {
        let engine = world.resource::<ScriptEngine>();
        engine
            .lua()
            .load(
                r#"_def_called = false
                   _ext_called = false
                   on("macrocosmo:building_built", function(evt)
                       _ext_called = true
                       _ext_id = evt.building_id
                   end)"#,
            )
            .exec()
            .unwrap();
    }
    let def_hook = compile_hook(&mut world, r#"return function(evt) _def_called = true end"#);
    {
        let mut reg = world.resource_mut::<BuildingRegistry>();
        let mut def = bare_building_def("mine");
        def.on_built = Some(def_hook);
        reg.insert(def);
    }
    {
        let mut sys_state: bevy::ecs::system::SystemState<(
            Res<ScriptEngine>,
            Res<BuildingRegistry>,
            Res<StructureRegistry>,
        )> = bevy::ecs::system::SystemState::new(&mut world);
        let (engine, br, sr) = sys_state.get(&world);
        register_building_built_hooks(engine, br, sr);
    }

    push_build_event(&mut world, "mine", "construction", None);
    dispatch_event_handlers(&mut world);

    let engine = world.resource::<ScriptEngine>();
    let def_called: bool = engine.lua().globals().get("_def_called").unwrap();
    let ext_called: bool = engine.lua().globals().get("_ext_called").unwrap();
    let ext_id: String = engine.lua().globals().get("_ext_id").unwrap();
    assert!(def_called, "definition-level on_built must fire");
    assert!(
        ext_called,
        "external on() subscription must also fire for the same event"
    );
    assert_eq!(ext_id, "mine");
}
