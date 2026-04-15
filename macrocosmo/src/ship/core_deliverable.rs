//! #296 (S-3): Infrastructure Core deliverable lifecycle support.
//!
//! When an `infrastructure_core` deliverable is delivered to a star system, a
//! dedicated immobile [`Ship`] entity is spawned to represent the Core. The
//! Core ship is the sole entity that confers sovereignty on a system under
//! the Phase 2 `system_owner` rule (#295 / #296).
//!
//! This module provides:
//!
//! * [`CoreShip`] — zero-sized marker distinguishing Core ships from ordinary
//!   fleet entities. `faction::system_owner` filters on this marker so that
//!   transient ships (colony ships, couriers, cruisers) never confer
//!   sovereignty.
//! * [`PendingCoreDeploys`] — per-tick queue of deploy tickets produced by
//!   `deliverable_ops::process_deliverable_commands`. Tickets are resolved in
//!   [`resolve_core_deploys`] with deterministic tie-breaking when multiple
//!   deploys target the same system on the same tick.
//! * [`spawn_core_ship_from_deliverable`] — helper that wraps the normal
//!   `spawn_ship` path, additionally attaching [`CoreShip`] and
//!   [`AtSystem`](crate::galaxy::AtSystem) so sovereignty follows the ship.

use bevy::prelude::*;

use crate::components::Position;
use crate::galaxy::{AtSystem, StarSystem};
use crate::ship::{Owner, Ship, spawn_ship};
use crate::ship_design::ShipDesignRegistry;

/// Zero-sized marker distinguishing Core ships (infrastructure_core
/// deployments) from ordinary fleet ships.
///
/// `faction::system_owner` keys off this marker, so ONLY Core ships confer
/// sovereignty on a star system. Combined with the immobile hull definition
/// (`sublight_speed = 0`, `ftl_range = 0`), the marker guarantees that
/// sovereignty is bound to a persistent, non-moving presence.
#[derive(Component, Default, Clone, Copy, Debug)]
pub struct CoreShip;

/// One pending Core deploy request, produced when a ship processes a
/// `DeployDeliverable` command whose target definition carries
/// `spawns_as_ship = Some(_)`.
///
/// The `submitted_at` field is captured from `GameClock.elapsed` at the moment
/// the ticket was created; `resolve_core_deploys` uses it only for logging.
#[derive(Debug, Clone)]
pub struct CoreDeployTicket {
    /// The ship entity that executed the deploy command.
    pub deployer: Entity,
    /// The target star system the Core should be spawned into.
    pub target_system: Entity,
    /// The deploy position in galactic coordinates (inner-orbit offset from
    /// the target system's position).
    pub deploy_pos: [f64; 3],
    /// Faction owning the Core ship. `None` for `Owner::Neutral` — such
    /// deploys self-destruct in `resolve_core_deploys` to preserve the
    /// invariant that every Core ship has a diplomatic identity.
    pub faction_owner: Option<Entity>,
    /// The `Owner` value to plumb through `spawn_ship`.
    pub owner: Owner,
    /// The `ShipDesignDefinition.id` used to spawn the Core ship. Must point
    /// at a design with `sublight_speed = 0` and `ftl_range = 0`.
    pub design_id: String,
    /// Index in the deployer's `Cargo.items` of the consumed deliverable.
    /// Retained for traceability / future logging — cargo removal happens in
    /// `process_deliverable_commands` when the ticket is pushed, not here.
    pub cargo_item_index: usize,
    /// `GameClock.elapsed` hexadies when the ticket was enqueued.
    pub submitted_at: i64,
}

/// Per-tick queue of Core deploy tickets. Drained by [`resolve_core_deploys`]
/// each tick; grouping by `target_system` yields the tie-break set when
/// multiple deploys land on the same system in the same frame.
#[derive(Resource, Default, Debug)]
pub struct PendingCoreDeploys {
    pub tickets: Vec<CoreDeployTicket>,
}

/// Spawn a Core ship at `position` in `system`, attaching [`CoreShip`] and
/// [`AtSystem`] alongside the standard ship components.
///
/// This wraps [`spawn_ship`] so the Core participates in fleet bookkeeping /
/// modifier recompute / save-load like any other ship. The extra components
/// make it findable through the `system_owner` query and keep its position
/// invariant (the immobile hull already blocks movement — `AtSystem` provides
/// the reverse lookup into the containing star system for sovereignty
/// derivation).
pub fn spawn_core_ship_from_deliverable(
    commands: &mut Commands,
    design_id: &str,
    name: String,
    system: Entity,
    position: Position,
    owner: Owner,
    design_registry: &ShipDesignRegistry,
) -> Entity {
    let entity = spawn_ship(
        commands,
        design_id,
        name,
        system,
        position,
        owner,
        design_registry,
    );
    commands.entity(entity).insert((CoreShip, AtSystem(system)));
    entity
}

