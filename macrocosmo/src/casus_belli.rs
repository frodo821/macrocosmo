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

/// Definition of a Casus Belli loaded from Lua.
#[derive(Debug, Clone)]
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
#[derive(Debug, Clone)]
pub struct DemandSpec {
    /// Machine-readable demand type (e.g. `"return_cores"`, `"reparations"`).
    pub kind: String,
    /// Arbitrary key-value params consumed by the demand resolver.
    pub params: HashMap<String, String>,
}

/// A group of optional demands the winner may select from.
#[derive(Debug, Clone)]
pub struct AdditionalDemandGroup {
    pub label: String,
    pub max_picks: u32,
    pub demands: Vec<DemandSpec>,
}

/// A named scenario describing how a war can end.
#[derive(Debug, Clone)]
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
#[derive(Resource, Default, Debug)]
pub struct CasusBelliRegistry {
    pub definitions: HashMap<String, CasusBelliDefinition>,
}

impl CasusBelliRegistry {
    pub fn get(&self, id: &str) -> Option<&CasusBelliDefinition> {
        self.definitions.get(id)
    }
}

/// A currently active war between two factions, justified by a CB.
#[derive(Debug, Clone)]
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
#[derive(Resource, Default, Debug)]
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
