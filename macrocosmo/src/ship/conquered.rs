//! #298 (S-4): Conquered Core state mechanic.
//!
//! When an Infrastructure Core's hull HP drops to 1.0 via combat, it enters
//! the **conquered lock** state. The Core becomes indestructible (HP clamped
//! at 1.0) and a [`ConqueredCore`] component is attached recording the
//! attacker faction.
//!
//! Recovery depends on diplomatic context:
//! - **Wartime hold**: if the attacker is at war with the Core's owner,
//!   HP stays locked at 1.0 until the war ends.
//! - **Peacetime recovery**: if the attacker is not at war, recovery begins
//!   once the attacker's fleet has completely left the system. Recovery rate
//!   is governed by `GameBalance::core_recovery_rate_per_hexadies`.
//! - When recovery restores hull to `hull_max`, the `ConqueredCore` marker
//!   is removed and normal operation resumes.
//!
//! Attacking a Core during peacetime emits a `CasusBelli` game event (the
//! actual auto-war logic is deferred to S-11).

use bevy::prelude::*;

use crate::events::{GameEvent, GameEventKind};
use crate::faction::{FactionOwner, FactionRelations};
use crate::galaxy::AtSystem;
use crate::knowledge::FactSysParam;
use crate::time_system::GameClock;

use super::{CoreShip, Owner, Ship, ShipHitpoints, ShipState};

/// Marker component attached to a Core ship that has been conquered.
///
/// Presence of this component means:
/// - Hull HP is locked at 1.0 (no further damage)
/// - Normal ship repair (`tick_ship_repair`) is skipped
/// - Recovery ticks only when peacetime + attacker fleet absent
#[derive(Component, Clone, Debug)]
pub struct ConqueredCore {
    /// The faction entity that conquered this Core.
    pub attacker_faction: Entity,
}

/// Transition system: when a CoreShip's hull reaches 1.0, attach
/// [`ConqueredCore`] with the attacker faction from the most recent
/// combat engagement. Runs after `resolve_combat`.
///
/// Detection: any CoreShip with `hull == 1.0` that does NOT already have
/// a `ConqueredCore` is transitioning. The attacker is inferred from the
/// hostile entities present in the same system.
pub fn check_conquered_transition(
    mut commands: Commands,
    clock: Res<GameClock>,
    cores: Query<
        (Entity, &Ship, &ShipHitpoints, &AtSystem),
        (With<CoreShip>, Without<ConqueredCore>),
    >,
    hostiles: Query<(&AtSystem, &FactionOwner), With<crate::galaxy::Hostile>>,
    mut events: MessageWriter<GameEvent>,
    mut fact_sys: FactSysParam,
) {
    for (entity, ship, hp, at_system) in &cores {
        if hp.hull > 1.0 || hp.hull <= 0.0 {
            continue;
        }
        // hull == 1.0 — find the attacker faction present in this system
        let attacker = hostiles
            .iter()
            .filter(|(at, _)| at.0 == at_system.0)
            .map(|(_, fo)| fo.0)
            .next();
        let Some(attacker_faction) = attacker else {
            // No hostile in system — could be residual damage; skip.
            continue;
        };
        commands
            .entity(entity)
            .insert(ConqueredCore { attacker_faction });
        let event_id = fact_sys.allocate_event_id();
        let desc = format!("Infrastructure Core '{}' has been conquered!", ship.name,);
        events.write(GameEvent {
            id: event_id,
            timestamp: clock.elapsed,
            kind: GameEventKind::CoreConquered,
            description: desc,
            related_system: Some(at_system.0),
        });
    }
}

/// Safety clamp: during wartime, ensure conquered Core HP stays at 1.0.
/// This guards against any other system accidentally healing the Core
/// while the war is ongoing.
pub fn enforce_conquered_hp_lock(
    mut cores: Query<(&mut ShipHitpoints, &Ship, &ConqueredCore), With<CoreShip>>,
    relations: Res<FactionRelations>,
) {
    for (mut hp, ship, conquered) in &mut cores {
        let Owner::Empire(owner_faction) = ship.owner else {
            continue;
        };
        let view = relations.get_or_default(owner_faction, conquered.attacker_faction);
        if view.is_at_war() && hp.hull != 1.0 {
            hp.hull = 1.0;
        }
    }
}

/// Peacetime recovery: when the attacker faction is NOT at war with the
/// Core's owner AND no ships belonging to the attacker are present in the
/// system, the Core's hull recovers at `core_recovery_rate_per_hexadies`.
///
/// Once hull reaches `hull_max`, the `ConqueredCore` component is removed.
pub fn tick_conquered_recovery(
    mut commands: Commands,
    clock: Res<GameClock>,
    last_tick: Res<crate::colony::LastProductionTick>,
    mut cores: Query<
        (Entity, &Ship, &mut ShipHitpoints, &AtSystem, &ConqueredCore),
        With<CoreShip>,
    >,
    relations: Res<FactionRelations>,
    ships_q: Query<(&Ship, &ShipState)>,
    hostiles_q: Query<
        (&crate::galaxy::AtSystem, &crate::faction::FactionOwner),
        With<crate::galaxy::Hostile>,
    >,
    balance: Res<crate::technology::GameBalance>,
) {
    let delta = clock.elapsed - last_tick.0;
    if delta <= 0 {
        return;
    }

    let recovery_rate = balance.core_recovery_rate_per_hexadies();

    for (entity, ship, mut hp, at_system, conquered) in &mut cores {
        let Owner::Empire(owner_faction) = ship.owner else {
            continue;
        };

        // Wartime hold: no recovery while at war
        let view = relations.get_or_default(owner_faction, conquered.attacker_faction);
        if view.is_at_war() {
            continue;
        }

        // Check if any attacker fleet ships are still present in the system
        let attacker_ship_present = ships_q.iter().any(|(s, state)| {
            if let Owner::Empire(f) = s.owner {
                if f == conquered.attacker_faction {
                    if let ShipState::InSystem { system } = state {
                        return *system == at_system.0;
                    }
                }
            }
            false
        });
        // Also check hostile entities belonging to the attacker faction
        let attacker_hostile_present = hostiles_q
            .iter()
            .any(|(at, fo)| at.0 == at_system.0 && fo.0 == conquered.attacker_faction);
        if attacker_ship_present || attacker_hostile_present {
            continue;
        }

        // Apply recovery
        let amount = recovery_rate * delta as f64;
        hp.hull = (hp.hull + amount).min(hp.hull_max);

        // Recovery complete — remove the conquered marker
        if (hp.hull - hp.hull_max).abs() < f64::EPSILON {
            commands.entity(entity).remove::<ConqueredCore>();
            info!("Core '{}' has recovered from conquered state", ship.name,);
        }
    }
}
