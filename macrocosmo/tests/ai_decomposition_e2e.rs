//! H1 (#447): Full Bevy app integration test for the
//! `colonize_system → deploy_deliverable + colonize_planet` AI macro
//! decomposition chain.
//!
//! Round 9 PR series + F-track context: AI commands flow producer →
//! `AiCommandOutbox` → consumer (`drain_ai_commands`) → ECS event
//! handlers. The Short layer (`CampaignReactiveShort`) intercepts macro
//! commands and expands them into primitive sequences via
//! [`build_default_registry`](macrocosmo::ai::decomposition_rules::build_default_registry):
//!
//! ```text
//! colonize_system  →  [
//!     build_deliverable,
//!     load_deliverable,
//!     reposition,
//!     unload_deliverable,
//!     colonize_planet,
//! ]
//! ```
//!
//! The F4 unit smoke (`colonize_system_macro_decomposes_full_chain_via_short_agent`
//! in `decomposition_rules.rs`) asserts the *abstract* chain at the
//! `CampaignReactiveShort` boundary. This test is the **end-to-end**
//! companion: it pipes the chain through the real Bevy schedule
//! (`run_short_agents` → `dispatch_ai_pending_commands` →
//! `process_ai_pending_commands` → `drain_ai_commands` → handler dispatch
//! → ECS event emission), verifying that each primitive surfaces as the
//! correct game-side ECS event one hexadies after the prior:
//!
//! | primitive            | observable game-side effect                              |
//! |----------------------|----------------------------------------------------------|
//! | `build_deliverable`  | `BuildOrder` pushed onto the `BuildQueue` component      |
//! | `load_deliverable`   | `LoadDeliverableRequested` message                        |
//! | `reposition`         | `MoveRequested` message                                   |
//! | `unload_deliverable` | `DeployDeliverableRequested` message                      |
//! | `colonize_planet`    | `ColonizeRequested { planet: Some(_), .. }` message       |
//!
//! ## How the macro reaches the short layer
//!
//! `CampaignReactiveShort::make_command` synthesizes a command from a
//! Campaign with only `campaign` / `source_intent` params — none of the
//! spatial parameters that the consumer-side handlers require
//! (`target_system`, `target_planet`, `ship_*`, `definition_id`). To
//! exercise the full pipeline we therefore seed the courier fleet's
//! `ShortAgent` `PlanState` (#449 PR2c — the per-fleet replacement for
//! the deleted `OrchestratorState.plan_states` map) directly with a
//! fully-parameterized primitive sequence. The Short layer's
//! `intercept_and_drain` Step 2 drains one head per non-empty slot per
//! tick, so over five `advance_time(&mut app, 1)` steps the agent emits
//! the five primitives in order.
//!
//! This is faithful to the F-track contract: F2/F3 build the registry
//! and the rules, F4 wires `intercept_and_drain` against `PlanState`, and
//! H1 (this file) shows the per-primitive emission flowing all the way
//! through the production Bevy systems to the ECS events the gameplay
//! handlers consume.

mod common;

use bevy::prelude::*;

use macrocosmo::ai::core::{Command, CommandValue, ObjectiveId};
use macrocosmo::ai::schema::ids::command as cmd_ids;
use macrocosmo::ai::short_agent::{ShortAgent, ShortScope};
use macrocosmo::colony::{BuildKind, BuildQueue};
use macrocosmo::empire::CommsParams;
use macrocosmo::galaxy::HomeSystem;
use macrocosmo::knowledge::{KnowledgeStore, SystemVisibilityMap};
use macrocosmo::player::{Empire, Faction};
use macrocosmo::ship::command_events::{
    ColonizeRequested, DeployDeliverableRequested, LoadDeliverableRequested, MoveRequested,
};
use macrocosmo::ship::{Cargo, CargoItem, Owner, Ship};
use macrocosmo_core::amount::Amt;

use common::{
    advance_time, spawn_mock_core_ship, spawn_test_ruler, spawn_test_ship,
    spawn_test_system_with_planet, test_app,
};

