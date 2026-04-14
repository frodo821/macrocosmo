//! Integration tests for #173: NPC empires must spawn in player mode
//! (`ObserverMode.enabled = false`) with healthy initial relations and
//! without panicking during tick.
//!
//! These tests exercise the real `GameSetupPlugin` + `ScriptingPlugin` +
//! `FactionRelationsPlugin` pipeline so the `.after(run_faction_on_game_start)`
//! ordering, the `existing_empire_ids` filter, and `seed_npc_relations` are
//! all covered.

use bevy::prelude::*;

use macrocosmo::ai::AiPlugin;
use macrocosmo::faction::{
    FactionRelations, FactionRelationsPlugin, HostileFactions, RelationState,
};
use macrocosmo::observer::{ObserverMode, ObserverPlugin, RngSeed};
use macrocosmo::player::{Empire, Faction, PlayerEmpire};
use macrocosmo::time_system::{GameClock, GameSpeed};

/// Build a player-mode app that mirrors (a subset of) the production plugin
/// list so NPC empire spawning goes through the real path:
///
/// - `observer_mode.enabled = false` (player mode)
/// - `GalaxyPlugin` generates the star map + capital
/// - `PlayerPlugin` spawns the player empire
/// - `ColonyPlugin` spawns the capital colony
/// - `ScriptingPlugin` + lifecycle loads the Lua definitions (including the
///   humanity/vesk/aurelian factions)
/// - `GameSetupPlugin` runs the faction `on_game_start` callbacks AND the
///   NPC-empire spawn loop (#173)
/// - `FactionRelationsPlugin` seeds hostile + NPC relations
/// - `AiPlugin` wires the `npc_decision_tick` under `AiTickSet::Reason`
///
/// We deliberately skip `UiPlugin` (egui) and the visualization plugin —
/// they require a windowing backend the headless test runner does not
/// provide.
fn player_mode_app() -> App {
    let mut app = App::new();
    app.add_plugins(MinimalPlugins);
    // `PlayerPlugin` registers `log_player_info` which takes
    // `Res<ButtonInput<KeyCode>>`. Headless MinimalPlugins don't provide
    // it, so add InputPlugin to satisfy the parameter.
    app.add_plugins(bevy::input::InputPlugin);

    // Observer mode explicitly disabled — this is the regression guard.
    app.insert_resource(ObserverMode {
        enabled: false,
        ..Default::default()
    });
    // Deterministic seed so the galaxy generator produces the same capital
    // every run.
    app.insert_resource(RngSeed(Some(0xC0FFEE)));
    app.insert_resource(GameClock::new(0));
    app.insert_resource(GameSpeed::default());

    // Production plugins required to exercise the real startup ordering.
    app.add_plugins((
        macrocosmo::time_system::GameTimePlugin,
        macrocosmo::galaxy::GalaxyPlugin,
        macrocosmo::player::PlayerPlugin,
        macrocosmo::communication::CommunicationPlugin,
        macrocosmo::knowledge::KnowledgePlugin,
        macrocosmo::ship::ShipPlugin,
        macrocosmo::colony::ColonyPlugin,
        macrocosmo::scripting::ScriptingPlugin,
        macrocosmo::technology::TechnologyPlugin,
        macrocosmo::event_system::EventSystemPlugin,
        macrocosmo::events::EventsPlugin,
        macrocosmo::species::SpeciesPlugin,
        macrocosmo::ship_design::ShipDesignPlugin,
    ));
    app.add_plugins((
        macrocosmo::deep_space::DeepSpacePlugin,
        macrocosmo::setup::GameSetupPlugin,
        macrocosmo::notifications::NotificationsPlugin,
        FactionRelationsPlugin,
        macrocosmo::choice::ChoicesPlugin,
        AiPlugin,
        ObserverPlugin,
    ));
    app
}

/// Collect the (entity, faction id) tuple for every NPC empire — i.e.
/// every Empire entity that is not tagged `PlayerEmpire`.
fn collect_npc_empires(app: &mut App) -> Vec<(Entity, String)> {
    let mut q = app
        .world_mut()
        .query_filtered::<(Entity, &Faction), (With<Empire>, Without<PlayerEmpire>)>();
    q.iter(app.world())
        .map(|(e, f)| (e, f.id.clone()))
        .collect()
}

#[test]
fn npc_empires_spawn_in_player_mode() {
    let mut app = player_mode_app();
    // One Startup pass is enough — every Startup system runs before the
    // first Update tick completes.
    app.update();

    let npcs = collect_npc_empires(&mut app);
    assert!(
        npcs.len() >= 2,
        "expected >= 2 NPC empires to spawn in player mode; found {} ({:?})",
        npcs.len(),
        npcs.iter().map(|(_, id)| id).collect::<Vec<_>>()
    );

    // Sanity: the player empire also exists and is *not* counted above.
    let mut player_q = app
        .world_mut()
        .query_filtered::<&Faction, With<PlayerEmpire>>();
    let player_ids: Vec<String> = player_q.iter(app.world()).map(|f| f.id.clone()).collect();
    assert_eq!(
        player_ids.len(),
        1,
        "expected exactly one PlayerEmpire; found {}",
        player_ids.len()
    );
    assert!(
        !npcs.iter().any(|(_, id)| id == &player_ids[0]),
        "PlayerEmpire id '{}' must not also appear as an NPC empire",
        player_ids[0]
    );
}

