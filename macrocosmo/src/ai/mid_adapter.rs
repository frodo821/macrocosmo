//! Mid-layer game adapter — bridges Bevy world state into the
//! engine-agnostic Mid logic (#448).
//!
//! [`MidGameAdapter`] is the read-only interface
//! [`super::mid_stance::MidStanceAgent`] consumes. After #449 PR2d
//! the Mid layer owns Rules 1 (attack), 3 (colonize), 5a (shipyard),
//! 6 (fleet composition), 7 (retreat), and 8 (fortify); Rules 2
//! (survey) and 5b (slot fill) moved to
//! [`super::short_stance::ShortStanceAgent`] (per-Fleet /
//! per-ColonizedSystem `ShortAgent`). `npc_decision_tick` builds a
//! [`BevyMidGameAdapter`] per `MidAgent` per tick and hands it to
//! [`super::mid_stance::MidStanceAgent::decide`].
//!
//! The identity arbiter ([`arbitrate`]) strips [`Locality`] and
//! returns the inner [`Command`]s; #467 phase 2 replaces it with a
//! real FCFS arbiter.

use bevy::prelude::*;
use macrocosmo_ai::{Command, FactionId, Proposal};

use super::npc_decision::NpcContext;
use crate::ai::convert::to_ai_faction;

/// What the Mid layer can read about the game world. Bevy-agnostic
/// in spirit — the trait stays decoupled from `Query` / `Resource`
/// internals, so future engine-agnostic Mid logic can be tested with
/// stub adapters. [`BevyMidGameAdapter`] is the concrete production
/// impl.
///
/// All read-only. Returning slices keeps the trait allocation-free
/// at the call site for the common "iterate this list" pattern.
pub trait MidGameAdapter {
    /// Faction the adapter is currently scoped to.
    fn faction(&self) -> Entity;

    /// Systems known-hostile from this faction's perspective. Mirrors
    /// `NpcContext.hostile_systems`. Used by Rule 1 (attack target).
    fn hostile_systems(&self) -> &[Entity];

    /// Idle ship entities currently flagged as `is_combat`. Rule 1
    /// fires only when both `hostile_systems` and this list are
    /// non-empty.
    fn idle_combat_ships(&self) -> Vec<Entity>;

    /// `true` when the empire's Ruler is **not** aboard a ship —
    /// i.e. eligible for a `move_ruler` follow-up emit. Mirrors
    /// `NpcContext.ruler_aboard == false && ruler_entity.is_some()`.
    fn ruler_movable(&self) -> bool;

    /// Per-faction `can_build_ships` metric (0.0 / 1.0 today). Rule
    /// 5a fires only when this is below 1.0 — i.e. the empire still
    /// lacks a usable shipyard.
    fn can_build_ships(&self) -> f64;

    /// Per-faction `systems_with_core` metric. Rule 5a's #370 gate:
    /// without a Core deployed somewhere, the build_structure handler
    /// has nowhere to emplace the shipyard, so we keep the policy
    /// silent.
    fn systems_with_core(&self) -> f64;

    /// Per-faction `colony_count` metric. Rule 5a additionally
    /// requires at least one colony — a Core-only empire with no
    /// colony is not yet ready for a shipyard.
    fn colony_count(&self) -> f64;

    // ---- PR2d additions (Rules 3/6/7/8) ----

    /// Surveyed-but-uncolonized systems that already pass the Bug B
    /// filter chain (`!has_hostile`, own Core present, no in-flight
    /// `colonize_system` outbox entry). Mirrors
    /// `NpcContext.colonizable_systems`. Rule 3 input.
    fn colonizable_systems(&self) -> &[Entity];

    /// Idle ship entities flagged as `can_colonize`. Rule 3 zips this
    /// against `colonizable_systems` and emits one `colonize_system`
    /// command per pair.
    fn idle_colonizers(&self) -> Vec<Entity>;

    /// Per-faction `my_fleet_ready` metric (0..=1). Rule 7's retreat
    /// gate fires only when `0.0 < fleet_ready < 0.3` — the lower
    /// bound is intentional: a value of exactly 0.0 means "no fleet
    /// at all" (no metric emitted yet), not "fleet wiped", so
    /// retreat is silent.
    fn fleet_ready_ratio(&self) -> f64;

    /// Per-faction `my_total_ships` metric. Rule 8's gate compares
    /// `total_ships < colony_count * 2`.
    fn total_ships(&self) -> f64;

