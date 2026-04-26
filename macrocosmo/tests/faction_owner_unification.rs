//! Integration tests for #297 (S-2): `FactionOwner` must attach to every
//! empire-owned Colony / SystemBuildings-bearing StarSystem / Ship /
//! DeepSpaceStructure at spawn time, so all owned classes share a single
//! diplomatic-identity component.
//!
//! See `docs/plan-297-faction-owner-unification.md` for the full scope.

use bevy::prelude::*;

use macrocosmo::amount::Amt;
use macrocosmo::colony::{
    Buildings, ColonizationOrder, ColonizationQueue, Colony, LastProductionTick, Production,
    ResourceStockpile, SystemBuildings,
};
use macrocosmo::components::Position;
use macrocosmo::deep_space::{
    DeepSpaceStructure, DeliverableMetadata, ResourceCost, StructureDefinition, StructureRegistry,
    spawn_deliverable_entity,
};
use macrocosmo::faction::{FactionOwner, entity_owner};
use macrocosmo::galaxy::{
    Anomalies, AtSystem, Hostile, HostileHitpoints, HostileStats, Planet, Sovereignty, StarSystem,
    SystemAttributes, SystemModifiers,
};
use macrocosmo::player::{Empire, Faction, PlayerEmpire};
use macrocosmo::ship::{Owner, Ship, ShipState, spawn_ship};
use macrocosmo::ship_design::ShipDesignRegistry;
use macrocosmo::time_system::{GameClock, GameSpeed};
use std::collections::HashMap;

mod common;
use common::{spawn_test_system_with_planet, test_app};

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Spawn a minimal PlayerEmpire entity with the two components read by
/// `spawn_capital_colony` / `apply_game_start_actions`.
fn spawn_player_empire_entity(world: &mut World, faction_id: &str) -> Entity {
    world
        .spawn((
            Empire {
                name: "Test Empire".into(),
            },
            PlayerEmpire,
            Faction::new(faction_id, "Test Faction"),
        ))
        .id()
}

/// Spawn a capital-style StarSystem + its first habitable planet so
/// `spawn_capital_colony` can find the capital.
fn spawn_capital_system_with_planet(world: &mut World) -> (Entity, Entity) {
    let sys = world
        .spawn((
            StarSystem {
                name: "Sol".into(),
                surveyed: true,
                is_capital: true,
                star_type: "yellow_dwarf".into(),
            },
            Position::from([0.0, 0.0, 0.0]),
            Sovereignty::default(),
            macrocosmo::technology::TechKnowledge::default(),
            SystemModifiers::default(),
            Anomalies::default(),
        ))
        .id();
    let planet = world
        .spawn((
            Planet {
                name: "Earth".into(),
                system: sys,
                planet_type: "terrestrial".into(),
            },
            SystemAttributes {
                habitability: 1.0,
                mineral_richness: 0.5,
                energy_potential: 0.5,
                research_potential: 0.5,
                max_building_slots: 4,
            },
            Position::from([0.0, 0.0, 0.0]),
        ))
        .id();
    (sys, planet)
}

// ---------------------------------------------------------------------------
// spawn_capital_colony
// ---------------------------------------------------------------------------

#[test]
fn spawn_capital_colony_is_noop() {
    // spawn_capital_colony is now a no-op — colony creation is handled by
    // each faction's on_game_start Lua callback (#429).
    let mut app = App::new();
    app.add_plugins(MinimalPlugins);
    let _empire = spawn_player_empire_entity(app.world_mut(), "humanity");
    let (_capital_system, _planet) = spawn_capital_system_with_planet(app.world_mut());
    app.add_systems(Update, macrocosmo::colony::spawn_capital_colony);
    app.update();

    let mut q = app.world_mut().query::<&Colony>();
    let colonies: Vec<_> = q.iter(app.world()).collect();
    assert_eq!(colonies.len(), 0, "spawn_capital_colony should be a no-op");
}

// ---------------------------------------------------------------------------
// tick_colonization_queue — new colony inherits source owner
// ---------------------------------------------------------------------------

