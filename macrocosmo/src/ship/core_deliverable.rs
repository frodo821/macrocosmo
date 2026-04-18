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
//! * [`handle_core_deploy_requested`] — consumes
//!   [`CoreDeployRequested`](crate::ship::command_events::CoreDeployRequested)
//!   messages emitted by `handle_deploy_deliverable_requested`. Tickets are
//!   resolved with deterministic tie-breaking when multiple deploys target
//!   the same system on the same tick.
//! * [`spawn_core_ship_from_deliverable`] — helper that wraps the normal
//!   `spawn_ship` path, additionally attaching [`CoreShip`] and
//!   [`AtSystem`](crate::galaxy::AtSystem) so sovereignty follows the ship.
//!
//! #334 Phase 2 (Commit 2): the intermediate `PendingCoreDeploys` resource +
//! `CoreDeployTicket` struct were retired in favour of the per-frame
//! `MessageReader<CoreDeployRequested>` stream. Tie-break + already-has-core
//! semantics are unchanged.

use bevy::prelude::*;

use crate::colony::{DEFAULT_SYSTEM_BUILDING_SLOTS, SystemBuildingQueue, SystemBuildings};
use crate::components::Position;
use crate::faction::FactionOwner;
use crate::galaxy::{AtSystem, StarSystem};
use crate::ship::command_events::{
    CommandExecuted, CommandKind, CommandResult, CoreDeployRequested,
};
use crate::ship::{Owner, Ship, spawn_ship};
use crate::ship_design::ShipDesignRegistry;
use crate::time_system::GameClock;

/// Zero-sized marker distinguishing Core ships (infrastructure_core
/// deployments) from ordinary fleet ships.
///
/// `faction::system_owner` keys off this marker, so ONLY Core ships confer
/// sovereignty on a star system. Combined with the immobile hull definition
/// (`sublight_speed = 0`, `ftl_range = 0`), the marker guarantees that
/// sovereignty is bound to a persistent, non-moving presence.
#[derive(Component, Default, Clone, Copy, Debug)]
pub struct CoreShip;

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

