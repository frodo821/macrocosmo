//! #305 (S-11): Casus Belli system.
//!
//! A **Casus Belli** (CB) is a Lua-defined justification for war. Each CB
//! definition carries:
//!
//! - `evaluate`: a Lua function `(attacker, defender) -> bool` that checks
//!   whether conditions for auto-war are met (e.g. "defender has the
//!   `core_attacked` modifier against attacker").
//! - `auto_war`: when `true` and `evaluate` returns `true`, war is declared
//!   automatically without diplomatic delay.
//! - `demands`: base demands imposed on the loser at war end.
//! - `additional_demand_groups`: optional extra demands the winner may choose.
//! - `end_scenarios`: named ways a war can end, each with its own `available`
//!   Lua function and demand adjustments.
//!
//! Runtime state is tracked in [`ActiveWars`], a flat `Vec<ActiveWar>` resource.

use std::collections::HashMap;

use bevy::prelude::*;

use crate::event_system::{EventSystem, LuaDefinedEventContext};
use crate::events::{GameEvent, GameEventKind};
use crate::faction::FactionRelations;
use crate::knowledge::NextEventId;
use crate::player::{Empire, Faction};
use crate::scripting::ScriptEngine;
use crate::time_system::GameClock;

/// Definition of a Casus Belli loaded from Lua.
#[derive(Debug, Clone, bevy::reflect::Reflect)]
pub struct CasusBelliDefinition {
    /// Unique string id (e.g. `"core_attack"`).
    pub id: String,
    /// Human-readable name.
    pub name: String,
    /// Whether war is declared automatically when `evaluate` returns `true`.
    pub auto_war: bool,
    /// Base demands imposed on the loser.
    pub demands: Vec<DemandSpec>,
    /// Optional groups of additional demands the winner may choose from.
    pub additional_demand_groups: Vec<AdditionalDemandGroup>,
    /// Named end-scenario definitions (e.g. white peace, unconditional surrender).
    pub end_scenarios: Vec<EndScenario>,
}

/// A single demand imposed at war's end.
#[derive(Debug, Clone, bevy::reflect::Reflect)]
pub struct DemandSpec {
    /// Machine-readable demand type (e.g. `"return_cores"`, `"reparations"`).
    pub kind: String,
    /// Arbitrary key-value params consumed by the demand resolver.
    pub params: HashMap<String, String>,
}

/// A group of optional demands the winner may select from.
#[derive(Debug, Clone, bevy::reflect::Reflect)]
pub struct AdditionalDemandGroup {
    pub label: String,
    pub max_picks: u32,
    pub demands: Vec<DemandSpec>,
}

/// A named scenario describing how a war can end.
#[derive(Debug, Clone, bevy::reflect::Reflect)]
pub struct EndScenario {
    /// Machine-readable id (e.g. `"white_peace"`, `"unconditional_surrender"`).
    pub id: String,
    /// Human-readable label.
    pub label: String,
    /// Demand adjustments applied when this scenario is selected.
    /// Empty means demands are dropped (white peace).
    pub demand_adjustments: Vec<DemandSpec>,
}

/// Registry of all CB definitions loaded from Lua at startup.
#[derive(Resource, Default, Debug, Reflect)]
#[reflect(Resource)]
pub struct CasusBelliRegistry {
    pub definitions: HashMap<String, CasusBelliDefinition>,
}

impl CasusBelliRegistry {
    pub fn get(&self, id: &str) -> Option<&CasusBelliDefinition> {
        self.definitions.get(id)
    }
}

/// A currently active war between two factions, justified by a CB.
#[derive(Debug, Clone, bevy::reflect::Reflect)]
pub struct ActiveWar {
    /// The CB that justified this war.
    pub cb_id: String,
    /// The faction that declared war (attacker).
    pub attacker: Entity,
    /// The faction being attacked (defender).
    pub defender: Entity,
    /// Game clock timestamp when the war was declared.
    pub started_at: i64,
}

/// Resource tracking all active wars.
#[derive(Resource, Default, Debug, Reflect)]
#[reflect(Resource)]
pub struct ActiveWars {
    pub wars: Vec<ActiveWar>,
}

impl ActiveWars {
    /// Check if a war already exists between the two factions (in either direction).
    pub fn has_war_between(&self, a: Entity, b: Entity) -> bool {
        self.wars
            .iter()
            .any(|w| (w.attacker == a && w.defender == b) || (w.attacker == b && w.defender == a))
    }