#[test]
fn tick_colonization_queue_inherits_source_colony_owner() {
    let mut app = test_app();

    // Two mock empires. The source colony is owned by empire A; we assert
    // the new colony is tagged with empire A (not PlayerEmpire or empire B).
    let empire_a = app.world_mut().spawn_empty().id();
    let _empire_b = app.world_mut().spawn_empty().id();

    // StarSystem with stockpile + an in-flight colonization queue.
    let (sys, _planet_a) =
        spawn_test_system_with_planet(app.world_mut(), "TestSys", [0.0, 0.0, 0.0], 1.0, true);
    // Target planet in the same system.
    let target_planet = app
        .world_mut()
        .spawn((
            Planet {
                name: "TargetPlanet".into(),
                system: sys,
                planet_type: "terrestrial".into(),
            },
            SystemAttributes {
                habitability: 1.0,
                mineral_richness: 0.5,
                energy_potential: 0.5,
                research_potential: 0.5,
                max_building_slots: 3,
            },
            Position::from([0.1, 0.0, 0.0]),
        ))
        .id();

    // Source colony, owned by empire A, on a separate planet.
    let source_planet = app
        .world_mut()
        .spawn((
            Planet {
                name: "SourcePlanet".into(),
                system: sys,
                planet_type: "terrestrial".into(),
            },
            SystemAttributes {
                habitability: 1.0,
                mineral_richness: 0.5,
                energy_potential: 0.5,
                research_potential: 0.5,
                max_building_slots: 3,
            },
            Position::from([0.2, 0.0, 0.0]),
        ))
        .id();
    let source_colony = app
        .world_mut()
        .spawn((
            Colony {
                planet: source_planet,
                growth_rate: 0.005,
            },
            FactionOwner(empire_a),
        ))
        .id();

    // Attach stockpile + queue to the system.
    app.world_mut().entity_mut(sys).insert((
        ResourceStockpile {
            minerals: Amt::units(1_000),
            energy: Amt::units(1_000),
            research: Amt::ZERO,
            food: Amt::units(500),
            authority: Amt::ZERO,
        },
        ColonizationQueue {
            orders: vec![ColonizationOrder {
                target_planet,
                source_colony,
                minerals_remaining: Amt::ZERO,
                energy_remaining: Amt::ZERO,
                // Build completes on the next tick.
                build_time_remaining: 0,
                initial_population: 10.0,
            }],
        },
    ));

    // Drive one tick through the full pipeline so `tick_colonization_queue`
    // runs with `delta > 0`.
    app.world_mut().resource_mut::<GameClock>().elapsed = 1;
    app.world_mut().resource_mut::<LastProductionTick>().0 = 0;
    app.update();

    // Exactly one new colony on `target_planet`, tagged with empire_a.
    let mut q = app
        .world_mut()
        .query::<(Entity, &Colony, Option<&FactionOwner>)>();
    let new_colony = q
        .iter(app.world())
        .find(|(_, c, _)| c.planet == target_planet)
        .expect("new colony on target_planet must exist");
    let owner = new_colony
        .2
        .expect("new colony must inherit FactionOwner from source");
    assert_eq!(
        owner.0, empire_a,
        "inherited owner must match source colony's empire_a"
    );
}

// ---------------------------------------------------------------------------
// process_settling — colony spawned by settling ship carries FactionOwner
// ---------------------------------------------------------------------------

