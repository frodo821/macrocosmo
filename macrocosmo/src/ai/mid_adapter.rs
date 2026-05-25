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
use crate::amount::Amt;

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

    /// Per-faction `can_build_ships` metric. Numerically equal to
    /// `systems_with_shipyard` (a set count): `0.0` = no shipyard
    /// anywhere, `>= 1.0` = at least one owned shipyard. Rule 5a
    /// fires only when this is below 1.0 — i.e. the empire still
    /// lacks a usable shipyard. For total parallel build throughput
    /// use `total_shipyard_slots` instead (#445 fold-in).
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
    ///
    /// **Hotfix-3 semantics**: counts every empire-owned ship that
    /// currently exists OR is queued in any of the empire's colony
    /// build queues, regardless of `ShipState` (InSystem, SubLight,
    /// InFTL, Surveying, Settling, Refitting, Loitering, Scouting). The
    /// previous semantics (filter on `system.is_some()` inside
    /// `NpcContext.ships`) drove an infinite-build loop for
    /// `explorer_mk1`: once an explorer transitioned to `Surveying`,
    /// `survey_count` dropped to 0 and Rule 6 re-emitted `build_ship`
    /// every Reason tick until the empire ran out of minerals. Queued
    /// orders are folded in so the count rises the moment a build is
    /// pushed, not 30 hexadies later when the ship spawns.
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

    /// #444 hotfix: surveyed but not-yet-owned systems sorted by
    /// region-centroid distance. Rule 3.5 zips this against
    /// [`Self::idle_couriers`] and emits `deploy_deliverable(infra_core,
    /// target)` for each pair so the AI can plant a sovereignty
    /// anchor outside the seed region. Without this, Rule 3
    /// (colonize) starves on a starter empire whose region has
    /// exactly one (already-colonised) capital — no surveyed,
    /// uncolonised, own-Core system exists, so colonizable_systems
    /// stays empty forever.
    ///
    /// **Default empty** = preserves StubAdapter / existing test
    /// behaviour. Production [`BevyMidGameAdapter`] populates this
    /// from `NpcContext.expansion_frontier_systems`, which is built
    /// in `npc_decision_tick` from the empire's KnowledgeStore
    /// minus the existing Core / pending-deploy / region member
    /// sets.
    fn expansion_frontier_systems(&self) -> &[Entity] {
        &[]
    }

    /// #444 hotfix: idle ships eligible to ferry a Core out of the
    /// region (`can_colonize && is_idle` — colony ships moonlight as
    /// couriers until #446 lands a dedicated transport class). Rule
    /// 3.5 consumes one per emitted `deploy_deliverable`; Rule 3
    /// must not double-claim them, which `npc_decision_tick`
    /// enforces by pre-filtering its `idle_colonizers` slice so
    /// the same ship is never offered to both lists in one tick.
    ///
    /// **Default empty** = preserves StubAdapter / existing test
    /// behaviour.
    fn idle_couriers(&self) -> &[Entity] {
        &[]
    }

    /// Hotfix-3 resource gate: `true` when the empire's combined
    /// minerals + energy stockpile (summed across the Mid's
    /// `member_systems`) can afford `design_id`'s build cost RIGHT
    /// NOW. **Soft gate** — ignores future revenue / maintenance
    /// accrual, permits deficit spending as long as the stockpile is
    /// non-zero. Rules 6 (`build_ship`) and 3.5 (`deploy_deliverable
    /// → build_deliverable`) consult this before emitting a
    /// proposal; absent the gate a starving empire re-emits every
    /// Reason tick and the handler-side dedup eventually fills the
    /// build queue with orders that can never make progress because
    /// `tick_production`'s investment step has nothing to draw on.
    ///
    /// **Default `true`** = preserves StubAdapter behaviour. An
    /// unknown `design_id` also returns `true` so the adapter does
    /// not silently suppress a Rule emission on a typo (the handler
    /// will warn-and-skip).
    fn can_afford_design(&self, _design_id: &str) -> bool {
        true
    }

    /// Hotfix-3 resource gate: same soft-stockpile check as
    /// [`Self::can_afford_design`] but keyed on a Building id. Used
    /// by Rule 5a (`build_structure` shipyard) and the Short-side
    /// Rule 5b (slot fill — mine / farm / power_plant) so a
    /// minerals-starved empire stops stacking orders the colony
    /// build queue cannot drain.
    ///
    /// **Default `true`** = preserves StubAdapter behaviour.
    fn can_afford_building(&self, _building_id: &str) -> bool {
        true
    }

    /// Current minerals stockpile (sum across `member_systems`).
    /// Exposed so rule implementations / tests can introspect the
    /// gate decision; production rules consume the boolean
    /// [`Self::can_afford_design`] / [`Self::can_afford_building`]
    /// helpers instead. Default `Amt(u64::MAX)` so StubAdapter
    /// callers never accidentally trigger the gate.
    fn current_minerals(&self) -> Amt {
        Amt(u64::MAX)
    }

    /// Current energy stockpile (sum across `member_systems`). See
    /// [`Self::current_minerals`].
    fn current_energy(&self) -> Amt {
        Amt(u64::MAX)
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
    /// #444 hotfix: surveyed-but-not-owned systems pre-ranked by
    /// region-centroid distance, used by Rule 3.5
    /// (`deploy_deliverable(infra_core)`) to seed new owned-Core
    /// systems outside the current region scope. Pre-computed in
    /// `npc_decision_tick` because the centroid + KnowledgeStore
    /// walk needs the same ECS queries.
    pub expansion_frontier: &'a [Entity],
    /// #444 hotfix: idle colony-capable ships not yet claimed by
    /// Rule 3 (colonize). `npc_decision_tick` pre-partitions
    /// idle_colonizers vs. idle_couriers so the two rules never
    /// double-book the same ship.
    pub idle_couriers: &'a [Entity],
    /// Hotfix-3: pre-computed empire-wide fleet composition.
    /// Includes ships in every `ShipState` variant (in-transit,
    /// surveying, settling, etc.) AND any same-design ships
    /// currently queued in colony `BuildQueue`s, so the count rises
    /// the moment Rule 6 emits a `build_ship` order rather than 30
    /// hexadies later when the ship spawns. Closes the infinite
    /// `explorer_mk1` loop where the surveying-ship's
    /// `system: None` evicted it from `NpcContext.ships` and Rule 6
    /// re-emitted every tick.
    pub fleet_composition: FleetComposition,
    /// Hotfix-3: sum of `ResourceStockpile.minerals` across this
    /// Mid's `member_systems`. Consumed by [`MidGameAdapter::can_afford_design`]
    /// / [`MidGameAdapter::can_afford_building`] gates so a starving
    /// empire stops emitting build orders the colony queue cannot
    /// drain. Soft gate: zero stockpile blocks emission, deficit
    /// spending (`stockpile > 0 && revenue < expense`) is permitted.
    pub current_minerals: Amt,
    /// Hotfix-3: sum of `ResourceStockpile.energy`. See
    /// [`Self::current_minerals`].
    pub current_energy: Amt,
    /// Hotfix-3: design registry borrow used by
    /// [`MidGameAdapter::can_afford_design`] to look up
    /// `build_cost_minerals` / `build_cost_energy`. `None` only in
    /// test setups that never load the Lua registry; an unknown
    /// `design_id` returns `true` from the gate so a typo does not
    /// silently suppress an emission (the handler will warn).
    pub design_registry: Option<&'a crate::ship_design::ShipDesignRegistry>,
    /// Hotfix-3: building registry borrow used by
    /// [`MidGameAdapter::can_afford_building`].
    pub building_registry: Option<&'a crate::colony::BuildingRegistry>,
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
        // Hotfix-3: return the empire-wide census pre-computed in
        // `npc_decision_tick` (covers every `ShipState` variant +
        // BuildQueue-queued ships). Pre-hotfix this iterated
        // `NpcContext.ships`, which is filtered on
        // `system.is_some()` — surveying / in-transit ships were
        // silently evicted, causing Rule 6 to re-emit `build_ship
        // explorer_mk1` every tick until the empire starved.
        self.fleet_composition
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

    fn expansion_frontier_systems(&self) -> &[Entity] {
        self.expansion_frontier
    }

    fn idle_couriers(&self) -> &[Entity] {
        self.idle_couriers
    }

    fn can_afford_design(&self, design_id: &str) -> bool {
        // Unknown design → permissive: avoid silently suppressing
        // a Rule emission on a typo; the handler will warn.
        let Some(registry) = self.design_registry else {
            return true;
        };
        let Some(def) = registry.get(design_id) else {
            return true;
        };
        // Soft gate: each resource must be individually fundable
        // out of the current stockpile. Cost == 0 trivially passes
        // (Amt comparison is on raw u64).
        self.current_minerals >= def.build_cost_minerals
            && self.current_energy >= def.build_cost_energy
    }

    fn can_afford_building(&self, building_id: &str) -> bool {
        let Some(registry) = self.building_registry else {
            return true;
        };
        let Some(def) = registry.get(building_id) else {
            return true;
        };
        self.current_minerals >= def.minerals_cost && self.current_energy >= def.energy_cost
    }

    fn current_minerals(&self) -> Amt {
        self.current_minerals
    }

    fn current_energy(&self) -> Amt {
        self.current_energy
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