/// Resolve incoming [`CoreDeployRequested`] messages into actual Core ships.
///
/// Ordering:
/// 1. Group requests by `target_system`.
/// 2. If any existing `CoreShip` is already present in that system, REJECT
///    all requests for that system (the deliverable self-destructs, matching
///    the "already has a Core" validation in the plan). A terminal
///    `CommandExecuted { result: Rejected }` is emitted for each.
/// 3. If multiple requests target the same system in the same tick, pick a
///    winner deterministically via `GameRng`; the losers receive
///    `CommandExecuted { result: Rejected { reason: "lost tie-break" } }`.
/// 4. Requests with `Owner::Neutral` / no `faction_owner` are rejected (Core
///    ships require a diplomatic identity).
/// 5. Spawn the winner via [`spawn_core_ship_from_deliverable`] and emit a
///    terminal `CommandExecuted { result: Ok }` keyed by `command_id`.
///
/// Runs `.after(handle_deploy_deliverable_requested)` so messages enqueued
/// this tick resolve in the same frame. Replaces the legacy
/// `resolve_core_deploys` + `PendingCoreDeploys` intermediate resource
/// (#334 Phase 2 Commit 2).
#[allow(clippy::too_many_arguments)]
pub fn handle_core_deploy_requested(
    mut commands: Commands,
    clock: Res<GameClock>,
    mut reqs: MessageReader<CoreDeployRequested>,
    mut executed: MessageWriter<CommandExecuted>,
    rng: Res<crate::scripting::GameRng>,
    design_registry: Res<ShipDesignRegistry>,
    existing_cores: Query<&AtSystem, With<CoreShip>>,
    star_systems: Query<&StarSystem>,
    existing_system_buildings: Query<(), With<SystemBuildings>>,
    station_ships: Query<(&Ship, &crate::ship::ShipState)>,
) {
    use rand::Rng;
    use std::collections::HashMap;

    // Drain incoming messages into an owned vec so we can group / mutate
    // freely without fighting the reader borrow.
    let incoming: Vec<CoreDeployRequested> = reqs.read().cloned().collect();
    if incoming.is_empty() {
        return;
    }

    // Existing Core coverage per system — an already-owned system rejects new
    // deploys.
    let mut owned: std::collections::HashSet<Entity> = std::collections::HashSet::new();
    for at in existing_cores.iter() {
        owned.insert(at.0);
    }

    // Group tickets by target_system.
    let mut by_system: HashMap<Entity, Vec<CoreDeployRequested>> = HashMap::new();
    for r in incoming {
        by_system.entry(r.target_system).or_default().push(r);
    }

    for (system, mut group) in by_system.into_iter() {
        // System must still exist.
        if star_systems.get(system).is_err() {
            for r in &group {
                warn!(
                    "Core deploy discarded: target system {:?} no longer exists (deployer {:?})",
                    system, r.deployer
                );
                executed.write(CommandExecuted {
                    command_id: r.command_id,
                    kind: CommandKind::CoreDeploy,
                    ship: r.deployer,
                    result: CommandResult::Rejected {
                        reason: "target system despawned".to_string(),
                    },
                    completed_at: clock.elapsed,
                });
            }
            continue;
        }
        if owned.contains(&system) {
            for r in &group {
                info!(
                    "Core deploy discarded: system {:?} already has a Core ship (deployer {:?})",
                    system, r.deployer
                );
                executed.write(CommandExecuted {
                    command_id: r.command_id,
                    kind: CommandKind::CoreDeploy,
                    ship: r.deployer,
                    result: CommandResult::Rejected {
                        reason: "system already has Core".to_string(),
                    },
                    completed_at: clock.elapsed,
                });
            }
            continue;
        }
        // Separate neutral / owner-less requests from empire-owned ones and
        // emit terminal rejections for the neutrals (Core ships require a
        // diplomatic identity — legacy dropped these silently; we now log
        // Rejected explicitly so CommandLog reflects the outcome).
        let mut retained: Vec<CoreDeployRequested> = Vec::with_capacity(group.len());
        for r in group.drain(..) {
            if r.faction_owner.is_some() && matches!(r.owner, Owner::Empire(_)) {
                retained.push(r);
            } else {
                executed.write(CommandExecuted {
                    command_id: r.command_id,
                    kind: CommandKind::CoreDeploy,
                    ship: r.deployer,
                    result: CommandResult::Rejected {
                        reason: "neutral owner".to_string(),
                    },
                    completed_at: clock.elapsed,
                });
            }
        }
        if retained.is_empty() {
            continue;
        }
        // Tie-break via GameRng when multiple empire-owned requests collide.
        let winner_idx = if retained.len() == 1 {
            0
        } else {
            let handle = rng.handle();
            let mut guard = handle.lock().unwrap();
            let idx = guard.random_range(0..retained.len());
            drop(guard);
            idx
        };
        // Emit Rejected for losers before consuming the winner.
        for (i, r) in retained.iter().enumerate() {
            if i == winner_idx {
                continue;
            }
            info!(
                "Core deploy lost tie-break in system {:?} (deployer {:?})",
                system, r.deployer
            );
            executed.write(CommandExecuted {
                command_id: r.command_id,
                kind: CommandKind::CoreDeploy,
                ship: r.deployer,
                result: CommandResult::Rejected {
                    reason: "lost tie-break".to_string(),
                },
                completed_at: clock.elapsed,
            });
        }
        let winner = retained.swap_remove(winner_idx);
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
        // #370: Attach SystemBuildings + SystemBuildingQueue if the system
        // does not already have them. Core deployment alone is sufficient for
        // system building construction — colony is NOT required.
        if existing_system_buildings.get(system).is_err() {
            commands.entity(system).insert((
                SystemBuildings {
                    slots: vec![None; DEFAULT_SYSTEM_BUILDING_SLOTS],
                },
                SystemBuildingQueue::default(),
            ));
            // Tag the StarSystem with FactionOwner so the administrative owner
            // matches the Core deployer (same pattern as settlement.rs).
            if let Some(faction) = winner.faction_owner {
                commands.entity(system).insert(FactionOwner(faction));
            }
        }
        // #387: Auto-spawn a Shipyard station if none exists in this system.
        if !crate::ship::system_has_station_ship("station_shipyard_v1", system, &station_ships) {
            let shipyard_owner = winner
                .faction_owner
                .map(Owner::Empire)
                .unwrap_or(Owner::Neutral);
            spawn_ship(
                &mut commands,
                "station_shipyard_v1",
                "Shipyard".to_string(),
                system,
                pos,
                shipyard_owner,
                &design_registry,
            );
            info!(
                "Auto-spawned Shipyard station in system {:?} on Core deploy",
                system
            );
        }

        info!(
            "Spawned Core ship {:?} in system {:?} (deployer {:?}, submitted at {} hd)",
            entity, system, winner.deployer, winner.submitted_at
        );
        // #300 (S-6): Create a Defense Fleet and reassign the Core ship
        // from its auto-created single-ship fleet into it. We use
        // `commands.queue` because `spawn_ship` wired the Core into a
        // single-ship fleet via Commands that haven't applied yet — the
        // world closure runs after Commands flush and can read/write
        // the live Fleet/Ship components directly.
        let core_entity = entity;
        commands.queue(move |world: &mut World| {
            use crate::ship::defense_fleet::DefenseFleet;
            use crate::ship::fleet::{Fleet, FleetMembers};

            // 1. Read the old auto-created fleet from the Core ship.
            let old_fleet = world.get::<Ship>(core_entity).and_then(|s| s.fleet);

            // 2. Remove Core from the old fleet's members (prune_empty_fleets
            //    will despawn it if it becomes empty).
            if let Some(old_fleet_entity) = old_fleet {
                if let Some(mut members) = world.get_mut::<FleetMembers>(old_fleet_entity) {
                    members.0.retain(|e| *e != core_entity);
                }
            }

            // 3. Spawn the Defense Fleet entity with the Core as sole member.
            let defense_fleet_entity = world
                .spawn((
                    Fleet {
                        name: "Defense Fleet".to_string(),
                        flagship: Some(core_entity),
                    },
                    FleetMembers(vec![core_entity]),
                    DefenseFleet { system },
                ))
                .id();

            // 4. Update the Core ship's fleet back-pointer.
            if let Some(mut ship) = world.get_mut::<Ship>(core_entity) {
                ship.fleet = Some(defense_fleet_entity);
            }
        });
        executed.write(CommandExecuted {
            command_id: winner.command_id,
            kind: CommandKind::CoreDeploy,
            ship: winner.deployer,
            result: CommandResult::Ok,
            completed_at: clock.elapsed,
        });
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