#[test]
fn process_settling_attaches_faction_owner_via_ship_owner() {
    let mut app = test_app();
    let empire = app.world_mut().spawn_empty().id();

    // System + habitable planet.
    let (sys, _planet) =
        spawn_test_system_with_planet(app.world_mut(), "Settle-Sys", [0.0, 0.0, 0.0], 1.0, true);

    // #299 (S-5): A Core owned by the empire must exist in the target
    // system for settling to succeed.
    common::spawn_mock_core_ship(app.world_mut(), sys, empire);

    // Colony ship owned by `empire` (via `Ship.owner`, no `FactionOwner`
    // component) — exercises the ship.owner fallback branch.
    let ship = common::spawn_test_ship(
        app.world_mut(),
        "Colonist",
        "colony_ship_mk1",
        sys,
        [0.0, 0.0, 0.0],
    );
    // Upgrade the test ship from Neutral to Empire for this scenario.
    {
        let mut s = app.world_mut().get_mut::<Ship>(ship).unwrap();
        s.owner = Owner::Empire(empire);
    }
    let mut state = app.world_mut().get_mut::<ShipState>(ship).unwrap();
    *state = ShipState::Settling {
        system: sys,
        planet: None,
        started_at: 0,
        completes_at: 1,
    };

    app.world_mut().resource_mut::<GameClock>().elapsed = 2;
    app.update();

    // A new Colony should exist at `sys`, tagged with FactionOwner(empire).
    let mut q = app.world_mut().query::<(&Colony, Option<&FactionOwner>)>();
    let (colony, owner) = q
        .iter(app.world())
        .next()
        .expect("settling must have produced a Colony");
    let _ = colony;
    let owner = owner.expect("settled colony must carry FactionOwner");
    assert_eq!(owner.0, empire);

    // The StarSystem should now also carry FactionOwner because
    // SystemBuildings was freshly attached.
    let sys_owner = app
        .world()
        .get::<FactionOwner>(sys)
        .expect("StarSystem must carry FactionOwner once SystemBuildings appears");
    assert_eq!(sys_owner.0, empire);
    assert!(
        app.world().get::<SystemBuildings>(sys).is_some(),
        "SystemBuildings must be present on the settled system"
    );
}

#[test]
fn process_settling_neutral_ship_produces_unowned_colony() {
    let mut app = test_app();

    let (sys, _planet) =
        spawn_test_system_with_planet(app.world_mut(), "Neutral-Sys", [0.0, 0.0, 0.0], 1.0, true);
    let ship = common::spawn_test_ship(
        app.world_mut(),
        "Drifter",
        "colony_ship_mk1",
        sys,
        [0.0, 0.0, 0.0],
    );
    // `spawn_test_ship` already defaults to `Owner::Neutral` with no
    // `FactionOwner` component — exactly the state we want to test.
    let mut state = app.world_mut().get_mut::<ShipState>(ship).unwrap();
    *state = ShipState::Settling {
        system: sys,
        planet: None,
        started_at: 0,
        completes_at: 1,
    };

    app.world_mut().resource_mut::<GameClock>().elapsed = 2;
    app.update();

    let mut q = app.world_mut().query::<(&Colony, Option<&FactionOwner>)>();
    let (_, owner) = q.iter(app.world()).next().expect("colony must spawn");
    assert!(
        owner.is_none(),
        "colony settled by neutral ship must not carry FactionOwner"
    );
    assert!(
        app.world().get::<FactionOwner>(sys).is_none(),
        "StarSystem settled by neutral ship must not carry FactionOwner"
    );
}

// ---------------------------------------------------------------------------
// Round 9 follow-up: process_settling consults the *settling ship's*
// faction relations, not the player empire's. Pre-fix, an NPC ship
// settling at a system co-located with a hostile that the NPC was
// neutral-friendly toward (but the player was at war with) was wrongly
// blocked because the gate consumed `With<PlayerEmpire>` for the viewer.
// ---------------------------------------------------------------------------