/// Build a minimal `infra_core` deliverable definition and graft it
/// onto the test deliverable registry. Mirrors the production
/// `infrastructure_core` shape from `scripts/structures/cores.lua` but
/// with cost / build_time trimmed so handlers don't gate on production
/// progress.
///
/// #532 F1: this used to inject a fake `ShipDesignDefinition` keyed on
/// the deliverable id because `handle_build_deliverable` previously
/// resolved through `ShipDesignRegistry`. After F1 the handler resolves
/// through `DeliverableRegistry`, so this fixture inserts the real
/// shape — anything else and the bug-free production handler would
/// reject `definition_id = "infra_core"` as unknown.
fn install_infra_core_design(app: &mut App) {
    use macrocosmo::deep_space::{
        DeliverableMetadata, DeliverableRegistry, ResourceCost, StructureDefinition,
    };
    let mut registry = app.world_mut().resource_mut::<DeliverableRegistry>();
    registry.insert(StructureDefinition {
        id: "infra_core".into(),
        name: "Infrastructure Core".into(),
        description: String::new(),
        max_hp: 1.0,
        energy_drain: Amt::ZERO,
        capabilities: std::collections::HashMap::new(),
        prerequisites: None,
        deliverable: Some(DeliverableMetadata {
            cost: ResourceCost {
                minerals: Amt::units(20),
                energy: Amt::units(10),
            },
            build_time: 5,
            cargo_size: 1,
            scrap_refund: 0.25,
            // The deploy / spawn pipeline is not exercised by this
            // decomposition E2E test (cargo is pre-seeded; unload only
            // checks the cargo entry by id, not the registry's
            // `spawns_as_ship` ref). Leaving `None` keeps the fixture
            // minimal.
            spawns_as_ship: None,
        }),
        upgrade_to: Vec::new(),
        upgrade_from: None,
        on_built: None,
        on_upgraded: None,
    });
}

/// Spawn an NPC empire with a `Faction` (no `PlayerEmpire`) and the
/// component soup the AI integration layer expects:
/// `KnowledgeStore`, `SystemVisibilityMap`, and `CommsParams`. The
/// `HomeSystem` link is attached separately by the caller after the
/// home `StarSystem` exists.
fn spawn_npc_empire(world: &mut World) -> Entity {
    world
        .spawn((
            Empire {
                name: "NPC Decomposition Test".into(),
            },
            Faction::new("npc_decomp_test", "NPC Decomposition Test"),
            KnowledgeStore::default(),
            SystemVisibilityMap::default(),
            CommsParams::default(),
        ))
        .id()
}

/// Spawn the empire's initial `Region` + `MidAgent` pair so the new
/// `ShortAgent` spawn hook (`spawn_short_agent_for_new_fleets`) can
/// resolve `system → region → mid_agent`. `test_app()` starts the
/// world directly in `InGame` and bypasses the production
/// `OnEnter(NewGame)` spawn pipeline, so PR2c tests must seed the
/// region structure by hand. Mirrors the path in
/// `setup::spawn_initial_region_for_faction` minus the Lua-side
/// faction lookup.
fn install_region_and_mid_agent(app: &mut App, empire: Entity, home_system: Entity) -> Entity {
    use macrocosmo::ai::MidAgent;
    use macrocosmo::region::{Region, RegionMembership, RegionRegistry, spawn_initial_region};

    let world = app.world_mut();
    if world.get_resource::<RegionRegistry>().is_none() {
        world.insert_resource(RegionRegistry::default());
    }
    // Skip if a region already covers the home system (defensive).
    if world.get::<RegionMembership>(home_system).is_some() {
        let existing = world
            .get::<RegionMembership>(home_system)
            .map(|m| m.region)
            .unwrap();
        return existing;
    }
    let region = spawn_initial_region(world, empire, home_system);
    let mid_agent = world
        .spawn(MidAgent {
            region,
            state: macrocosmo::ai::core::MidTermState::default(),
            auto_managed: true,
        })
        .id();
    if let Some(mut region_comp) = world.get_mut::<Region>(region) {
        region_comp.mid_agent = Some(mid_agent);
    }
    region
}

