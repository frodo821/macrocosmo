//! Per-faction `macrocosmo-ai` three-layer orchestrator skeleton +
//! registry resource.
//!
//! Step 1 of the macrocosmo-ai → macrocosmo game-integration cut-in.
//! This module owns *the data*: a [`FactionOrchestrator`] bundle (one
//! orchestrator + one dispatcher + one victory condition per faction)
//! and an [`OrchestratorRegistry`] Bevy `Resource` keyed by `Entity`.
//!
//! **Plugin / system wiring is intentionally NOT done here** — the
//! next round (Steps 2-7) will register systems that drive
//! [`Orchestrator::tick`], spawn `FactionOrchestrator` instances when
//! NPC factions come online, and route emitted [`Command`]s through
//! the existing AI bus / command consumer pipeline.
//!
//! The constructor [`FactionOrchestrator::new_demo`] builds its
//! [`VictoryCondition`] **via the Step 0 Lua parser**
//! ([`crate::scripting::victory_api::parse_ai_victory_condition`])
//! so the demo VictoryCondition format stays exercised end-to-end as
//! the parser evolves.

use bevy::platform::collections::HashMap;
use bevy::prelude::{Entity, Resource};

use macrocosmo_ai::{
    CampaignReactiveShort, FactionId, FixedDelayDispatcher, IntentDrivenMidTerm,
    ObjectiveDrivenLongTerm, Orchestrator, OrchestratorConfig, VictoryCondition,
};

use crate::scripting::victory_api::parse_ai_victory_condition;

/// Per-faction bundle: one three-layer [`Orchestrator`], one
/// [`FixedDelayDispatcher`], and the faction's [`VictoryCondition`].
///
/// Held inside [`OrchestratorRegistry`], keyed by the Bevy `Entity`
/// of the faction's empire / `Ruler` entity (the integration layer
/// owns the entity-mapping decision and will populate the map as
/// factions come online).
pub struct FactionOrchestrator {
    pub orchestrator:
        Orchestrator<ObjectiveDrivenLongTerm, IntentDrivenMidTerm, CampaignReactiveShort>,
    pub dispatcher: FixedDelayDispatcher,
    pub victory: VictoryCondition,
}

impl FactionOrchestrator {
    /// Build a demo `FactionOrchestrator` for `faction`.
    ///
    /// Cadences are deliberately quick (`long_cadence = 5`,
    /// `mid_cadence = 2`) so short playthroughs show visible Long /
    /// Mid activity. The dispatcher uses a symbolic `2`-tick (= 2
    /// hexadies) courier delay.
    ///
    /// The [`VictoryCondition`] is built **through the Step 0 Lua
    /// parser** so the demo continually exercises the parser surface:
    /// "first faction member to push `colony_count.faction_<n>` above
    /// `1.0`". This is intentionally easy to satisfy in playtests.
    pub fn new_demo(faction: FactionId) -> Self {
        let lua = mlua::Lua::new();
        let victory = build_demo_victory(&lua, faction)
            .expect("demo VictoryCondition must parse — parser regression?");

        let orchestrator = Orchestrator::new(
            faction,
            ObjectiveDrivenLongTerm::new(),
            IntentDrivenMidTerm::new(),
            CampaignReactiveShort::new(),
        )
        .with_config(OrchestratorConfig {
            long_cadence: 5,
            mid_cadence: 2,
            ..Default::default()
        });

        // 2-hexadies courier delay — tiny, but non-zero so the
        // demo exercises the in-flight intent queue.
        let dispatcher = FixedDelayDispatcher::new(2);

        Self {
            orchestrator,
            dispatcher,
            victory,
        }
    }
}

/// Build the demo [`VictoryCondition`] table in Lua, then parse it
/// via the Step 0 parser. Encapsulated so test asserts can re-run the
/// same construction path.
fn build_demo_victory(lua: &mlua::Lua, faction: FactionId) -> mlua::Result<VictoryCondition> {
    let metric_name = format!("colony_count.faction_{}", faction.0);

    let win = lua.create_table()?;
    win.set("kind", "metric_above")?;
    win.set("metric", metric_name)?;
    win.set("threshold", 1.0_f64)?;

    // `prerequisites` defaults to vacuous-true when omitted; we still
    // pass an explicit empty `all` table so the demo doubles as a
    // smoke test of the explicit-empty path.
    let prereq = lua.create_table()?;
    prereq.set("kind", "all")?;
    prereq.set("children", lua.create_table()?)?;

    let outer = lua.create_table()?;
    outer.set("win", win)?;
    outer.set("prerequisites", prereq)?;

    parse_ai_victory_condition(lua, outer)
}

/// Bevy `Resource` mapping each faction `Entity` to its
/// [`FactionOrchestrator`].
///
/// Empty by default; populated by Step 2's plugin-wiring system as
/// NPC factions are spawned. Lookup is `O(1)` via `bevy::platform`
/// `HashMap` (the same `HashMap` flavor used elsewhere in the AI
/// integration layer).
#[derive(Resource, Default)]
pub struct OrchestratorRegistry {
    pub by_entity: HashMap<Entity, FactionOrchestrator>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use macrocosmo_ai::{Condition, ConditionAtom, MetricId};

    #[test]
    fn new_demo_builds_metric_above_for_faction() {
        let fo = FactionOrchestrator::new_demo(FactionId(0));

        // Victory: MetricAbove on `colony_count.faction_0` at threshold 1.0.
        match &fo.victory.win {
            Condition::Atom(ConditionAtom::MetricAbove { metric, threshold }) => {
                assert_eq!(metric, &MetricId::from("colony_count.faction_0"));
                assert_eq!(*threshold, 1.0);
            }
            other => panic!("expected MetricAbove leaf, got {other:?}"),
        }
        // Prerequisites: explicit-empty `all` (= vacuous true).
        assert_eq!(fo.victory.prerequisites, Condition::All(Vec::new()));
        // No time limit on the demo.
        assert_eq!(fo.victory.time_limit, None);

        // Cadence config wired through.
        assert_eq!(fo.orchestrator.config.long_cadence, 5);
        assert_eq!(fo.orchestrator.config.mid_cadence, 2);

        // Dispatcher delay symbolic 2.
        assert_eq!(fo.dispatcher.delay, 2);

        // Faction id propagated to the orchestrator.
        assert_eq!(fo.orchestrator.faction, FactionId(0));
    }

    #[test]
    fn new_demo_metric_id_tracks_faction_number() {
        // Different faction ids produce different metric names so the
        // per-faction VictoryCondition does not collide.
        let f7 = FactionOrchestrator::new_demo(FactionId(7));
        match &f7.victory.win {
            Condition::Atom(ConditionAtom::MetricAbove { metric, .. }) => {
                assert_eq!(metric, &MetricId::from("colony_count.faction_7"));
            }
            other => panic!("expected MetricAbove leaf, got {other:?}"),
        }
        assert_eq!(f7.orchestrator.faction, FactionId(7));
    }

    #[test]
    fn registry_default_is_empty() {
        let reg = OrchestratorRegistry::default();
        assert!(reg.by_entity.is_empty());
        assert_eq!(reg.by_entity.len(), 0);
    }
}
