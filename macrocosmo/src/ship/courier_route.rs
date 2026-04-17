//! #117: Automated courier routes.
//!
//! A `CourierRoute` lets a courier ship follow a fixed sequence of waypoints
//! (star systems) without manual intervention. At each waypoint the courier
//! performs a mode-specific pickup/deliver action, then automatically queues
//! a `MoveTo` command for the next waypoint.
//!
//! Three transport modes are supported:
//!  * `KnowledgeRelay` — copies the local `KnowledgeStore` into a cargo
//!    component on pickup and merges it into the destination's
//!    `KnowledgeStore` on delivery (newer `observed_at` wins). This bypasses
//!    light-speed delay for empire knowledge.
//!  * `ResourceTransport` — loads minerals and energy from the system
//!    stockpile (up to the `Cargo` capacity at a fixed default of 500 each)
//!    and unloads them at every subsequent waypoint.
//!  * `MessageDelivery` — placeholder for command delivery; logs but does
//!    not yet implement transport. (Optional in #117; can be expanded later.)
//!
//! Routes are opt-in: the existing manual `CommandQueue` is unaffected.

use bevy::prelude::*;

use crate::amount::Amt;
use crate::colony::ResourceStockpile;
use crate::components::Position;
use crate::galaxy::StarSystem;
use crate::knowledge::{KnowledgeStore, SystemKnowledge};
use crate::time_system::GameClock;

use super::{Cargo, CommandQueue, QueuedCommand, Ship, ShipState};

/// Default cargo capacity (in `Amt` units) for couriers operating in
/// `ResourceTransport` mode. Used per-resource (minerals, energy).
pub const COURIER_DEFAULT_CARGO_CAPACITY: Amt = Amt::units(500);

/// Behaviour selector for an automated courier route.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CourierMode {
    /// Carry knowledge snapshots between waypoints.
    KnowledgeRelay,
    /// Carry minerals/energy between waypoints.
    ResourceTransport,
    /// Carry pending commands faster than light-speed delay.
    /// (Currently a stub — the courier still travels but no commands are
    /// physically transported. Intended for a follow-up issue.)
    MessageDelivery,
}

impl CourierMode {
    pub fn label(&self) -> &'static str {
        match self {
            CourierMode::KnowledgeRelay => "Knowledge Relay",
            CourierMode::ResourceTransport => "Resource Transport",
            CourierMode::MessageDelivery => "Message Delivery",
        }
    }
}

/// A repeating waypoint route attached to a courier ship.
///
/// `current_index` points at the next waypoint the courier should arrive
/// at. When a courier docks at `waypoints[current_index]`, the system
/// performs the pickup/deliver action for that stop and advances the
/// index.
#[derive(Component, Clone, Debug)]
pub struct CourierRoute {
    pub waypoints: Vec<Entity>,
    pub current_index: usize,
    pub mode: CourierMode,
    pub repeat: bool,
    /// When true, the route is paused: dispatch logic skips this courier.
    pub paused: bool,
}

impl CourierRoute {
    pub fn new(waypoints: Vec<Entity>, mode: CourierMode) -> Self {
        Self {
            waypoints,
            current_index: 0,
            mode,
            repeat: true,
            paused: false,
        }
    }

    /// True when the route has been fully traversed and won't repeat.
    pub fn is_finished(&self) -> bool {
        !self.repeat && self.current_index >= self.waypoints.len()
    }

    /// Advance the index, wrapping when `repeat` is true.
    pub fn advance(&mut self) {
        self.current_index += 1;
        if self.repeat && self.current_index >= self.waypoints.len() {
            self.current_index = 0;
        }
    }
}

/// Knowledge snapshots carried by a courier between docks.
#[derive(Component, Default, Clone, Debug)]
pub struct CourierKnowledgeCargo {
    pub entries: Vec<SystemKnowledge>,
}

impl CourierKnowledgeCargo {
    pub fn clear(&mut self) {
        self.entries.clear();
    }
}

