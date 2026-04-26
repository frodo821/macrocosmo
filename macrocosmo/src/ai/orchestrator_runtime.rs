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
use bevy::prelude::*;

use macrocosmo_ai::{
    CampaignReactiveShort, FactionId, FixedDelayDispatcher, IntentDrivenMidTerm,
    ObjectiveDrivenLongTerm, Orchestrator, OrchestratorConfig, Tick, VictoryCondition,
};

use crate::ai::convert::to_ai_faction;
use crate::ai::decomposition_rules::build_default_registry;
use crate::ai::plugin::AiBusResource;
use crate::player::{Empire, PlayerEmpire};
use crate::scripting::victory_api::parse_ai_victory_condition;
use crate::time_system::GameClock;

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
        })
        // Install game-side decomposition rules so the short layer
        // expands `colonize_system` / `deploy_deliverable` macros
        // into primitive commands the consumer pipeline understands.
        // Without this attach, the short layer falls back to the
        // legacy "emit campaign id verbatim" path and macros reach
        // the consumer undecomposed.
        .with_decomposition(build_default_registry());

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
/// Populated by [`register_demo_orchestrator`] on `OnEnter(NewGame)`.
/// Lookup is `O(1)` via `bevy::platform` `HashMap` (the same
/// `HashMap` flavor used elsewhere in the AI integration layer).
#[derive(Resource, Default, Reflect)]
#[reflect(Resource)]
pub struct OrchestratorRegistry {
    /// `FactionOrchestrator` wraps `Orchestrator<Long, Mid, Short>` from
    /// the engine-agnostic `macrocosmo-ai` crate (no `bevy_reflect`
    /// dependency allowed; see `ai-core-isolation.yml` CI). The
    /// resource itself appears in the BRP type registry, but per-faction
    /// orchestrator state is opaque to reflection.
    #[reflect(ignore)]
    pub by_entity: HashMap<Entity, FactionOrchestrator>,
}

/// One-shot system: arm a single demo orchestrator for the first NPC
/// empire that exists at `OnEnter(GameState::NewGame)`. Logs once.
///
/// This is intentionally minimal: the multi-Mid / multi-Short cluster
/// shape (one Mid per region, one Short per fleet) belongs to a later
/// round. Today we run **one** orchestrator per NPC faction, and the
/// demo only arms the first NPC found so logs stay readable.
pub fn register_demo_orchestrator(
    mut registry: ResMut<OrchestratorRegistry>,
    npc_empires: Query<Entity, (With<Empire>, Without<PlayerEmpire>)>,
) {
    let Some(entity) = npc_empires.iter().next() else {
        info!(target: "ai_orch", "no NPC empire found; skipping orchestrator arm");
        return;
    };
    let fid = to_ai_faction(entity);
    registry
        .by_entity
        .insert(entity, FactionOrchestrator::new_demo(fid));
    info!(target: "ai_orch", "AI orchestrator armed for entity={entity:?} faction={fid:?}");
}

/// Per-tick driver for all registered orchestrators.
///
/// Skips when `GameClock` has not advanced since last call (game is
/// paused / sub-hexadies fraction not crossed). For each registered
/// orchestrator: tick once, push produced commands back onto the bus
/// so the existing `drain_ai_commands` consumer observes them, log
/// activity. The command consumer logs unknown command kinds as
/// `unknown` and falls through — appropriate for the skeleton: the
/// orchestrator's `pursue_metric:*` kinds have no game-side mapping
/// yet, so they are observed-only.
pub fn run_orchestrators(
    mut bus: ResMut<AiBusResource>,
    mut registry: ResMut<OrchestratorRegistry>,
    clock: Res<GameClock>,
    mut last_tick: Local<i64>,
) {
    let now: Tick = clock.elapsed;
    if now <= *last_tick {
        return;
    }
    *last_tick = now;

    for (entity, fo) in registry.by_entity.iter_mut() {
        let out = fo
            .orchestrator
            .tick(&mut bus.0, &mut fo.dispatcher, &fo.victory, None, now);
        for cmd in &out.commands {
            // Per-command observer log — `pursue_metric:*` kinds have
            // no game-side mapping yet, so `drain_ai_commands` will
            // `debug!` "not handled" and ignore them. The info-level
            // log here makes them visible without `RUST_LOG=debug`.
            info!(
                target: "ai_orch_cmd",
                "tick={now} entity={entity:?} cmd_kind={} issuer={:?}",
                cmd.kind.as_str(),
                cmd.issuer,
            );
            bus.0.emit_command(cmd.clone());
        }
        if out.long_fired || out.mid_fired || !out.commands.is_empty() {
            info!(
                target: "ai_orch",
                "tick={now} entity={entity:?} long={} mid={} short={} cmds={} status={:?}",
                out.long_fired,
                out.mid_fired,
                out.short_fired,
                out.commands.len(),
                out.victory_status,
            );
        }
    }
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

    #[test]
    fn new_demo_attaches_decomposition_registry() {
        // F4: the orchestrator must carry a decomposition registry so
        // the short layer can expand `colonize_system` / `deploy_deliverable`
        // macros into primitive commands. Without this attach, the
        // short layer falls back to legacy behavior and the consumer
        // pipeline never sees the decomposed primitives.
        let fo = FactionOrchestrator::new_demo(FactionId(0));
        assert!(
            fo.orchestrator.decomposition.is_some(),
            "FactionOrchestrator::new_demo must install a decomposition registry",
        );
        // Sanity: both expected macros are looked up successfully.
        let reg = fo.orchestrator.decomposition.as_deref().unwrap();
        assert!(
            reg.lookup(&crate::ai::schema::ids::command::colonize_system())
                .is_some(),
            "registry should have colonize_system rule",
        );
        assert!(
            reg.lookup(&crate::ai::schema::ids::command::deploy_deliverable())
                .is_some(),
            "registry should have deploy_deliverable rule",
        );
    }
}