#[test]
fn process_settling_uses_ship_faction_for_hostile_gate_not_player() {
    use macrocosmo::faction::{
        FactionOwner, FactionRelations, FactionView, HostileFactions, RelationState,
    };

    let mut app = test_app();

    // A separate NPC empire (not the test PlayerEmpire). The test_app
    // helper does not spawn a PlayerEmpire by default for this file —
    // explicit empire entity is enough for `Ship.owner = Empire(npc)` and
    // FactionOwner-based settlement.
    let npc_empire = app
        .world_mut()
        .spawn((
            macrocosmo::player::Empire { name: "NPC".into() },
            macrocosmo::player::Faction::new("npc_faction", "NPC Faction"),
        ))
        .id();

    let (sys, _planet) = spawn_test_system_with_planet(
        app.world_mut(),
        "Friendly-Hostile-Sys",
        [0.0, 0.0, 0.0],
        1.0,
        true,
    );

    // Sovereignty: the settling faction needs a Core ship of its own at
    // the target system (#299 S-5 safety net). Borrow the existing
    // helper to spawn one for the NPC empire.
    common::spawn_mock_core_ship(app.world_mut(), sys, npc_empire);

    // Initialise the hostile-faction registry so the relation we override
    // below targets a real entity.
    {
        let world = app.world_mut();
        let needs_setup = {
            let hf = world.resource::<HostileFactions>();
            hf.space_creature.is_none()
        };
        if needs_setup {
            let sc_faction = world
                .spawn(macrocosmo::player::Faction::new(
                    "space_creature_faction",
                    "Space Creatures",
                ))
                .id();
            let mut hf = world.resource_mut::<HostileFactions>();
            hf.space_creature = Some(sc_faction);
        }
    }

    // Spawn a passive hostile owned by `space_creature` faction.
    let _hostile = common::spawn_raw_hostile(
        app.world_mut(),
        sys,
        500.0,
        500.0,
        0.0, // strength = 0 so combat doesn't kill the colony ship
        0.0,
        "space_creature",
    );

    // Override the NPC↔space_creature relation to friendly Neutral so
    // `can_attack_aggressive()` returns false from the NPC's perspective.
    // The default `seed_npc_relations` would set standing -100; we want
    // the explicit non-blocking case to verify the gate consults the NPC,
    // not the player.
    {
        let space_creature = app
            .world()
            .resource::<HostileFactions>()
            .space_creature
            .expect("space_creature faction should be initialised");
        let mut rel = app.world_mut().resource_mut::<FactionRelations>();
        rel.set(
            npc_empire,
            space_creature,
            FactionView::new(RelationState::Neutral, 50.0),
        );
        rel.set(
            space_creature,
            npc_empire,
            FactionView::new(RelationState::Neutral, 50.0),
        );
    }

    // Spawn a colony ship owned by the NPC empire and put it in
    // `Settling` state at this system.
    let ship = common::spawn_test_ship(
        app.world_mut(),
        "NPC-Colonist",
        "colony_ship_mk1",
        sys,
        [0.0, 0.0, 0.0],
    );
    {
        let mut s = app.world_mut().get_mut::<Ship>(ship).unwrap();
        s.owner = Owner::Empire(npc_empire);
    }
    app.world_mut()
        .entity_mut(ship)
        .insert(FactionOwner(npc_empire));
    {
        let mut state = app.world_mut().get_mut::<ShipState>(ship).unwrap();
        *state = ShipState::Settling {
            system: sys,
            planet: None,
            started_at: 0,
            completes_at: 1,
        };
    }

    app.world_mut().resource_mut::<GameClock>().elapsed = 2;
    app.update();

    // Pre-fix: the gate would have used `With<PlayerEmpire>` and (since
    // there is no player empire spawned in this test) defaulted to
    // blocking ALL hostiles regardless of NPC relations — the colony
    // would never spawn. Post-fix: the gate uses the NPC ship's own
    // faction, sees the friendly relation, and lets the ship settle.
    let colony_count = app.world_mut().query::<&Colony>().iter(app.world()).count();
    assert_eq!(
        colony_count, 1,
        "NPC ship friendly toward the co-located hostile must be allowed to settle \
         (pre-fix this used PlayerEmpire as the relations viewer, defaulting to block)"
    );

    // The colony also carries the NPC's FactionOwner (sanity check on
    // the existing #297 path).
    let mut q = app.world_mut().query::<(&Colony, &FactionOwner)>();
    let (_, owner) = q
        .iter(app.world())
        .next()
        .expect("the spawned colony must carry FactionOwner");
    assert_eq!(owner.0, npc_empire);
}

