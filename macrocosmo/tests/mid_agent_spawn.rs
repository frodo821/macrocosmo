//! Integration tests for #449 PR2b: every spawned Empire (player + NPCs)
//! receives a `MidAgent` Component anchored at its initial Region.
//!
//! Mirrors the plugin wiring in `tests/region_spawn.rs` so the real
//! `GameSetupPlugin` flow (now extended with the MidAgent spawn step)
//! is exercised.

use bevy::prelude::*;

use macrocosmo::ai::{AiPlugin, MidAgent};
use macrocosmo::faction::FactionRelationsPlugin;
use macrocosmo::observer::{ObserverMode, ObserverPlugin, RngSeed};
use macrocosmo::player::{Empire, Faction, PlayerEmpire};
use macrocosmo::region::{Region, RegionRegistry};
use macrocosmo::time_system::{GameClock, GameSpeed};

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

fn collect_all_empires(app: &mut App) -> Vec<(Entity, String, bool)> {
    let mut q = app
        .world_mut()
        .query_filtered::<(Entity, &Faction, Option<&PlayerEmpire>), With<Empire>>();
    q.iter(app.world())
        .map(|(e, f, p)| (e, f.id.clone(), p.is_some()))
        .collect()
}

#[test]
fn every_empire_gets_one_mid_agent_anchored_at_its_region() {
    let mut app = player_mode_app();
    app.update();

    let empires = collect_all_empires(&mut app);
    assert!(
        empires.len() >= 2,
        "expected player + at least one NPC empire; got {}",
        empires.len()
    );

    let registry = app
        .world()
        .get_resource::<RegionRegistry>()
        .expect("RegionRegistry should exist after spawn")
        .by_empire
        .clone();

    for (empire_entity, faction_id, _is_player) in &empires {
        let region_entities = registry
            .get(empire_entity)
            .unwrap_or_else(|| panic!("RegionRegistry has no entry for empire '{}'", faction_id));
        assert_eq!(
            region_entities.len(),
            1,
            "empire '{}' should have exactly one initial region",
            faction_id
        );
        let region_entity = region_entities[0];

        // Region.mid_agent must be populated by PR2b.
        let region = app
            .world()
            .get::<Region>(region_entity)
            .expect("Region component on region entity");
        let mid_agent_entity = region.mid_agent.unwrap_or_else(|| {
            panic!(
                "Region.mid_agent must be Some after PR2b for '{}'",
                faction_id
            )
        });

        // The MidAgent Component must exist on that entity, with the
        // right back-reference and a default-constructed
        // MidTermState.
        let mid_agent = app
            .world()
            .get::<MidAgent>(mid_agent_entity)
            .unwrap_or_else(|| {
                panic!(
                    "MidAgent component must exist on Region.mid_agent for '{}'",
                    faction_id
                )
            });
        assert_eq!(
            mid_agent.region, region_entity,
            "MidAgent.region must point at its owning Region for '{}'",
            faction_id
        );
        assert_eq!(
            mid_agent.state.stance,
            macrocosmo_ai::Stance::Consolidating,
            "MidAgent.state.stance must default to Consolidating for '{}'",
            faction_id
        );
        assert!(
            mid_agent.state.active_operations.is_empty(),
            "MidAgent.state.active_operations must default to empty for '{}'",
            faction_id
        );
        assert!(
            mid_agent.state.region_id.is_none(),
            "MidAgent.state.region_id (ai-core string id) must stay None — \
             ECS-side Region entity is the source of truth"
        );
    }

    // Cardinality cross-check: total MidAgent count == number of
    // empires (1 region per empire today).
    let mid_agent_count = {
        let mut q = app.world_mut().query::<&MidAgent>();
        q.iter(app.world()).count()
    };
    assert_eq!(
        mid_agent_count,
        empires.len(),
        "exactly one MidAgent per empire (PR2b: 1 region per empire)"
    );
}

#[test]
fn auto_managed_flag_splits_player_vs_npc() {
    let mut app = player_mode_app();
    app.update();

    let empires = collect_all_empires(&mut app);
    let registry = app
        .world()
        .get_resource::<RegionRegistry>()
        .unwrap()
        .by_empire
        .clone();

    let mut player_seen = false;
    let mut npc_seen = false;
    for (empire_entity, faction_id, is_player) in &empires {
        let region_entity = registry.get(empire_entity).unwrap()[0];
        let mid_agent_entity = app
            .world()
            .get::<Region>(region_entity)
            .unwrap()
            .mid_agent
            .unwrap();
        let mid_agent = app.world().get::<MidAgent>(mid_agent_entity).unwrap();

        if *is_player {
            assert!(
                !mid_agent.auto_managed,
                "player empire '{}' MidAgent.auto_managed must default to false (manual)",
                faction_id
            );
            player_seen = true;
        } else {
            assert!(
                mid_agent.auto_managed,
                "NPC empire '{}' MidAgent.auto_managed must default to true (legacy NPC behavior)",
                faction_id
            );
            npc_seen = true;
        }
    }
    assert!(player_seen, "test setup must include the player empire");
    assert!(npc_seen, "test setup must include at least one NPC empire");
}