/// Resolve `PendingCoreDeploys.tickets` into actual Core ships.
///
/// Ordering:
/// 1. Group tickets by `target_system`.
/// 2. If any existing `CoreShip` is already present in that system, DROP all
///    tickets for that system (the deliverable self-destructs, matching the
///    "already has a Core" validation in the plan).
/// 3. If multiple tickets target the same system in the same tick, pick a
///    winner deterministically via `GameRng` and discard the rest.
/// 4. Tickets with `Owner::Neutral` / no `faction_owner` are dropped (Core
///    ships require a diplomatic identity — see [`CoreDeployTicket::owner`]).
/// 5. Spawn the winner via [`spawn_core_ship_from_deliverable`].
///
/// Runs `.after(process_deliverable_commands)` so tickets enqueued this tick
/// resolve in the same frame.
pub fn resolve_core_deploys(
    mut commands: Commands,
    mut pending: ResMut<PendingCoreDeploys>,
    rng: Res<crate::scripting::GameRng>,
    design_registry: Res<ShipDesignRegistry>,
    existing_cores: Query<&AtSystem, With<CoreShip>>,
    star_systems: Query<&StarSystem>,
) {
    use rand::Rng;
    use std::collections::HashMap;

    if pending.tickets.is_empty() {
        return;
    }

    // Existing Core coverage per system — an already-owned system rejects new
    // deploys.
    let mut owned: std::collections::HashSet<Entity> = std::collections::HashSet::new();
    for at in existing_cores.iter() {
        owned.insert(at.0);
    }

    // Group tickets by target_system.
    let mut by_system: HashMap<Entity, Vec<CoreDeployTicket>> = HashMap::new();
    for t in pending.tickets.drain(..) {
        by_system.entry(t.target_system).or_default().push(t);
    }

    for (system, mut group) in by_system.into_iter() {
        // System must still exist.
        if star_systems.get(system).is_err() {
            for t in &group {
                warn!(
                    "Core deploy discarded: target system {:?} no longer exists (ticket from {:?})",
                    system, t.deployer
                );
            }
            continue;
        }
        if owned.contains(&system) {
            for t in &group {
                info!(
                    "Core deploy discarded: system {:?} already has a Core ship (ticket from {:?})",
                    system, t.deployer
                );
            }
            continue;
        }
        // Filter out neutral / owner-less tickets.
        group.retain(|t| t.faction_owner.is_some() && matches!(t.owner, Owner::Empire(_)));
        if group.is_empty() {
            continue;
        }
        let winner = if group.len() == 1 {
            group.remove(0)
        } else {
            let handle = rng.handle();
            let mut guard = handle.lock().unwrap();
            let idx = guard.random_range(0..group.len());
            drop(guard);
            group.swap_remove(idx)
        };
        let name = format!(
            "Infrastructure Core ({:?})",
            winner.faction_owner.unwrap_or(Entity::PLACEHOLDER)
        );
        let pos = Position::from(winner.deploy_pos);
        let entity = spawn_core_ship_from_deliverable(
            &mut commands,
            &winner.design_id,
            name,
            system,
            pos,
            winner.owner,
            &design_registry,
        );
        info!(
            "Spawned Core ship {:?} in system {:?} (deployer {:?}, submitted at {} hd)",
            entity, system, winner.deployer, winner.submitted_at
        );
    }
}

/// #296 (S-3): Minimum sensible `Ship` view needed by callers that only
/// know about `sublight_speed` / `ftl_range`. Kept as a free function so
/// other modules can adopt the same predicate without importing `Ship`.
pub fn ship_is_immobile(ship: &Ship) -> bool {
    ship.sublight_speed <= 0.0 && ship.ftl_range <= 0.0
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ship::{EquippedModule, Owner, Ship};
    use bevy::prelude::Entity;

    fn make_ship(sublight: f64, ftl: f64) -> Ship {
        Ship {
            name: "Test".into(),
            design_id: "test".into(),
            hull_id: "test_hull".into(),
            modules: Vec::<EquippedModule>::new(),
            owner: Owner::Neutral,
            sublight_speed: sublight,
            ftl_range: ftl,
            player_aboard: false,
            home_port: Entity::PLACEHOLDER,
            design_revision: 0,
            fleet: None,
        }
    }

    #[test]
    fn is_immobile_true_when_both_zero() {
        let s = make_ship(0.0, 0.0);
        assert!(s.is_immobile());
        assert!(ship_is_immobile(&s));
    }

    #[test]
    fn is_immobile_false_when_sublight_positive() {
        let s = make_ship(0.5, 0.0);
        assert!(!s.is_immobile());
    }

    #[test]
    fn is_immobile_false_when_ftl_positive() {
        let s = make_ship(0.0, 10.0);
        assert!(!s.is_immobile());
    }

    #[test]
    fn is_immobile_false_when_both_positive() {
        let s = make_ship(0.5, 10.0);
        assert!(!s.is_immobile());
    }

    #[test]
    fn is_immobile_false_for_tiny_positive_values() {
        // Epsilon-boundary: any strictly positive value disables immobility.
        let s = make_ship(1e-12, 0.0);
        assert!(!s.is_immobile());
    }
}