    /// Per-Rule-6 fleet composition snapshot — counts of survey,
    /// colony-capable, and combat-capable ships across the empire's
    /// owned fleet. Returned as a struct rather than three methods so
    /// the call site can copy a single value (matches the legacy
    /// `let survey_count = … let colony_count_ships = … let
    /// combat_count = …` block).
    fn fleet_composition(&self) -> FleetComposition;

    /// Whether the empire has any unsurveyed systems known to it.
    /// Rule 6's first branch (`build_ship explorer_mk1`) fires only
    /// when `survey_count == 0 && unsurveyed_systems is non-empty` —
    /// the second condition lives here so the trait does not have to
    /// expose the full list.
    fn has_unsurveyed_targets(&self) -> bool;

    /// Whether `colonizable_systems` is non-empty. Used by Rule 6's
    /// second branch (`build_ship colony_ship_mk1`) — equivalent to
    /// `!self.colonizable_systems().is_empty()` but kept as its own
    /// method to match the legacy logic's reading order.
    fn has_colonizable_targets(&self) -> bool;

    // ---- PR3a (Rule 2 survey) / PR3b (Rule 5b slot fill) -----------
    //
    // Removed in #449 PR2d: both rules now live on
    // [`super::short_stance::ShortStanceAgent`] (per-Fleet /
    // per-ColonizedSystem `ShortAgent`). The trait methods that
    // sourced their inputs (`unsurveyed_systems`, `idle_surveyors`,
    // `free_building_slots`, `net_production_energy`,
    // `net_production_food`) are gone — the Short adapter
    // [`super::short_adapter::ShortGameAdapter`] owns them now.

    /// Member systems of this Mid's region. Adapter implementations
    /// filter all per-system / per-ship lists they expose
    /// (`hostile_systems`, `colonizable_systems`, `unsurveyed_systems`,
    /// `idle_*_ships`) by intersection with this slice.
    ///
    /// **Default empty** = "no filter" — preserves the legacy single
    /// empire-wide Mid behavior so existing test stubs (e.g.
    /// `mid_stance::tests::StubAdapter`) keep working without
    /// change. Production [`BevyMidGameAdapter`] always returns the
    /// region's actual member set so PR2c+ multi-region splits
    /// activate cross-region isolation automatically.
    fn member_systems(&self) -> &[Entity] {
        &[]
    }
}

/// Three counts Rule 6 needs to pick the next ship to build.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct FleetComposition {
    pub survey_count: usize,
    pub colony_count: usize,
    pub combat_count: usize,
}

/// Bevy implementation of [`MidGameAdapter`]. Wraps a borrow of the
/// per-tick `NpcContext` and the bus so Rule ports can read game
/// signals without duplicating the ECS scan.
///
/// Lifetimes are explicit because `NpcContext` and `AiBus` are both
/// owned higher up the call stack (in `npc_decision_tick`); the
/// adapter just hands views into them.
pub struct BevyMidGameAdapter<'a> {
    pub faction: Entity,
    pub context: &'a NpcContext,
    pub bus: &'a macrocosmo_ai::AiBus,
    /// Pre-computed `idle_combat` set. Borrowed so the adapter stays
    /// cheap to construct per-empire.
    pub idle_combat: &'a [Entity],
    /// Pre-computed `idle_colonizers` set — `s.is_idle &&
    /// s.can_colonize` filtered over `context.ships`. Owned by the
    /// caller (`npc_decision_tick`) so the adapter does not allocate
    /// per `idle_colonizers()` call.
    pub idle_colonizers: &'a [Entity],
    /// Member systems of the Mid's `Region` (#449 PR2b). All
    /// per-system / per-ship lists exposed through the trait are
    /// intersected with this slice so a Mid sees only the systems in
    /// its region. Pre-region-split (today) every empire has exactly
    /// one region containing every owned system, so the intersect is
    /// a no-op and existing NPC integration tests stay green.
    pub member_systems: &'a [Entity],
}

impl<'a> BevyMidGameAdapter<'a> {
    /// Convenience: faction id derived from the faction entity. Used
    /// by `MidStanceAgent::decide` to look up per-faction metrics on
    /// the bus.
    pub fn faction_id(&self) -> FactionId {
        to_ai_faction(self.faction)
    }
}

impl<'a> MidGameAdapter for BevyMidGameAdapter<'a> {
    fn faction(&self) -> Entity {
        self.faction
    }

    fn hostile_systems(&self) -> &[Entity] {
        &self.context.hostile_systems
    }