// ---------------------------------------------------------------------------
// spawn_ship — Commit 3
// ---------------------------------------------------------------------------

#[test]
fn spawn_ship_empire_gets_faction_owner() {
    let mut app = test_app();
    let empire = app.world_mut().spawn_empty().id();
    let (sys, _planet) =
        spawn_test_system_with_planet(app.world_mut(), "Sys", [0.0, 0.0, 0.0], 1.0, true);

    let ship = {
        let mut q_state =
            bevy::ecs::system::SystemState::<(Commands, Res<ShipDesignRegistry>)>::new(
                app.world_mut(),
            );
        let (mut commands, registry) = q_state.get_mut(app.world_mut());
        let e = spawn_ship(
            &mut commands,
            "scout_mk1",
            "Pioneer".into(),
            sys,
            Position::from([0.0, 0.0, 0.0]),
            Owner::Empire(empire),
            &registry,
        );
        q_state.apply(app.world_mut());
        e
    };

    let owner = app
        .world()
        .get::<FactionOwner>(ship)
        .expect("empire-owned ship must carry FactionOwner");
    assert_eq!(owner.0, empire);
}

#[test]
fn spawn_ship_neutral_has_no_faction_owner() {
    let mut app = test_app();
    let (sys, _planet) =
        spawn_test_system_with_planet(app.world_mut(), "Sys", [0.0, 0.0, 0.0], 1.0, true);

    let ship = {
        let mut q_state =
            bevy::ecs::system::SystemState::<(Commands, Res<ShipDesignRegistry>)>::new(
                app.world_mut(),
            );
        let (mut commands, registry) = q_state.get_mut(app.world_mut());
        let e = spawn_ship(
            &mut commands,
            "scout_mk1",
            "Drifter".into(),
            sys,
            Position::from([0.0, 0.0, 0.0]),
            Owner::Neutral,
            &registry,
        );
        q_state.apply(app.world_mut());
        e
    };

    assert!(
        app.world().get::<FactionOwner>(ship).is_none(),
        "Neutral ship must not carry FactionOwner"
    );
}

// ---------------------------------------------------------------------------
// spawn_deliverable_entity — Commit 3
// ---------------------------------------------------------------------------

fn make_single_structure_registry() -> StructureRegistry {
    let mut reg = StructureRegistry::default();
    reg.insert(StructureDefinition {
        id: "outpost".into(),
        name: "Outpost".into(),
        description: String::new(),
        max_hp: 100.0,
        energy_drain: Amt::ZERO,
        capabilities: HashMap::new(),
        prerequisites: None,
        deliverable: Some(DeliverableMetadata {
            cost: ResourceCost::default(),
            build_time: 1,
            cargo_size: 1,
            scrap_refund: 0.0,
            spawns_as_ship: None,
        }),
        upgrade_to: Vec::new(),
        upgrade_from: None,
        on_built: None,
        on_upgraded: None,
    });
    reg.rebuild_effective_edges();
    reg
}

#[test]
fn spawn_deliverable_entity_empire_gets_faction_owner() {
    let mut app = App::new();
    app.add_plugins(MinimalPlugins);
    let empire = app.world_mut().spawn_empty().id();

    let registry = make_single_structure_registry();

    let structure = {
        let mut q_state = bevy::ecs::system::SystemState::<Commands>::new(app.world_mut());
        let mut commands = q_state.get_mut(app.world_mut());
        let e = spawn_deliverable_entity(
            &mut commands,
            "outpost",
            [1.0, 2.0, 3.0],
            Owner::Empire(empire),
            &registry,
        )
        .expect("spawn_deliverable_entity must produce an entity for known def");
        q_state.apply(app.world_mut());
        e
    };

    let owner = app
        .world()
        .get::<FactionOwner>(structure)
        .expect("empire-owned structure must carry FactionOwner");
    assert_eq!(owner.0, empire);
    // Sanity: legacy owner field still agrees.
    assert!(matches!(
        app.world().get::<DeepSpaceStructure>(structure).unwrap().owner,
        Owner::Empire(e) if e == empire
    ));
}