    /// Find wars where `faction` is involved (as attacker or defender).
    pub fn wars_involving(&self, faction: Entity) -> Vec<&ActiveWar> {
        self.wars
            .iter()
            .filter(|w| w.attacker == faction || w.defender == faction)
            .collect()
    }

    /// Remove the war between the two factions (if any). Returns the removed war.
    pub fn remove_war_between(&mut self, a: Entity, b: Entity) -> Option<ActiveWar> {
        if let Some(idx) = self.wars.iter().position(|w| {
            (w.attacker == a && w.defender == b) || (w.attacker == b && w.defender == a)
        }) {
            Some(self.wars.remove(idx))
        } else {
            None
        }
    }
}

// ---------------------------------------------------------------------------
// #305 S-11: Evaluate Casus Belli — auto-war system
// ---------------------------------------------------------------------------

/// Event id for the core_attacked bus event that Lua on() handlers can
/// subscribe to. Fired by [`bridge_casus_belli_to_event_system`].
pub const CORE_ATTACKED_EVENT_ID: &str = "macrocosmo:core_attacked";

/// Bridge system: when the legacy `GameEventKind::CasusBelli` message fires,
/// also queue a `macrocosmo:core_attacked` event into the [`EventSystem`] so
/// Lua `on("macrocosmo:core_attacked", fn)` handlers can react.
pub fn bridge_casus_belli_to_event_system(
    mut reader: MessageReader<GameEvent>,
    mut event_system: ResMut<EventSystem>,
    clock: Res<GameClock>,
) {
    for event in reader.read() {
        if event.kind == GameEventKind::CasusBelli {
            let mut details = HashMap::new();
            details.insert("description".to_string(), event.description.clone());
            if let Some(sys) = event.related_system {
                details.insert("system".to_string(), sys.to_bits().to_string());
            }
            let ctx = LuaDefinedEventContext::new(CORE_ATTACKED_EVENT_ID, details);
            event_system.fire_event_with_payload(None, clock.elapsed, Box::new(ctx));
        }
    }
}

/// Exclusive system that evaluates all registered Casus Belli definitions
/// against every ordered pair of empire factions. For CBs with `auto_war =
/// true` whose Lua `evaluate(attacker_faction_id, defender_faction_id)`
/// returns `true`, war is declared automatically.
///
/// Runs `.after(advance_game_time)` in `Update`.
pub fn evaluate_casus_belli(world: &mut World) {
    // Collect empire entities + faction ids first to avoid borrow issues
    let empires: Vec<(Entity, String)> = {
        let mut q = world.query_filtered::<(Entity, &Faction), With<Empire>>();
        q.iter(world)
            .map(|(e, f)| (e, f.id.clone()))
            .collect::<Vec<_>>()
    };

    if empires.len() < 2 {
        return;
    }

    // Collect CB ids that have auto_war
    let auto_war_cb_ids: Vec<String> = {
        let registry = world.resource::<CasusBelliRegistry>();
        registry
            .definitions
            .values()
            .filter(|cb| cb.auto_war)
            .map(|cb| cb.id.clone())
            .collect()
    };

    if auto_war_cb_ids.is_empty() {
        return;
    }

    // For each CB, for each ordered pair (attacker, defender), check evaluate
    let mut wars_to_declare: Vec<(String, Entity, String, Entity, String)> = Vec::new();

    world.resource_scope::<ScriptEngine, _>(|world, engine| {
        let lua = engine.lua();
        let active_wars = world.resource::<ActiveWars>();

        for cb_id in &auto_war_cb_ids {
            // Look up the evaluate function from the accumulator
            let evaluate_fn = {
                let defs: mlua::Table = match lua.globals().get("_casus_belli_definitions") {
                    Ok(t) => t,
                    Err(_) => continue,
                };
                let mut found = None;
                for pair in defs.pairs::<i64, mlua::Table>() {
                    if let Ok((_, tbl)) = pair {
                        if let Ok(id) = tbl.get::<String>("id") {
                            if id == *cb_id {
                                if let Ok(mlua::Value::Function(f)) = tbl.get("evaluate") {
                                    found = Some(f);
                                }
                                break;
                            }
                        }
                    }
                }
                found
            };

            let Some(evaluate) = evaluate_fn else {
                continue;
            };

            for (attacker_entity, attacker_id) in &empires {
                for (defender_entity, defender_id) in &empires {
                    if attacker_entity == defender_entity {
                        continue;
                    }
                    // Skip if war already exists between these factions
                    if active_wars.has_war_between(*attacker_entity, *defender_entity) {
                        continue;
                    }

                    // Call the Lua evaluate function
                    let result: bool =
                        match evaluate.call::<bool>((attacker_id.as_str(), defender_id.as_str())) {
                            Ok(v) => v,
                            Err(e) => {
                                warn!(
                                    "CB '{}' evaluate({}, {}) error: {e}",
                                    cb_id, attacker_id, defender_id
                                );
                                false
                            }
                        };

                    if result {
                        wars_to_declare.push((
                            cb_id.clone(),
                            *attacker_entity,
                            attacker_id.clone(),
                            *defender_entity,
                            defender_id.clone(),
                        ));
                    }
                }
            }
        }
    });

    // Apply war declarations outside the resource_scope
    for (cb_id, attacker, attacker_id, defender, defender_id) in wars_to_declare {
        let clock_elapsed = world.resource::<GameClock>().elapsed;

        // Check again — another CB may have triggered a war between these two
        if world
            .resource::<ActiveWars>()
            .has_war_between(attacker, defender)
        {
            continue;
        }

        // Declare war in FactionRelations (both directions, immediate)
        {
            let mut relations = world.resource_mut::<FactionRelations>();
            relations.declare_war(attacker, defender);
            relations.declare_war(defender, attacker);
        }

        // Record in ActiveWars
        world.resource_mut::<ActiveWars>().wars.push(ActiveWar {
            cb_id: cb_id.clone(),
            attacker,
            defender,
            started_at: clock_elapsed,
        });

        // Emit WarDeclared GameEvent
        let event_id = world.resource_mut::<NextEventId>().allocate();
        let desc = format!(
            "War declared: {} vs {} (casus belli: {})",
            attacker_id, defender_id, cb_id
        );
        world.write_message(GameEvent {
            id: event_id,
            timestamp: clock_elapsed,
            kind: GameEventKind::WarDeclared,
            description: desc,
            related_system: None,
        });

        info!(
            "Auto-war declared via CB '{}': {} -> {}",
            cb_id, attacker_id, defender_id
        );
    }
}

