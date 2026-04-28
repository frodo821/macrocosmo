//! Integration tests for #449 PR2a: every spawned Empire (player + NPCs)
//! receives an initial `Region` whose capital and only member is the
//! empire's `HomeSystem`, the home StarSystem carries a `RegionMembership`
//! pointing at that Region, and the `RegionRegistry` resource is updated.
//!
//! Mirrors the plugin wiring in `tests/npc_empires_in_player_mode.rs` so
//! the real `GameSetupPlugin` flow (including the new
//! `spawn_initial_region_for_faction` hook) is exercised.

use bevy::prelude::*;

use macrocosmo::ai::AiPlugin;
use macrocosmo::faction::FactionRelationsPlugin;
use macrocosmo::galaxy::HomeSystem;
use macrocosmo::observer::{ObserverMode, ObserverPlugin, RngSeed};
use macrocosmo::player::{Empire, Faction, PlayerEmpire};
use macrocosmo::region::{EmpireLongTermState, Region, RegionMembership, RegionRegistry};
use macrocosmo::time_system::{GameClock, GameSpeed};

/// Build a player-mode app that runs the real GameSetupPlugin pipeline so
/// player + NPC empires both spawn and the new Region wiring fires.
fn player_mode_app() -> App {
    let mut app = App::new();
    app.add_plugins(MinimalPlugins);
    app.add_plugins(bevy::input::InputPlugin);

    app.insert_resource(ObserverMode {
        enabled: false,
        ..Default::default()
    });
    app.insert_resource(RngSeed(Some(0xC0FFEE)));
    app.insert_resource(GameClock::new(0));
    app.insert_resource(GameSpeed::default());

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

/// (entity, faction id) for every Empire (player + NPCs).
fn collect_all_empires(app: &mut App) -> Vec<(Entity, String, bool)> {
    let mut q = app
        .world_mut()
        .query_filtered::<(Entity, &Faction, Option<&PlayerEmpire>), With<Empire>>();
    q.iter(app.world())
        .map(|(e, f, p)| (e, f.id.clone(), p.is_some()))
        .collect()
}

#[test]
fn every_empire_gets_initial_region_anchored_at_home_system() {
    let mut app = player_mode_app();
    app.update();

    let empires = collect_all_empires(&mut app);
    assert!(
        empires.len() >= 2,
        "expected player + at least one NPC empire; got {}",
        empires.len()
    );

    // The registry resource must exist after empire spawn.
    let registry = app
        .world()
        .get_resource::<RegionRegistry>()
        .expect("RegionRegistry resource should be created during empire spawn");

    // Every empire must have exactly one Region in the registry, pointing
    // at a real Region entity, with capital_system == HomeSystem and
    // member_systems == [capital_system].
    for (empire_entity, faction_id, is_player) in &empires {
        let home = app
            .world()
            .get::<HomeSystem>(*empire_entity)
            .map(|h| h.0)
            .unwrap_or_else(|| {
                panic!(
                    "empire '{}' (player={}) has no HomeSystem — Region spawn precondition not met",
                    faction_id, is_player
                )
            });

        let region_entities = registry
            .by_empire
            .get(empire_entity)
            .unwrap_or_else(|| panic!("RegionRegistry has no entry for empire '{}'", faction_id));
        assert_eq!(
            region_entities.len(),
            1,
            "empire '{}' should have exactly one initial region; got {}",
            faction_id,
            region_entities.len()
        );
        let region_entity = region_entities[0];

        let region = app.world().get::<Region>(region_entity).unwrap_or_else(|| {
            panic!(
                "region entity for '{}' missing Region component",
                faction_id
            )
        });
        assert_eq!(region.empire, *empire_entity);
        assert_eq!(region.capital_system, home);
        assert_eq!(
            region.member_systems,
            vec![home],
            "empire '{}': initial region member_systems should be [home_system]",
            faction_id
        );
        assert!(
            region.mid_agent.is_none(),
            "empire '{}': mid_agent slot should remain None until #449 PR2b",
            faction_id
        );

        // Reverse index on the StarSystem entity.
        let membership = app
            .world()
            .get::<RegionMembership>(home)
            .unwrap_or_else(|| {
                panic!(
                    "home system of '{}' should carry RegionMembership pointing at its Region",
                    faction_id
                )
            });
        assert_eq!(
            membership.region, region_entity,
            "RegionMembership on home system must point at the empire's Region"
        );
    }
}

#[test]
fn every_empire_carries_empire_long_term_state_component() {
    let mut app = player_mode_app();
    app.update();

    let empires = collect_all_empires(&mut app);
    assert!(!empires.is_empty(), "expected at least one Empire");

    // PR1's `OrchestratorState.long_state: LongTermState` was migrated to
    // an Empire Component in PR2a. Every empire (player + NPC) must carry
    // it.
    for (empire_entity, faction_id, _) in &empires {
        let state = app.world().get::<EmpireLongTermState>(*empire_entity);
        assert!(
            state.is_some(),
            "empire '{}' must have EmpireLongTermState component (state-on-Component migration)",
            faction_id
        );
        let state = state.unwrap();
        // Default-constructed: all collections empty.
        assert!(state.inner.pursued_metrics.is_empty());
        assert!(state.inner.victory_progress.is_empty());
        assert!(state.inner.current_campaign_phase.is_none());
    }
}
