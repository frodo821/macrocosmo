use bevy::prelude::*;

use crate::ai::command_handlers::find_empire_entity;
use crate::components::Position;
use crate::galaxy::{AtSystem, Hostile, Sovereignty, StarSystem};
use crate::physics::distance_ly;
use crate::player::{Empire, Faction};
use crate::ship::command_events::{MoveRequested, NextCommandId};
use crate::ship::{CommandQueue, Owner, Ship, ShipState};

/// Handle `retreat`: find ships in systems with hostiles and send them
/// back to the faction's home system (system with most colonies).
pub(crate) fn handle_retreat(
    issuer: &macrocosmo_ai::FactionId,
    ships: &Query<(Entity, &Ship, &ShipState, &CommandQueue)>,
    hostiles: &Query<&AtSystem, With<Hostile>>,
    sovereignty: &Query<(Entity, &Sovereignty), With<StarSystem>>,
    empires: &Query<(Entity, &Faction), With<Empire>>,
    positions: &Query<&Position>,
    move_writer: &mut MessageWriter<MoveRequested>,
    next_cmd_id: &mut NextCommandId,
    now: i64,
) {
    let empire_entity = match find_empire_entity(issuer, empires) {
        Some(e) => e,
        None => return,
    };

    let owned_systems: Vec<Entity> = sovereignty
        .iter()
        .filter(|(_, sov)| sov.owner == Some(Owner::Empire(empire_entity)))
        .map(|(e, _)| e)
        .collect();

    if owned_systems.is_empty() {
        debug!("retreat: faction {:?} has no sovereign systems", issuer);
        return;
    }

    let hostile_set: std::collections::HashSet<Entity> = hostiles.iter().map(|at| at.0).collect();
    let safe_systems: Vec<Entity> = owned_systems
        .iter()
        .filter(|s| !hostile_set.contains(s))
        .copied()
        .collect();
    let rally_candidates = if safe_systems.is_empty() {
        &owned_systems
    } else {
        &safe_systems
    };

    let mut retreated = 0;
    for (ship_entity, ship, state, queue) in ships.iter() {
        if ship.owner != Owner::Empire(empire_entity) {
            continue;
        }
        if ship.is_immobile() {
            continue;
        }
        let ShipState::InSystem { system } = state else {
            continue;
        };
        if !hostile_set.contains(system) || !queue.commands.is_empty() {
            continue;
        }
        if !safe_systems.is_empty() && safe_systems.contains(system) {
            continue;
        }

        let filtered: Vec<Entity> = rally_candidates
            .iter()
            .filter(|s| **s != *system)
            .copied()
            .collect();
        if filtered.is_empty() {
            continue;
        }

        let target = pick_nearest_system(*system, &filtered, positions);
        move_writer.write(MoveRequested {
            command_id: next_cmd_id.allocate(),
            ship: ship_entity,
            target,
            issued_at: now,
        });
        retreated += 1;
    }

    if retreated > 0 {
        info!(
            "retreat: {} ships from faction {:?} retreating to rally points",
            retreated, issuer
        );
    }
}

fn pick_nearest_system(
    origin: Entity,
    candidates: &[Entity],
    positions: &Query<&Position>,
) -> Entity {
    let origin_pos = positions.get(origin).ok();
    let mut best = candidates[0];
    let mut best_dist = f64::MAX;
    for &candidate in candidates {
        let dist = match (origin_pos, positions.get(candidate).ok()) {
            (Some(a), Some(b)) => distance_ly(a, b),
            _ => f64::MAX,
        };
        if dist < best_dist {
            best_dist = dist;
            best = candidate;
        }
    }
    best
}