/// End a war between two factions. Removes the [`ActiveWar`], transitions
/// both sides to Peace in [`FactionRelations`], and emits a
/// [`GameEventKind::WarEnded`] event.
///
/// Returns the removed [`ActiveWar`] if one existed, or `None` otherwise.
pub fn end_war(world: &mut World, a: Entity, b: Entity) -> Option<ActiveWar> {
    let removed = world.resource_mut::<ActiveWars>().remove_war_between(a, b);
    if let Some(ref war) = removed {
        // Make peace both directions
        {
            let mut relations = world.resource_mut::<FactionRelations>();
            relations.make_peace(a, b);
            relations.make_peace(b, a);
        }

        let clock_elapsed = world.resource::<GameClock>().elapsed;
        let event_id = world.resource_mut::<NextEventId>().allocate();
        let desc = format!("War ended (casus belli: {})", war.cb_id);
        world.write_message(GameEvent {
            id: event_id,
            timestamp: clock_elapsed,
            kind: GameEventKind::WarEnded,
            description: desc,
            related_system: None,
        });
    }
    removed
}

/// Query available end scenarios for a war between two factions.
/// Calls each scenario's `available` Lua function if present; returns
/// the ids of scenarios whose `available` returned true (or that have no
/// `available` gate).
pub fn available_end_scenarios(world: &mut World, a: Entity, b: Entity) -> Vec<String> {
    let war = {
        let active_wars = world.resource::<ActiveWars>();
        active_wars
            .wars
            .iter()
            .find(|w| (w.attacker == a && w.defender == b) || (w.attacker == b && w.defender == a))
            .cloned()
    };

    let Some(war) = war else {
        return Vec::new();
    };

    let cb_def = {
        let registry = world.resource::<CasusBelliRegistry>();
        registry.get(&war.cb_id).cloned()
    };

    let Some(cb_def) = cb_def else {
        return Vec::new();
    };

    // Collect faction ids for Lua calls
    let attacker_id = world
        .get::<Faction>(war.attacker)
        .map(|f| f.id.clone())
        .unwrap_or_default();
    let defender_id = world
        .get::<Faction>(war.defender)
        .map(|f| f.id.clone())
        .unwrap_or_default();

    let mut available = Vec::new();

    world.resource_scope::<ScriptEngine, _>(|_world, engine| {
        let lua = engine.lua();
        // Look up end_scenarios from the Lua definition for `available` callbacks
        let defs: mlua::Table = match lua.globals().get("_casus_belli_definitions") {
            Ok(t) => t,
            Err(_) => return,
        };
        let mut lua_scenarios: Option<mlua::Table> = None;
        for pair in defs.pairs::<i64, mlua::Table>() {
            if let Ok((_, tbl)) = pair {
                if let Ok(id) = tbl.get::<String>("id") {
                    if id == war.cb_id {
                        if let Ok(mlua::Value::Table(es)) = tbl.get("end_scenarios") {
                            lua_scenarios = Some(es);
                        }
                        break;
                    }
                }
            }
        }

        for scenario in &cb_def.end_scenarios {
            // Check if this scenario has an `available` Lua function
            let has_available_gate = if let Some(ref scenarios_tbl) = lua_scenarios {
                let mut found_fn = false;
                for pair in scenarios_tbl.pairs::<i64, mlua::Table>() {
                    if let Ok((_, sc_tbl)) = pair {
                        if let Ok(id) = sc_tbl.get::<String>("id") {
                            if id == scenario.id {
                                if let Ok(mlua::Value::Function(f)) = sc_tbl.get("available") {
                                    // Call the available function
                                    match f
                                        .call::<bool>((attacker_id.as_str(), defender_id.as_str()))
                                    {
                                        Ok(true) => {
                                            available.push(scenario.id.clone());
                                        }
                                        Ok(false) => {}
                                        Err(e) => {
                                            warn!(
                                                "End scenario '{}' available() error: {e}",
                                                scenario.id
                                            );
                                        }
                                    }
                                    found_fn = true;
                                }
                                break;
                            }
                        }
                    }
                }
                found_fn
            } else {
                false
            };

            // No available gate means always available
            if !has_available_gate {
                available.push(scenario.id.clone());
            }
        }
    });

    available
}