#[test]
fn spawn_deliverable_entity_neutral_has_no_faction_owner() {
    let mut app = App::new();
    app.add_plugins(MinimalPlugins);
    let registry = make_single_structure_registry();

    let structure = {
        let mut q_state = bevy::ecs::system::SystemState::<Commands>::new(app.world_mut());
        let mut commands = q_state.get_mut(app.world_mut());
        let e = spawn_deliverable_entity(
            &mut commands,
            "outpost",
            [0.0; 3],
            Owner::Neutral,
            &registry,
        )
        .expect("registry defines `outpost`");
        q_state.apply(app.world_mut());
        e
    };

    assert!(
        app.world().get::<FactionOwner>(structure).is_none(),
        "Neutral structure must not carry FactionOwner"
    );
}

// ---------------------------------------------------------------------------
// entity_owner helper — integration across all five entity classes
// ---------------------------------------------------------------------------

#[test]
fn entity_owner_resolves_all_owned_classes() {
    let mut app = test_app();
    let empire = app.world_mut().spawn_empty().id();

    // Colony: `FactionOwner` only.
    let colony = app.world_mut().spawn(FactionOwner(empire)).id();

    // StarSystem with SystemBuildings-style ownership: `FactionOwner` only.
    let sys = app.world_mut().spawn(FactionOwner(empire)).id();

    // Ship with Owner::Empire only (no FactionOwner) — fallback path.
    let system_anchor = app.world_mut().spawn_empty().id();
    let (ship_empire_only_sys, _) =
        spawn_test_system_with_planet(app.world_mut(), "ShipSys", [0.0, 0.0, 0.0], 1.0, true);
    let ship = common::spawn_test_ship(
        app.world_mut(),
        "Explorer",
        "scout_mk1",
        ship_empire_only_sys,
        [0.0, 0.0, 0.0],
    );
    {
        let mut s = app.world_mut().get_mut::<Ship>(ship).unwrap();
        s.owner = Owner::Empire(empire);
    }

    // DeepSpaceStructure with FactionOwner.
    let structure = app
        .world_mut()
        .spawn((
            DeepSpaceStructure {
                definition_id: "outpost".into(),
                name: "Outpost".into(),
                owner: Owner::Empire(empire),
            },
            FactionOwner(empire),
            Position::from([0.0, 0.0, 0.0]),
        ))
        .id();

    // Hostile with FactionOwner (the classic case).
    let hostile_faction = app.world_mut().spawn_empty().id();
    let hostile = app
        .world_mut()
        .spawn((
            Hostile,
            AtSystem(sys),
            FactionOwner(hostile_faction),
            HostileHitpoints {
                hp: 10.0,
                max_hp: 10.0,
            },
            HostileStats {
                strength: 1.0,
                evasion: 0.0,
            },
        ))
        .id();

    // Neutral ship — returns None.
    let neutral_ship = common::spawn_test_ship(
        app.world_mut(),
        "Wanderer",
        "scout_mk1",
        ship_empire_only_sys,
        [0.0, 0.0, 0.0],
    );

    let world = app.world();
    assert_eq!(entity_owner(world, colony), Some(empire));
    assert_eq!(entity_owner(world, sys), Some(empire));
    assert_eq!(entity_owner(world, ship), Some(empire));
    assert_eq!(entity_owner(world, structure), Some(empire));
    assert_eq!(entity_owner(world, hostile), Some(hostile_faction));
    assert_eq!(entity_owner(world, neutral_ship), None);
    // Bare entity anchor with no relevant components.
    assert_eq!(entity_owner(world, system_anchor), None);
}

// Keep reporting `Production` / `Buildings` as used so `#[allow]` isn't
// needed for unused-import-style lints on common test imports.
#[allow(dead_code)]
fn _force_use() {
    let _: Option<Production> = None;
    let _: Option<Buildings> = None;
    let _: Option<GameSpeed> = None;
}