/// Find the `ShortAgent` attached to `courier`'s fleet and seed its
/// `PlanState` with the fully-parameterized primitive sequence. The
/// Short layer's `intercept_and_drain` Step 2 will surface one head
/// per non-empty slot per tick.
///
/// The slot key `(macro_kind, ObjectiveId)` may be any pair as long as
/// it's unique within the `BTreeMap`. We use `(colonize_system,
/// "decomp-e2e")` so debug logs trace cleanly back to this test if a
/// regression ever surfaces in CI.
fn seed_plan_state(
    app: &mut App,
    empire: Entity,
    home_system: Entity,
    target_system: Entity,
    target_planet: Entity,
    courier: Entity,
) {
    let issuer = macrocosmo::ai::convert::to_ai_faction(empire);
    let home_sys_ref = macrocosmo::ai::convert::to_ai_system(home_system);
    let target_sys_ref = macrocosmo::ai::convert::to_ai_system(target_system);
    let target_planet_ref = macrocosmo::ai::convert::to_ai_entity(target_planet);
    let courier_ref = macrocosmo::ai::convert::to_ai_entity(courier);

    // Build the five primitives with the params each consumer-side
    // handler needs. Mirrors the param transfer the decomposition
    // rules in `decomposition_rules.rs` would themselves perform when
    // a parameter-rich `colonize_system` macro is expanded.
    let now = 0;
    let build = Command::new(cmd_ids::build_deliverable(), issuer, now)
        .with_param("definition_id", CommandValue::Str("infra_core".into()))
        .with_param("target_system", CommandValue::System(target_sys_ref));

    let load = Command::new(cmd_ids::load_deliverable(), issuer, now)
        .with_param("target_system", CommandValue::System(target_sys_ref))
        .with_param("definition_id", CommandValue::Str("infra_core".into()))
        .with_param("ship_count", CommandValue::I64(1))
        .with_param("ship_0", CommandValue::Entity(courier_ref))
        .with_param("ship", CommandValue::Entity(courier_ref))
        .with_param("stockpile_index", CommandValue::I64(0));

    // #468 PR-3 NICE-TO-FIX fold-in: reposition target moved from
    // `target_sys_ref` to `home_sys_ref` so the courier (which now
    // starts at `target` for the load precheck to succeed) is
    // moving to a non-current system. With both endpoints (courier
    // at target, reposition target = target) `apply_move_to_ship`
    // would emit a no-op skip and the tick-3 assertion would
    // observe zero events.
    let mv = Command::new(cmd_ids::reposition(), issuer, now)
        .with_param("target_system", CommandValue::System(home_sys_ref))
        .with_param("ship_count", CommandValue::I64(1))
        .with_param("ship_0", CommandValue::Entity(courier_ref));

    let unload = Command::new(cmd_ids::unload_deliverable(), issuer, now)
        .with_param("definition_id", CommandValue::Str("infra_core".into()))
        .with_param("ship_count", CommandValue::I64(1))
        .with_param("ship_0", CommandValue::Entity(courier_ref))
        .with_param("ship", CommandValue::Entity(courier_ref))
        .with_param("item_index", CommandValue::I64(0));

    let colonize = Command::new(cmd_ids::colonize_planet(), issuer, now)
        .with_param("target_system", CommandValue::System(target_sys_ref))
        .with_param("target_planet", CommandValue::Entity(target_planet_ref))
        .with_param("ship_count", CommandValue::I64(1))
        .with_param("ship_0", CommandValue::Entity(courier_ref))
        .with_param("ship", CommandValue::Entity(courier_ref));

    // Resolve courier's fleet → ShortAgent. `spawn_test_ship` always
    // creates a 1-ship fleet inline, so the courier's `Ship.fleet` is
    // populated synchronously.
    let fleet = app
        .world()
        .get::<macrocosmo::ship::Ship>(courier)
        .and_then(|s| s.fleet)
        .expect("courier must have a Fleet");
    let mut short_agent = app.world_mut().get_mut::<ShortAgent>(fleet).expect(
        "ShortAgent must already be installed on the courier fleet \
             (run `app.update()` after region+mid setup so \
             `spawn_short_agent_for_new_fleets` fires before seeding)",
    );
    assert!(
        matches!(short_agent.scope, ShortScope::Fleet(f) if f == fleet),
        "courier fleet's ShortAgent.scope should match"
    );
    short_agent.state.pending.insert(
        (cmd_ids::colonize_system(), ObjectiveId::from("decomp-e2e")),
        vec![build, load, mv, unload, colonize],
    );
}