/// Plugin that registers the CB evaluation systems.
pub struct CasusBelliPlugin;

impl Plugin for CasusBelliPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(
            Update,
            bridge_casus_belli_to_event_system.after(crate::time_system::advance_game_time),
        )
        .add_systems(
            Update,
            evaluate_casus_belli
                .after(crate::time_system::advance_game_time)
                .after(bridge_casus_belli_to_event_system),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::prelude::World;

    #[test]
    fn active_wars_has_war_between() {
        let mut world = World::default();
        let a = world.spawn_empty().id();
        let b = world.spawn_empty().id();
        let c = world.spawn_empty().id();

        let mut wars = ActiveWars::default();
        assert!(!wars.has_war_between(a, b));

        wars.wars.push(ActiveWar {
            cb_id: "test".into(),
            attacker: a,
            defender: b,
            started_at: 0,
        });
        assert!(wars.has_war_between(a, b));
        assert!(wars.has_war_between(b, a)); // symmetric
        assert!(!wars.has_war_between(a, c));
    }

    #[test]
    fn active_wars_remove_war_between() {
        let mut world = World::default();
        let a = world.spawn_empty().id();
        let b = world.spawn_empty().id();

        let mut wars = ActiveWars::default();
        wars.wars.push(ActiveWar {
            cb_id: "test".into(),
            attacker: a,
            defender: b,
            started_at: 0,
        });
        assert!(wars.has_war_between(a, b));
        let removed = wars.remove_war_between(b, a);
        assert!(removed.is_some());
        assert!(!wars.has_war_between(a, b));
    }

    #[test]
    fn active_wars_wars_involving() {
        let mut world = World::default();
        let a = world.spawn_empty().id();
        let b = world.spawn_empty().id();
        let c = world.spawn_empty().id();

        let mut wars = ActiveWars::default();
        wars.wars.push(ActiveWar {
            cb_id: "w1".into(),
            attacker: a,
            defender: b,
            started_at: 0,
        });
        wars.wars.push(ActiveWar {
            cb_id: "w2".into(),
            attacker: c,
            defender: a,
            started_at: 5,
        });
        assert_eq!(wars.wars_involving(a).len(), 2);
        assert_eq!(wars.wars_involving(b).len(), 1);
        assert_eq!(wars.wars_involving(c).len(), 1);
    }
}