#[test]
fn player_mode_ticks_without_panic() {
    let mut app = player_mode_app();
    // 20 ticks covers the `AiTickSet::Reason` schedule (where
    // `npc_decision_tick` runs), diplomatic-action ticking, and other
    // delta-based systems. If NPC spawn wiring breaks any of them (e.g.
    // a query conflict, or Empire entities missing a component expected
    // by a delta system), this panics. A longer horizon would exhaust
    // the Lua auxiliary stack in `evaluate_fire_conditions` (a separate
    // pre-existing issue unrelated to #173) so we keep it short.
    for t in 1..=20 {
        app.world_mut().resource_mut::<GameClock>().elapsed = t;
        app.update();
    }

    // NPC empires should still be present at the end.
    let npcs = collect_npc_empires(&mut app);
    assert!(
        npcs.len() >= 2,
        "NPC empires should persist across ticks; got {}",
        npcs.len()
    );
}

#[test]
fn npc_empires_are_valid_diplomatic_targets() {
    let mut app = player_mode_app();
    app.update();

    let npcs = collect_npc_empires(&mut app);
    assert!(!npcs.is_empty(), "need at least one NPC empire");

    // For each NPC, we should be able to look up a Faction component by
    // entity — that's exactly what the diplomatic-action registry does
    // when resolving `target` to a live empire.
    for (npc_entity, expected_id) in &npcs {
        let faction = app
            .world()
            .get::<Faction>(*npc_entity)
            .expect("NPC empire entity must carry a Faction component");
        assert_eq!(&faction.id, expected_id);
        // Also must have the Empire marker so diplomacy code treats it
        // as a real empire, not a hostile entity-only faction.
        assert!(
            app.world().get::<Empire>(*npc_entity).is_some(),
            "NPC '{}' must have Empire component to be a diplomatic target",
            expected_id
        );
    }
}

#[test]
fn npc_relations_with_hostiles_are_healthy() {
    let mut app = player_mode_app();
    app.update();

    let npcs = collect_npc_empires(&mut app);
    assert!(!npcs.is_empty(), "need at least one NPC empire");

    let hostiles = *app.world().resource::<HostileFactions>();
    let space_creature = hostiles
        .space_creature
        .expect("space_creature hostile faction should be spawned");
    let ancient_defense = hostiles
        .ancient_defense
        .expect("ancient_defense hostile faction should be spawned");

    let relations = app.world().resource::<FactionRelations>();

    for (npc_entity, npc_id) in &npcs {
        // NPC → hostile: must be Neutral + standing=-100 so
        // `can_attack_aggressive()` returns true (negative standing).
        let npc_to_sc = relations.get_or_default(*npc_entity, space_creature);
        assert_eq!(
            npc_to_sc.state,
            RelationState::Neutral,
            "{} → space_creature state should be Neutral",
            npc_id
        );
        assert!(
            npc_to_sc.standing < 0.0,
            "{} → space_creature standing should be negative (got {})",
            npc_id,
            npc_to_sc.standing
        );
        assert!(
            npc_to_sc.can_attack_aggressive(),
            "{} should be willing to attack space_creature under aggressive ROE",
            npc_id
        );

        let npc_to_ad = relations.get_or_default(*npc_entity, ancient_defense);
        assert!(
            npc_to_ad.can_attack_aggressive(),
            "{} should be willing to attack ancient_defense under aggressive ROE",
            npc_id
        );

        // Reverse direction — hostiles → NPC must also be set, so hostile
        // defensive ROE engages the NPC when they share a system.
        let sc_to_npc = relations.get_or_default(space_creature, *npc_entity);
        assert!(
            sc_to_npc.can_attack_aggressive(),
            "space_creature → {} must be hostile (aggressive-ROE attack allowed)",
            npc_id
        );
    }

    // NPC ↔ NPC: seeded as Neutral/standing=0. Not aggressive by default.
    if npcs.len() >= 2 {
        let (a_entity, a_id) = &npcs[0];
        let (b_entity, b_id) = &npcs[1];
        let a_to_b = relations.get_or_default(*a_entity, *b_entity);
        assert_eq!(
            a_to_b.state,
            RelationState::Neutral,
            "{} → {} should be Neutral by default",
            a_id,
            b_id
        );
        assert!(
            !a_to_b.can_attack_aggressive(),
            "{} → {} must not be attackable under aggressive ROE at game start (standing={})",
            a_id,
            b_id,
            a_to_b.standing
        );
    }
}

/// Smoke test: `macrocosmo-ai`'s `mock` feature is wired through the
/// dev-dependency, proving tests can opt into the feature without leaking
/// it into the production binary.
#[test]
fn mock_feature_reachable_via_dev_dependency() {
    // `macrocosmo_ai::mock::preconfigured_bus` is only present when the
    // `mock` feature is enabled. If this line fails to compile, the dev
    // dep in `macrocosmo/Cargo.toml` has dropped the feature.
    let bus = macrocosmo_ai::mock::preconfigured_bus();
    assert!(bus.has_metric(&macrocosmo_ai::mock::metric_ids::fleet_readiness()));
}