    fn idle_combat_ships(&self) -> Vec<Entity> {
        self.idle_combat.to_vec()
    }

    fn ruler_movable(&self) -> bool {
        !self.context.ruler_aboard && self.context.ruler_entity.is_some()
    }

    fn can_build_ships(&self) -> f64 {
        self.bus
            .current(&crate::ai::schema::ids::metric::for_faction(
                "can_build_ships",
                self.faction_id(),
            ))
            .unwrap_or(0.0)
    }

    fn systems_with_core(&self) -> f64 {
        self.bus
            .current(&crate::ai::schema::ids::metric::for_faction(
                "systems_with_core",
                self.faction_id(),
            ))
            .unwrap_or(0.0)
    }

    fn colony_count(&self) -> f64 {
        self.bus
            .current(&crate::ai::schema::ids::metric::for_faction(
                "colony_count",
                self.faction_id(),
            ))
            .unwrap_or(0.0)
    }

    fn colonizable_systems(&self) -> &[Entity] {
        &self.context.colonizable_systems
    }

    fn idle_colonizers(&self) -> Vec<Entity> {
        self.idle_colonizers.to_vec()
    }

    fn fleet_ready_ratio(&self) -> f64 {
        self.bus
            .current(&crate::ai::schema::ids::metric::for_faction(
                "my_fleet_ready",
                self.faction_id(),
            ))
            .unwrap_or(0.0)
    }

    fn total_ships(&self) -> f64 {
        self.bus
            .current(&crate::ai::schema::ids::metric::for_faction(
                "my_total_ships",
                self.faction_id(),
            ))
            .unwrap_or(0.0)
    }

    fn fleet_composition(&self) -> FleetComposition {
        // Three independent passes (rather than one fused pass) is
        // intentional — a future ship that satisfies multiple
        // predicates (e.g. both `can_survey` and `is_combat`) must be
        // counted in every matching bucket.
        let ships = &self.context.ships;
        FleetComposition {
            survey_count: ships.iter().filter(|s| s.can_survey).count(),
            colony_count: ships.iter().filter(|s| s.can_colonize).count(),
            combat_count: ships.iter().filter(|s| s.is_combat).count(),
        }
    }

    fn has_unsurveyed_targets(&self) -> bool {
        !self.context.unsurveyed_systems.is_empty()
    }

    fn has_colonizable_targets(&self) -> bool {
        !self.context.colonizable_systems.is_empty()
    }

    fn member_systems(&self) -> &[Entity] {
        // The other slice methods (`hostile_systems`,
        // `colonizable_systems`, `unsurveyed_systems`, `idle_*`) are
        // already region-scoped — `npc_decision_tick` builds
        // `NpcContext` and the idle ship lists by intersecting with
        // `Region.member_systems` before this adapter is constructed.
        // This accessor exposes the scope itself for diagnostic /
        // reflective use; rules do not need to re-filter.
        self.member_systems
    }
}

/// Identity arbiter — strips [`macrocosmo_ai::Locality`], returns
/// the inner [`Command`]s in the order they were proposed. The
/// single-Mid case has no real conflicts, so every proposal
/// trivially commits. #467 phase 2 replaces this with FCFS
/// arbitration (commitment registry, light-speed-delayed
/// [`macrocosmo_ai::ProposalOutcome`]).
pub fn arbitrate(proposals: Vec<Proposal>) -> Vec<Command> {
    proposals.into_iter().map(|p| p.command).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use macrocosmo_ai::{CommandKindId, FactionId, Locality, SystemRef};

    #[test]
    fn arbitrate_strips_locality_and_preserves_order() {
        let issuer = FactionId(7);
        let cmd_a = Command::new(CommandKindId::from("a"), issuer, 10);
        let cmd_b = Command::new(CommandKindId::from("b"), issuer, 11);
        let cmd_c = Command::new(CommandKindId::from("c"), issuer, 12);

        let proposals = vec![
            Proposal::faction_wide(cmd_a.clone()),
            Proposal::at_system(cmd_b.clone(), SystemRef(42)),
            Proposal {
                command: cmd_c.clone(),
                locality: Locality::FactionWide,
            },
        ];

        let commands = arbitrate(proposals);

        assert_eq!(commands.len(), 3, "arbitrate must preserve every proposal");
        assert_eq!(commands[0], cmd_a, "order must match input order");
        assert_eq!(commands[1], cmd_b);
        assert_eq!(commands[2], cmd_c);
    }
}