/// Drive courier ships along their routes.
///
/// Run after movement systems so docked-state transitions from the same
/// frame are visible.
#[allow(clippy::too_many_arguments)]
pub fn tick_courier_routes(
    clock: Res<GameClock>,
    systems_q: Query<(Entity, &Position), With<StarSystem>>,
    mut empire_q: Query<&mut KnowledgeStore, With<crate::player::PlayerEmpire>>,
    mut stockpiles_q: Query<&mut ResourceStockpile, With<StarSystem>>,
    mut couriers_q: Query<(
        Entity,
        &Ship,
        &ShipState,
        &mut CommandQueue,
        &mut CourierRoute,
        Option<&mut Cargo>,
        Option<&mut CourierKnowledgeCargo>,
    )>,
    mut commands: Commands,
) {
    // Build a small position lookup for queue prediction updates.
    let position_of = |entity: Entity| -> Option<[f64; 3]> {
        systems_q.get(entity).ok().map(|(_, pos)| pos.as_array())
    };

    // The empire knowledge store doubles as the "system local" store for
    // KnowledgeRelay in the current single-empire model. In future, each
    // system could have its own store; this query lookup is a single
    // point to swap out.
    let mut empire_store_opt = empire_q.iter_mut().next();

    for (entity, ship, state, mut queue, mut route, cargo, mut knowledge_cargo) in
        couriers_q.iter_mut()
    {
        if route.paused || route.is_finished() || route.waypoints.is_empty() {
            continue;
        }

        // Only act when docked and queue is empty (i.e., previous waypoint
        // command finished). If the queue is non-empty the courier is on
        // its way somewhere — let the standard movement systems work.
        let docked_system = match state {
            ShipState::InSystem { system } => *system,
            _ => continue,
        };
        if !queue.commands.is_empty() {
            continue;
        }

        let target = match route.waypoints.get(route.current_index).copied() {
            Some(t) => t,
            None => continue,
        };

        if docked_system != target {
            // Not at the next waypoint — push a MoveTo so the existing
            // command queue + routing systems handle travel.
            queue.push(QueuedCommand::MoveTo { system: target }, &position_of);
            continue;
        }

        // We're at the target waypoint — run the mode-specific action.
        match route.mode {
            CourierMode::KnowledgeRelay => {
                if let Some(ref mut store_mut) = empire_store_opt {
                    let store: &mut KnowledgeStore = store_mut;

                    // Step 1: deliver currently-carried entries into the
                    // local store. KnowledgeStore::update preserves newer
                    // observed_at automatically.
                    if let Some(ref mut bag) = knowledge_cargo {
                        for entry in bag.entries.drain(..) {
                            store.update(entry);
                        }
                    }

                    // Step 2: pickup — copy all current snapshots into a
                    // fresh bag, refreshing received_at to "now". Build
                    // the bag locally so we can either update an existing
                    // component in-place or insert a new one for couriers
                    // that didn't have one yet.
                    let new_bag: Vec<SystemKnowledge> = store
                        .iter()
                        .map(|(_, k)| {
                            let mut snap = k.clone();
                            snap.received_at = clock.elapsed;
                            snap
                        })
                        .collect();
                    let count = new_bag.len();

                    if let Some(mut bag) = knowledge_cargo {
                        bag.entries = new_bag;
                    } else {
                        commands
                            .entity(entity)
                            .insert(CourierKnowledgeCargo { entries: new_bag });
                    }

                    info!(
                        "Courier {} picked up {} knowledge snapshots at waypoint",
                        ship.name, count,
                    );
                }
            }
            CourierMode::ResourceTransport => {
                // Cargo capacity used for both load and reserve checks.
                let cap = COURIER_DEFAULT_CARGO_CAPACITY;
                if let Some(mut cargo) = cargo {
                    if let Ok(mut stockpile) = stockpiles_q.get_mut(docked_system) {
                        // Step 1: deliver everything we're carrying first.
                        if cargo.minerals > Amt::ZERO {
                            stockpile.minerals = stockpile.minerals.add(cargo.minerals);
                            info!(
                                "Courier {} delivered {} minerals",
                                ship.name, cargo.minerals
                            );
                            cargo.minerals = Amt::ZERO;
                        }
                        if cargo.energy > Amt::ZERO {
                            stockpile.energy = stockpile.energy.add(cargo.energy);
                            info!("Courier {} delivered {} energy", ship.name, cargo.energy);
                            cargo.energy = Amt::ZERO;
                        }

                        // Step 2: pick up a fresh load for the next leg.
                        let take_m = cap.min(stockpile.minerals);
                        stockpile.minerals = stockpile.minerals.sub(take_m);
                        cargo.minerals = take_m;
                        let take_e = cap.min(stockpile.energy);
                        stockpile.energy = stockpile.energy.sub(take_e);
                        cargo.energy = take_e;
                        if take_m > Amt::ZERO || take_e > Amt::ZERO {
                            info!("Courier {} loaded {}M / {}E", ship.name, take_m, take_e);
                        }
                    }
                }
            }
            CourierMode::MessageDelivery => {
                // Stub: full implementation deferred. Couriers still cycle
                // waypoints so testing the loop logic remains possible.
            }
        }

        // Advance to the next waypoint.
        route.advance();

        // Queue the move to the next waypoint (if any).
        if let Some(next_target) = route.waypoints.get(route.current_index).copied() {
            if next_target != docked_system {
                queue.push(
                    QueuedCommand::MoveTo {
                        system: next_target,
                    },
                    &position_of,
                );
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::ecs::world::World;

    #[test]
    fn route_advance_wraps_when_repeat() {
        let mut world = World::new();
        let a = world.spawn_empty().id();
        let b = world.spawn_empty().id();
        let mut route = CourierRoute::new(vec![a, b], CourierMode::ResourceTransport);
        assert_eq!(route.current_index, 0);
        route.advance();
        assert_eq!(route.current_index, 1);
        route.advance();
        assert_eq!(route.current_index, 0, "repeat should wrap");
    }

    #[test]
    fn route_advance_terminates_without_repeat() {
        let mut world = World::new();
        let a = world.spawn_empty().id();
        let b = world.spawn_empty().id();
        let mut route = CourierRoute::new(vec![a, b], CourierMode::ResourceTransport);
        route.repeat = false;
        route.advance();
        route.advance();
        assert!(route.is_finished());
    }
}