/// Convenience: count messages of a given type emitted during the
/// most recent `Update`. Mirrors the helper in
/// `tests/ai_command_lightspeed.rs`.
fn current_count<M: bevy::ecs::message::Message>(app: &App) -> usize {
    let messages = app.world().resource::<bevy::ecs::message::Messages<M>>();
    messages.iter_current_update_messages().count()
}

/// H1 regression: full Bevy app integration test for the
/// `colonize_system` macro chain end-to-end. Drives `run_short_agents`
/// (registered by `AiPlugin` in `test_app()`), the AI command outbox,
/// and the consumer pipeline; asserts each primitive surfaces as its
/// intended ECS event in the per-tick order
/// `BuildQueue.push → LoadDeliverableRequested → MoveRequested →
/// DeployDeliverableRequested → ColonizeRequested { planet: Some(_) }`.
#[test]
fn npc_colonize_system_decomposes_through_full_event_chain() {
    let mut app = test_app();

    // The default `test_app()` design registry doesn't ship an
    // `infra_core` definition (the production one comes from the Lua
    // scripts, which we don't run in headless tests). Inject the
    // minimal shape `handle_build_deliverable` looks up.
    install_infra_core_design(&mut app);

    // `run_short_agents` is registered by `AiPlugin::build` (already
    // included in `test_app()`); we don't add it again here since the
    // plugin set up the `.after(npc_decision_tick)` ordering and the
    // `run_if(in_state(GameState::InGame))` gate, and `test_app()`
    // seeds the world directly into `GameState::InGame`. Adding the
    // system a second time would also panic on `SystemTypeSet`
    // ambiguity because `dispatch_ai_pending_commands.after(run_short_agents)`
    // already references the type-set form.

    // Spawn the NPC empire (no PlayerEmpire — this is the AI-controlled
    // path) and the home + target systems. Both at [0, 0, 0] so that
    // the AI command outbox computes light_delay = 0 for every primitive
    // (capital-bound and target_system-bound alike): the orchestrator
    // tick → outbox dispatch → outbox release → consumer drain → handler
    // event chain all collapses into a single Bevy `Update`.
    let npc_empire = spawn_npc_empire(app.world_mut());
    let (home, home_planet) =
        spawn_test_system_with_planet(app.world_mut(), "Home", [0.0, 0.0, 0.0], 1.0, true);
    let (target, target_planet) =
        spawn_test_system_with_planet(app.world_mut(), "Target", [0.0, 0.0, 0.0], 0.8, false);

    // Sovereignty: NPC owns home (so `handle_build_deliverable` finds
    // an owned system to queue the BuildOrder against) but not target.
    // The `Sovereignty.owner` field is **derived** every Update by
    // `update_sovereignty`, which scans for `CoreShip + AtSystem(home)
    // + FactionOwner(npc_empire)`. A directly-set `Sovereignty.owner`
    // would be overwritten on the first tick — we therefore plant a
    // mock Core ship at home (no Position / Ship components needed by
    // the sovereignty derivation, just `(CoreShip, AtSystem,
    // FactionOwner)`).
    spawn_mock_core_ship(app.world_mut(), home, npc_empire);
    // #470: `BuildQueue` is the per-**colony** order accumulator the
    // build_deliverable arm pushes into (not per-system — that was the
    // bug fixed in #470). Spawn a host colony at the home planet with
    // FactionOwner so `handle_build_deliverable` can route the order.
    let home_colony = app
        .world_mut()
        .spawn((
            macrocosmo::colony::Colony {
                planet: home_planet,
                growth_rate: 0.0,
            },
            BuildQueue::default(),
            macrocosmo::faction::FactionOwner(npc_empire),
        ))
        .id();

    // `HomeSystem` is the canonical capital pointer used by the AI
    // outbox's `resolve_capital_system` chain. Without it, capital
    // resolution falls back to scanning for any `is_capital` system,
    // which `spawn_test_system_with_planet` doesn't set — capital
    // resolution would fail and the dispatcher would `warn! + drop`
    // every spatial-less primitive (build_deliverable, load_deliverable,
    // unload_deliverable, colonize_planet).
    app.world_mut()
        .entity_mut(npc_empire)
        .insert(HomeSystem(home));

    // Ruler stationed at home: collapses ruler→capital and ruler→target
    // light-delay to 0 (target is co-located with home at [0, 0, 0]).
    spawn_test_ruler(app.world_mut(), npc_empire, home);

    // #468 PR-3 NICE-TO-FIX #7 fold-in: the dispatcher now pre-gates
    // `load_deliverable` on the target system having a non-empty
    // `DeliverableStockpile`. The previous AI E2E behaviour (no
    // stockpile = pass-through emit, downstream Reject) no longer
    // applies. Seed a one-item stockpile so the load primitive arm
    // continues to emit during the tick-2 assertion below.
    app.world_mut()
        .entity_mut(target)
        .insert(macrocosmo::colony::DeliverableStockpile {
            items: vec![macrocosmo::ship::CargoItem::Deliverable {
                definition_id: "infra_core".into(),
            }],
        });

    // Courier ship at the TARGET system (was: at home), owned by the
    // NPC, with `infra_core` already pre-loaded into Cargo so
    // `handle_unload_deliverable` finds an item at `item_index = 0`
    // and writes `DeployDeliverableRequested`.
    //
    // #468 PR-3 NICE-TO-FIX fold-in: the courier must dock at the
    // target system (not at home) so the load_deliverable handler at
    // tick 2 succeeds in-place — the previous "courier at home"
    // setup relied on the load handler emitting unconditionally
    // even when the courier was not at the target. With the new
    // dispatcher precheck the load DOES emit (stockpile is now
    // populated) but the downstream handler would defer with a
    // MoveTo + retry insert into the courier's CommandQueue, which
    // would then block tick-3's reposition arm (`apply_move_to_ship`
    // requires an empty queue). Co-locating the courier at the
    // target keeps the handler's docked-system check satisfied so the
    // queue stays empty for subsequent ticks.
    let courier = spawn_test_ship(
        app.world_mut(),
        "Courier-1",
        "courier_mk1",
        target,
        [0.0, 0.0, 0.0],
    );
    {
        let mut em = app.world_mut().entity_mut(courier);
        em.get_mut::<Ship>().unwrap().owner = Owner::Empire(npc_empire);
        em.get_mut::<Cargo>()
            .unwrap()
            .items
            .push(CargoItem::Deliverable {
                definition_id: "infra_core".into(),
            });
    }

    // #449 PR2c: install the Region + MidAgent pair so the Added<Fleet>
    // hook (`spawn_short_agent_for_new_fleets`) can resolve
    // `home_system → region → mid_agent` and attach a `ShortAgent` to
    // the courier's fleet. Then run one Update so the spawn hook
    // fires before we mutate the agent's state.
    install_region_and_mid_agent(&mut app, npc_empire, home);
    app.update();
    seed_plan_state(&mut app, npc_empire, home, target, target_planet, courier);

    // Reset the message buffers so `iter_current_update_messages()`
    // observes only events emitted by the systems-under-test rather
    // than any startup chatter.
    app.world_mut()
        .resource_mut::<bevy::ecs::message::Messages<LoadDeliverableRequested>>()
        .update();
    app.world_mut()
        .resource_mut::<bevy::ecs::message::Messages<MoveRequested>>()
        .update();
    app.world_mut()
        .resource_mut::<bevy::ecs::message::Messages<DeployDeliverableRequested>>()
        .update();
    app.world_mut()
        .resource_mut::<bevy::ecs::message::Messages<ColonizeRequested>>()
        .update();

    // ---- Tick 1: build_deliverable ----------------------------------------
    // The first primitive is a `build_deliverable` — its game-side
    // effect is a `BuildOrder` pushed onto the home `BuildQueue`, not
    // an ECS message. We assert the BuildQueue grew by one with the
    // `infra_core` design id.
    advance_time(&mut app, 1);
    {
        let queue = app
            .world()
            .get::<BuildQueue>(home_colony)
            .expect("home colony should have BuildQueue (#470)");
        assert_eq!(
            queue.queue.len(),
            1,
            "tick 1: build_deliverable arm should push 1 BuildOrder; got {} entries",
            queue.queue.len()
        );
        assert_eq!(
            queue.queue[0].design_id, "infra_core",
            "tick 1: queued BuildOrder should be the infra_core deliverable"
        );
        assert!(
            matches!(queue.queue[0].kind, BuildKind::Deliverable { .. }),
            "tick 1: BuildOrder kind should be Deliverable, got {:?}",
            queue.queue[0].kind,
        );
    }

    // ---- Tick 2: load_deliverable -----------------------------------------
    advance_time(&mut app, 1);
    assert_eq!(
        current_count::<LoadDeliverableRequested>(&app),
        1,
        "tick 2: load_deliverable arm should emit exactly 1 \
         LoadDeliverableRequested; got {}",
        current_count::<LoadDeliverableRequested>(&app),
    );

    // ---- Tick 3: reposition ----------------------------------------------
    advance_time(&mut app, 1);
    assert_eq!(
        current_count::<MoveRequested>(&app),
        1,
        "tick 3: reposition arm should emit exactly 1 MoveRequested; got {}",
        current_count::<MoveRequested>(&app),
    );

    // #468 PR-3 NICE-TO-FIX fold-in: reset the courier's runtime
    // ShipState + CommandQueue between ticks so the per-tick assertions
    // still observe single-event emissions. Without this:
    //   - tick 3's reposition turns the courier into `SubLight` or
    //     `InFTL` via `handle_move_requested` → `dispatch_queued_commands`
    //     → `start_sublight_travel` (the home-target axis is 0 ly so
    //     the move resolves immediately into one of the two transit
    //     states or back to InSystem at home);
    //   - tick 4's unload precheck requires InSystem/Loitering;
    //   - tick 5's colonize requires `queue.commands.is_empty()`.
    // We force the world back into the "courier at target, queue
    // empty" shape after each per-tick assertion so the next
    // primitive sees a clean ship.
    {
        use macrocosmo::ship::{CommandQueue, ShipState};
        let mut em = app.world_mut().entity_mut(courier);
        *em.get_mut::<ShipState>().unwrap() = ShipState::InSystem { system: target };
        em.get_mut::<CommandQueue>().unwrap().commands.clear();
    }

    // ---- Tick 4: unload_deliverable --------------------------------------
    advance_time(&mut app, 1);
    assert_eq!(
        current_count::<DeployDeliverableRequested>(&app),
        1,
        "tick 4: unload_deliverable arm should emit exactly 1 \
         DeployDeliverableRequested; got {}",
        current_count::<DeployDeliverableRequested>(&app),
    );

    // Same reset between tick 4 and tick 5 — the deploy handler may
    // adjust the cargo or trigger downstream state changes that
    // would break the colonize precheck.
    {
        use macrocosmo::ship::{CommandQueue, ShipState};
        let mut em = app.world_mut().entity_mut(courier);
        *em.get_mut::<ShipState>().unwrap() = ShipState::InSystem { system: target };
        em.get_mut::<CommandQueue>().unwrap().commands.clear();
    }

    // ---- Tick 5: colonize_planet -----------------------------------------
    advance_time(&mut app, 1);
    let colonize_msgs: Vec<ColonizeRequested> = {
        let messages = app
            .world()
            .resource::<bevy::ecs::message::Messages<ColonizeRequested>>();
        messages.iter_current_update_messages().cloned().collect()
    };
    assert_eq!(
        colonize_msgs.len(),
        1,
        "tick 5: colonize_planet arm should emit exactly 1 \
         ColonizeRequested; got {}",
        colonize_msgs.len(),
    );
    let evt = &colonize_msgs[0];
    assert_eq!(
        evt.planet,
        Some(target_planet),
        "tick 5: ColonizeRequested should carry the explicit \
         target_planet (Some(_)) rather than the legacy `colonize_system` \
         planet=None form"
    );
    assert_eq!(
        evt.target_system, target,
        "tick 5: ColonizeRequested.target_system should match the \
         macro's target_system param"
    );
    assert_eq!(
        evt.ship, courier,
        "tick 5: ColonizeRequested.ship should be the seeded courier"
    );
}
