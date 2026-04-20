//! Knowledge store + light-speed propagation.
//!
//! # Observation freshness model (#215)
//!
//! Every [`SystemKnowledge`] / [`ShipSnapshot`] entry carries an
//! [`ObservationSource`] tag identifying how the entry was obtained:
//!
//! * [`ObservationSource::Direct`] — optical / sensor baseline (light-speed).
//!   Written by `propagate_knowledge`, `sensor_buoy_detect_system`, and ship
//!   survey/courier delivery paths.
//! * [`ObservationSource::Relay`] — FTL Comm Relay (#216). Written by
//!   `relay_knowledge_propagate_system`.
//! * [`ObservationSource::Scout`] — scout ship reports (#217). Reserved for
//!   future use.
//! * [`ObservationSource::Stale`] — **never written by producers**; it is a
//!   read-side overlay. The [`perceived::perceived_system`] accessor rewrites
//!   an entry's source to `Stale` when
//!   `current_time - observed_at >= STALE_THRESHOLD_HEXADIES`.
//!
//! Producers should pick exactly one of `Direct / Relay / Scout`. The
//! convention `observed_at: i64` (integer hexadies) is preserved — the
//! [`perceived::PerceivedInfo`] facade only renames it to `last_updated`.
pub mod facts;
pub mod kind_registry;
pub mod payload;
pub mod perceived;

use bevy::prelude::*;
use std::collections::HashMap;

#[allow(unused_imports)]
pub use facts::{
    ArrivalPlan, CombatVictor, EventId, FTL_RELAY_BASE_MULTIPLIER, FactSysParam, KnowledgeFact,
    NextEventId, NotifiedEventIds, PendingFactQueue, PerceivedFact, PlayerVantage, RelayNetwork,
    RelaySnapshot, compute_fact_arrival, effective_relay_range, rebuild_relay_network,
    record_fact_or_local, record_world_event_fact, relay_delay_hexadies, sweep_notified_event_ids,
};

use crate::amount::Amt;
use crate::colony::ResourceStockpile;
use crate::components::Position;
use crate::galaxy::StarSystem;
use crate::physics;
use crate::player::{Player, StationedAt};
use crate::ship::{Ship, ShipState};
use crate::time_system::GameClock;

#[allow(unused_imports)]
pub use perceived::{FactionId, PerceivedInfo, perceived_fleet, perceived_system};

/// Observation source tag for knowledge entries.
///
/// Writers must use `Direct`, `Relay`, or `Scout`. `Stale` is a read-side
/// overlay applied by [`perceived::perceived_system`] — see module docs.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ObservationSource {
    /// Optical / baseline sensor (light-speed propagation, surveys, sensor buoys).
    Direct,
    /// FTL Comm Relay forwarded observation (#216).
    Relay,
    /// Scout ship report (#217).
    Scout,
    /// Read-side overlay: entry is older than [`STALE_THRESHOLD_HEXADIES`].
    /// **Do not write this directly** — accessors apply it on read.
    Stale,
}

/// Staleness threshold in hexadies (≈10 in-game years). Matches the existing
/// "VERY OLD" cutoff used by the system panel.
pub const STALE_THRESHOLD_HEXADIES: i64 = 600;

pub struct KnowledgePlugin;

impl Plugin for KnowledgePlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<RelayNetwork>()
            .add_systems(
                Startup,
                initialize_capital_knowledge
                    .after(crate::galaxy::generate_galaxy)
                    .after(crate::player::spawn_player)
                    .after(crate::player::spawn_player_empire),
            )
            .add_systems(Update, propagate_knowledge)
            .add_systems(
                Update,
                (rebuild_relay_network, snapshot_production_knowledge)
                    .after(crate::time_system::advance_game_time),
            );
    }
}

#[derive(Clone, Debug)]
pub struct SystemKnowledge {
    pub system: Entity,
    pub observed_at: i64,
    pub received_at: i64,
    pub data: SystemSnapshot,
    /// #215: Which channel produced this observation.
    pub source: ObservationSource,
}

#[derive(Clone, Debug, Default)]
pub struct SystemSnapshot {
    pub name: String,
    pub position: [f64; 3],
    pub surveyed: bool,
    pub colonized: bool,
    pub population: f64,
    pub production: f64,
    pub minerals: Amt,
    pub energy: Amt,
    pub food: Amt,
    pub authority: Amt,
    // #176: Extended snapshot fields for light-speed accurate UI/visualization
    pub has_hostile: bool,
    pub hostile_strength: f64,
    pub has_port: bool,
    pub has_shipyard: bool,
    pub habitability: Option<f64>,
    pub mineral_richness: Option<f64>,
    pub energy_potential: Option<f64>,
    pub research_potential: Option<f64>,
    pub max_building_slots: Option<u8>,
    pub production_minerals: Amt,
    pub production_energy: Amt,
    pub production_food: Amt,
    pub production_research: Amt,
    pub maintenance_energy: Amt,
    /// #269: Per-colony snapshot. Populated by the snapshot build path so
    /// remote colony detail UI reads from this instead of the live world.
    /// Empty vec means "system is known but no colonies observed yet".
    pub colonies: Vec<ColonySnapshot>,
}

/// #269: Snapshot of a single colony's observable state at the moment of
/// last observation. Carries enough data for the remote colony detail
/// panel to render without reading live components.
#[derive(Clone, Debug)]
pub struct ColonySnapshot {
    pub colony_entity: Entity,
    pub planet_entity: Entity,
    pub planet_name: String,
    pub population: f64,
    pub carrying_cap_hint: f64,
    pub production_minerals: Amt,
    pub production_energy: Amt,
    pub production_food: Amt,
    pub production_research: Amt,
    pub food_consumption: Amt,
    pub maintenance_energy: Amt,
    pub buildings: Vec<Option<crate::scripting::building_api::BuildingId>>,
    pub build_queue: Vec<BuildQueueEntrySnapshot>,
    pub demolition_queue: Vec<DemolitionSnapshot>,
    pub upgrade_queue: Vec<UpgradeSnapshot>,
}

#[derive(Clone, Debug)]
pub struct BuildQueueEntrySnapshot {
    pub building_id: crate::scripting::building_api::BuildingId,
    pub target_slot: usize,
    pub build_time_remaining: i64,
}

#[derive(Clone, Debug)]
pub struct DemolitionSnapshot {
    pub target_slot: usize,
    pub building_id: crate::scripting::building_api::BuildingId,
    pub time_remaining: i64,
}

#[derive(Clone, Debug)]
pub struct UpgradeSnapshot {
    pub slot_index: usize,
    pub target_id: crate::scripting::building_api::BuildingId,
    pub build_time_remaining: i64,
}

/// #175: Snapshot of a ship's last known state for light-speed delayed visibility.
#[derive(Clone, Debug)]
pub struct ShipSnapshot {
    pub entity: Entity,
    pub name: String,
    pub design_id: String,
    pub last_known_state: ShipSnapshotState,
    pub last_known_system: Option<Entity>,
    pub observed_at: i64,
    pub hp: f64,
    pub hp_max: f64,
    /// #215: Which channel produced this observation.
    pub source: ObservationSource,
}

/// Simplified ship state for knowledge snapshots.
#[derive(Clone, Debug, PartialEq)]
pub enum ShipSnapshotState {
    InSystem,
    InTransit,
    Surveying,
    Settling,
    Refitting,
    Destroyed,
    /// #185: Ship is loitering at a deep-space coordinate (not in any system).
    Loitering {
        position: [f64; 3],
    },
}

#[derive(Resource, Component, Default)]
pub struct KnowledgeStore {
    entries: HashMap<Entity, SystemKnowledge>,
    /// #175: Ship snapshots keyed by ship entity. Updated via light-speed propagation.
    ship_snapshots: HashMap<Entity, ShipSnapshot>,
}

impl KnowledgeStore {
    pub fn get(&self, system: Entity) -> Option<&SystemKnowledge> {
        self.entries.get(&system)
    }

    pub fn update(&mut self, knowledge: SystemKnowledge) {
        let dominated = self.entries.get(&knowledge.system).is_some_and(|existing| {
            // Scout vs Relay source priority.
            //
            // Scout observations carry high-fidelity sensor-range data
            // (ships + structures snapshot) gathered by a ship physically
            // deployed to the target. Relay entries are continuous
            // low-fidelity Sensor-Buoy forwards that #216 writes every tick
            // with `observed_at = clock.elapsed` for every star system
            // within a source relay's range. In same-tick races the Relay
            // write would otherwise overwrite a fresh Scout report.
            //
            // Rule: Scout always dominates Relay, regardless of observed_at.
            // A newer Scout, any Direct observation, or the Stale overlay in
            // `perceived_system` still take over as expected.
            if existing.source == ObservationSource::Scout
                && knowledge.source == ObservationSource::Relay
            {
                return true;
            }
            if existing.source == ObservationSource::Relay
                && knowledge.source == ObservationSource::Scout
            {
                return false;
            }
            existing.observed_at >= knowledge.observed_at
        });

        if !dominated {
            self.entries.insert(knowledge.system, knowledge);
        }
    }

    pub fn info_age(&self, system: Entity, current_time: i64) -> Option<i64> {
        self.entries
            .get(&system)
            .map(|k| current_time - k.observed_at)
    }

    pub fn iter(&self) -> impl Iterator<Item = (&Entity, &SystemKnowledge)> {
        self.entries.iter()
    }

    /// #175: Get a ship snapshot by entity.
    pub fn get_ship(&self, ship: Entity) -> Option<&ShipSnapshot> {
        self.ship_snapshots.get(&ship)
    }

    /// #175: Update a ship snapshot. Only replaces if observed_at is newer.
    pub fn update_ship(&mut self, snapshot: ShipSnapshot) {
        let dominated = self
            .ship_snapshots
            .get(&snapshot.entity)
            .is_some_and(|existing| existing.observed_at >= snapshot.observed_at);
        if !dominated {
            self.ship_snapshots.insert(snapshot.entity, snapshot);
        }
    }

    /// #175: Iterate over all ship snapshots.
    pub fn iter_ships(&self) -> impl Iterator<Item = (&Entity, &ShipSnapshot)> {
        self.ship_snapshots.iter()
    }
}

fn initialize_capital_knowledge(
    mut empire_q: Query<&mut KnowledgeStore, With<crate::player::PlayerEmpire>>,
    player_q: Query<&StationedAt, With<Player>>,
    systems: Query<(Entity, &StarSystem, &Position)>,
) {
    let Ok(mut store) = empire_q.single_mut() else {
        warn!("Knowledge init: no player empire found");
        return;
    };
    let capital_entity = match player_q.iter().next() {
        Some(stationed) => stationed.system,
        None => {
            warn!("Knowledge init: no player found");
            return;
        }
    };

    let (_, capital, capital_pos) = match systems.get(capital_entity) {
        Ok(result) => result,
        Err(_) => {
            warn!("Knowledge init: capital entity not found");
            return;
        }
    };

    let snapshot = SystemSnapshot {
        name: capital.name.clone(),
        position: capital_pos.as_array(),
        surveyed: capital.surveyed,
        colonized: true, // Capital is always colonized
        population: 1.0,
        production: 1.0,
        ..default()
    };

    store.update(SystemKnowledge {
        system: capital_entity,
        observed_at: 0,
        received_at: 0,
        data: snapshot,
        source: ObservationSource::Direct,
    });

    info!("Player knowledge initialized: capital '{}'", capital.name);
}

/// #216: Build a `SystemSnapshot` describing the observed state of a star
/// system. Shared by `propagate_knowledge` (light-speed direct) and
/// `relay_knowledge_propagate_system` (FTL relay) so that relay-delivered
/// snapshots carry the same payload fields as direct observations.
///
/// `hostile_map` is a `system_entity → hostile_strength` lookup the caller
/// builds once per tick; passing it in lets both call sites share the
/// allocation. (#293: decoupled from legacy `HostilePresence` component.)
/// #269: The rich colony query used when building a `SystemSnapshot`. Pulled
/// out as a type alias so `build_system_snapshot` call sites don't repeat
/// the whole tuple.
pub type ColonySnapshotQuery<'w, 's> = Query<
    'w,
    's,
    (
        Entity,
        &'static crate::colony::Colony,
        Option<&'static crate::colony::Production>,
        Option<&'static crate::colony::Buildings>,
        Option<&'static crate::colony::BuildingQueue>,
        Option<&'static crate::colony::MaintenanceCost>,
        Option<&'static crate::colony::FoodConsumption>,
    ),
>;

pub fn build_system_snapshot(
    entity: Entity,
    star: &StarSystem,
    sys_pos: &Position,
    stockpile: Option<&ResourceStockpile>,
    has_port: bool,
    has_shipyard: bool,
    colonies: &ColonySnapshotQuery,
    planets: &Query<&crate::galaxy::Planet>,
    planet_attrs: &Query<(&crate::galaxy::Planet, &crate::galaxy::SystemAttributes)>,
    hostile_map: &HashMap<Entity, f64>,
) -> SystemSnapshot {
    crate::prof_span!("build_system_snapshot");
    let is_colonized = colonies
        .iter()
        .any(|(_, c, _, _, _, _, _)| c.system(planets) == Some(entity));

    // Resource snapshot from StarSystem's stockpile (#106)
    let (minerals, energy, food, authority) = stockpile
        .map(|s| (s.minerals, s.energy, s.food, s.authority))
        .unwrap_or((Amt::ZERO, Amt::ZERO, Amt::ZERO, Amt::ZERO));

    // #176: Hostile presence (#293: value is pre-computed strength from
    // HostileStats, built once per tick at the call site).
    let hostile = hostile_map.get(&entity);
    let has_hostile = hostile.is_some();
    let hostile_strength = hostile.copied().unwrap_or(0.0);

    // #176: System attributes — derive from best planet in the system
    let best_attrs = planet_attrs
        .iter()
        .filter(|(p, _)| p.system == entity)
        .map(|(_, a)| a)
        .max_by(|a, b| {
            a.habitability
                .partial_cmp(&b.habitability)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
    let (habitability, mineral_richness, energy_potential, research_potential, max_building_slots) =
        best_attrs
            .map(|a| {
                (
                    Some(a.habitability),
                    Some(a.mineral_richness),
                    Some(a.energy_potential),
                    Some(a.research_potential),
                    Some(a.max_building_slots),
                )
            })
            .unwrap_or((None, None, None, None, None));

    let colony_snapshots = build_colony_snapshots(entity, colonies, planets, planet_attrs);

    SystemSnapshot {
        name: star.name.clone(),
        position: sys_pos.as_array(),
        surveyed: star.surveyed,
        colonized: is_colonized,
        minerals,
        energy,
        food,
        authority,
        has_hostile,
        hostile_strength,
        has_port,
        has_shipyard,
        habitability,
        mineral_richness,
        energy_potential,
        research_potential,
        max_building_slots,
        colonies: colony_snapshots,
        ..SystemSnapshot::default()
    }
}

/// #269: Build per-colony snapshots for the colonies living in `system`.
/// Population, production, maintenance, buildings, and queue contents are
/// frozen at the current world state — the returned vec becomes the
/// snapshot the remote colony detail panel reads from.
fn build_colony_snapshots(
    system: Entity,
    colonies: &ColonySnapshotQuery,
    planets: &Query<&crate::galaxy::Planet>,
    planet_attrs: &Query<(&crate::galaxy::Planet, &crate::galaxy::SystemAttributes)>,
) -> Vec<ColonySnapshot> {
    use crate::galaxy::{BASE_CARRYING_CAPACITY, FOOD_PER_POP_PER_HEXADIES};
    let mut out = Vec::new();
    for (colony_entity, colony, production, buildings, bq, maintenance, food) in colonies.iter() {
        if colony.system(planets) != Some(system) {
            continue;
        }
        let planet_name = planets
            .get(colony.planet)
            .ok()
            .map(|p| p.name.clone())
            .unwrap_or_default();
        let habitability = planet_attrs
            .get(colony.planet)
            .ok()
            .map(|(_, a)| a.habitability)
            .unwrap_or(0.5);
        let food_prod = production
            .map(|p| p.food_per_hexadies.final_value())
            .unwrap_or(Amt::ZERO);
        let k_habitat = BASE_CARRYING_CAPACITY * habitability;
        let k_food = if FOOD_PER_POP_PER_HEXADIES.raw() > 0 {
            food_prod.div_amt(FOOD_PER_POP_PER_HEXADIES).to_f64()
        } else {
            k_habitat
        };
        let carrying_cap_hint = k_habitat.min(k_food).max(1.0);
        let build_queue = bq
            .map(|b| {
                b.queue
                    .iter()
                    .map(|o| BuildQueueEntrySnapshot {
                        building_id: o.building_id.clone(),
                        target_slot: o.target_slot,
                        build_time_remaining: o.build_time_remaining,
                    })
                    .collect()
            })
            .unwrap_or_default();
        let demolition_queue = bq
            .map(|b| {
                b.demolition_queue
                    .iter()
                    .map(|d| DemolitionSnapshot {
                        target_slot: d.target_slot,
                        building_id: d.building_id.clone(),
                        time_remaining: d.time_remaining,
                    })
                    .collect()
            })
            .unwrap_or_default();
        let upgrade_queue = bq
            .map(|b| {
                b.upgrade_queue
                    .iter()
                    .map(|u| UpgradeSnapshot {
                        slot_index: u.slot_index,
                        target_id: u.target_id.clone(),
                        build_time_remaining: u.build_time_remaining,
                    })
                    .collect()
            })
            .unwrap_or_default();
        out.push(ColonySnapshot {
            colony_entity,
            planet_entity: colony.planet,
            planet_name,
            population: colony.population,
            carrying_cap_hint,
            production_minerals: production
                .map(|p| p.minerals_per_hexadies.final_value())
                .unwrap_or(Amt::ZERO),
            production_energy: production
                .map(|p| p.energy_per_hexadies.final_value())
                .unwrap_or(Amt::ZERO),
            production_food: food_prod,
            production_research: production
                .map(|p| p.research_per_hexadies.final_value())
                .unwrap_or(Amt::ZERO),
            food_consumption: food
                .map(|f| f.food_per_hexadies.final_value())
                .unwrap_or(Amt::ZERO),
            maintenance_energy: maintenance
                .map(|m| m.energy_per_hexadies.final_value())
                .unwrap_or(Amt::ZERO),
            buildings: buildings.map(|b| b.slots.clone()).unwrap_or_default(),
            build_queue,
            demolition_queue,
            upgrade_queue,
        });
    }
    out
}

pub fn propagate_knowledge(
    clock: Res<GameClock>,
    player_q: Query<&StationedAt, With<Player>>,
    systems: Query<(
        Entity,
        &StarSystem,
        &Position,
        Option<&ResourceStockpile>,
    )>,
    station_ships: Query<(
        Entity,
        &crate::ship::Ship,
        &crate::ship::ShipState,
        &crate::colony::SlotAssignment,
    )>,
    positions: Query<&Position>,
    mut empire_q: Query<&mut KnowledgeStore, With<crate::player::PlayerEmpire>>,
    colonies: ColonySnapshotQuery,
    planets: Query<&crate::galaxy::Planet>,
    planet_attrs: Query<(&crate::galaxy::Planet, &crate::galaxy::SystemAttributes)>,
    hostiles: Query<
        (
            &crate::galaxy::AtSystem,
            &crate::galaxy::HostileStats,
            Option<&crate::faction::FactionOwner>,
        ),
        With<crate::galaxy::Hostile>,
    >,
    faction_relations: Res<crate::faction::FactionRelations>,
    empire_entity_q: Query<Entity, With<crate::player::PlayerEmpire>>,
    ships: Query<(Entity, &Ship, &ShipState, &crate::ship::ShipHitpoints)>,
    building_registry: Res<crate::colony::BuildingRegistry>,
) {
    crate::prof_span!("propagate_knowledge");
    let Ok(mut store) = empire_q.single_mut() else {
        return;
    };
    let stationed = match player_q.iter().next() {
        Some(s) => s,
        None => return,
    };

    let player_pos = match positions.get(stationed.system) {
        Ok(pos) => pos,
        Err(_) => return,
    };

    // #293: Build hostile system lookup from (AtSystem, HostileStats,
    // FactionOwner, Hostile). Filter by FactionRelations so only factions the
    // viewing empire considers hostile (can_attack_aggressive) count. Falls
    // back to including every hostile when the empire entity is not yet
    // spawned (tests without PlayerEmpire).
    let viewer_empire = empire_entity_q.iter().next();
    let mut hostile_map: HashMap<Entity, f64> = HashMap::new();
    for (at_system, stats, owner) in &hostiles {
        // Hostiles without FactionOwner default to "included" (legacy
        // behavior) — same fallback used by settlement/scout/viz.
        let include = match (viewer_empire, owner) {
            (Some(viewer), Some(o)) => faction_relations
                .get_or_default(viewer, o.0)
                .can_attack_aggressive(),
            _ => true,
        };
        if include {
            // Sum strength if multiple hostiles share a system.
            *hostile_map.entry(at_system.0).or_insert(0.0) += stats.strength;
        }
    }

    for (entity, star, sys_pos, stockpile) in &systems {
        let distance = physics::distance_ly(player_pos, sys_pos);
        let delay = physics::light_delay_hexadies(distance);
        let observed_at = clock.elapsed - delay;

        if observed_at < 0 {
            continue;
        }

        let dominated = store
            .get(entity)
            .is_some_and(|existing| existing.observed_at >= observed_at);

        if dominated {
            continue;
        }

        let sys_has_port = crate::colony::system_buildings::system_has_port(
            entity,
            &station_ships,
            &building_registry,
        );
        let sys_has_shipyard = crate::colony::system_buildings::system_has_shipyard(
            entity,
            &station_ships,
            &building_registry,
        );
        let snapshot = build_system_snapshot(
            entity,
            star,
            sys_pos,
            stockpile,
            sys_has_port,
            sys_has_shipyard,
            &colonies,
            &planets,
            &planet_attrs,
            &hostile_map,
        );

        store.update(SystemKnowledge {
            system: entity,
            observed_at,
            received_at: clock.elapsed,
            data: snapshot,
            source: ObservationSource::Direct,
        });
    }

    // #175 / #188: Ship knowledge propagation.
    // Ships are visible based on the light delay from their position to the player.
    // For docked / surveying / settling / refitting ships, use the system's position.
    // For SubLight ships, interpolate the current position from origin/destination
    // and apply the resulting light-speed delay (#188 fix — previously SubLight was
    // delivered with distance=0 because the position lookup returned None).
    // For Loitering ships, use the loitering coordinate directly (#185).
    let player_pos_arr = player_pos.as_array();
    for (ship_entity, ship, state, hp) in &ships {
        // Compute the ship's current world position as an [f64; 3].
        let ship_pos_arr: Option<[f64; 3]> = match state {
            ShipState::InSystem { system } => positions.get(*system).ok().map(|p| p.as_array()),
            ShipState::Surveying { target_system, .. } => {
                positions.get(*target_system).ok().map(|p| p.as_array())
            }
            ShipState::Settling { system, .. } => positions.get(*system).ok().map(|p| p.as_array()),
            ShipState::Refitting { system, .. } => {
                positions.get(*system).ok().map(|p| p.as_array())
            }
            ShipState::InFTL { origin_system, .. } => {
                // FTL ships are typically invisible to baseline sensors anyway, but use
                // the origin system as a coarse reference (matches existing behavior).
                positions.get(*origin_system).ok().map(|p| p.as_array())
            }
            ShipState::SubLight {
                origin,
                destination,
                departed_at,
                arrival_at,
                ..
            } => {
                // #188: Interpolate sublight position so the light delay reflects the
                // ship's actual deep-space location, not (0,0,0).
                let total = (*arrival_at - *departed_at) as f64;
                let elapsed = (clock.elapsed - *departed_at) as f64;
                let t = if total > 0.0 {
                    (elapsed / total).clamp(0.0, 1.0)
                } else {
                    1.0
                };
                Some([
                    origin[0] + (destination[0] - origin[0]) * t,
                    origin[1] + (destination[1] - origin[1]) * t,
                    origin[2] + (destination[2] - origin[2]) * t,
                ])
            }
            // #185: Loitering — coordinate is encoded in the state itself.
            ShipState::Loitering { position } => Some(*position),
            // #217: Scouting ships are parked at the target system's
            // position (they sit in orbit while the observation timer ticks).
            ShipState::Scouting { target_system, .. } => {
                positions.get(*target_system).ok().map(|p| p.as_array())
            }
        };

        let Some(ship_pos_arr) = ship_pos_arr else {
            continue;
        };

        let distance = physics::distance_ly_arr(player_pos_arr, ship_pos_arr);
        let delay = physics::light_delay_hexadies(distance);
        let observed_at = clock.elapsed - delay;

        if observed_at < 0 {
            continue;
        }

        let dominated = store
            .get_ship(ship_entity)
            .is_some_and(|existing| existing.observed_at >= observed_at);
        if dominated {
            continue;
        }

        let (snapshot_state, last_system) = match state {
            ShipState::InSystem { system } => (ShipSnapshotState::InSystem, Some(*system)),
            ShipState::SubLight { target_system, .. } => {
                (ShipSnapshotState::InTransit, *target_system)
            }
            ShipState::InFTL {
                destination_system, ..
            } => (ShipSnapshotState::InTransit, Some(*destination_system)),
            ShipState::Surveying { target_system, .. } => {
                (ShipSnapshotState::Surveying, Some(*target_system))
            }
            ShipState::Settling { system, .. } => (ShipSnapshotState::Settling, Some(*system)),
            ShipState::Refitting { system, .. } => (ShipSnapshotState::Refitting, Some(*system)),
            // #185: Loitering snapshot carries its position so the UI can render the
            // ship's last-known location even after it has moved on.
            ShipState::Loitering { position } => (
                ShipSnapshotState::Loitering {
                    position: *position,
                },
                None,
            ),
            // #217: Scouting — surface like Surveying to external observers.
            ShipState::Scouting { target_system, .. } => {
                (ShipSnapshotState::Surveying, Some(*target_system))
            }
        };

        store.update_ship(ShipSnapshot {
            entity: ship_entity,
            name: ship.name.clone(),
            design_id: ship.design_id.clone(),
            last_known_state: snapshot_state,
            last_known_system: last_system,
            observed_at,
            hp: hp.hull,
            hp_max: hp.hull_max,
            source: ObservationSource::Direct,
        });
    }
}

/// #176: Separate system to snapshot production rates into KnowledgeStore.
/// Runs after colony production systems to avoid query conflicts with &mut Production.
pub fn snapshot_production_knowledge(
    clock: Res<GameClock>,
    player_q: Query<&StationedAt, With<Player>>,
    positions: Query<&Position>,
    mut empire_q: Query<&mut KnowledgeStore, With<crate::player::PlayerEmpire>>,
    colonies: Query<(
        &crate::colony::Colony,
        Option<&crate::colony::Production>,
        Option<&crate::colony::MaintenanceCost>,
    )>,
    planets: Query<&crate::galaxy::Planet>,
) {
    let Ok(mut store) = empire_q.single_mut() else {
        return;
    };
    let stationed = match player_q.iter().next() {
        Some(s) => s,
        None => return,
    };
    let player_pos = match positions.get(stationed.system) {
        Ok(pos) => pos,
        Err(_) => return,
    };

    // For each system that already has a knowledge entry, update production data
    // We collect the keys first to avoid borrow issues
    let system_entities: Vec<Entity> = store.iter().map(|(_, k)| k.system).collect();

    for system_entity in system_entities {
        // Compute production for this system
        let mut prod_minerals = Amt::ZERO;
        let mut prod_energy = Amt::ZERO;
        let mut prod_food = Amt::ZERO;
        let mut prod_research = Amt::ZERO;
        let mut maint_energy = Amt::ZERO;

        for (colony, production, maintenance) in colonies.iter() {
            if colony.system(&planets) == Some(system_entity) {
                if let Some(prod) = production {
                    prod_minerals = prod_minerals.add(prod.minerals_per_hexadies.final_value());
                    prod_energy = prod_energy.add(prod.energy_per_hexadies.final_value());
                    prod_food = prod_food.add(prod.food_per_hexadies.final_value());
                    prod_research = prod_research.add(prod.research_per_hexadies.final_value());
                }
                if let Some(maint) = maintenance {
                    maint_energy = maint_energy.add(maint.energy_per_hexadies.final_value());
                }
            }
        }

        // Only update if there's actual production data
        if prod_minerals > Amt::ZERO
            || prod_energy > Amt::ZERO
            || prod_food > Amt::ZERO
            || prod_research > Amt::ZERO
            || maint_energy > Amt::ZERO
        {
            if let Some(entry) = store.entries.get_mut(&system_entity) {
                entry.data.production_minerals = prod_minerals;
                entry.data.production_energy = prod_energy;
                entry.data.production_food = prod_food;
                entry.data.production_research = prod_research;
                entry.data.maintenance_energy = maint_energy;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::ecs::world::World;

    fn make_knowledge(system: Entity, observed_at: i64) -> SystemKnowledge {
        SystemKnowledge {
            system,
            observed_at,
            received_at: observed_at,
            data: SystemSnapshot::default(),
            source: ObservationSource::Direct,
        }
    }

    #[test]
    fn update_inserts_new_knowledge() {
        let mut world = World::new();
        let entity = world.spawn_empty().id();
        let mut store = KnowledgeStore::default();
        store.update(make_knowledge(entity, 10));
        assert!(store.get(entity).is_some());
        assert_eq!(store.get(entity).unwrap().observed_at, 10);
    }

    #[test]
    fn newer_observation_replaces_older() {
        let mut world = World::new();
        let entity = world.spawn_empty().id();
        let mut store = KnowledgeStore::default();
        store.update(make_knowledge(entity, 10));
        store.update(make_knowledge(entity, 20));
        assert_eq!(store.get(entity).unwrap().observed_at, 20);
    }

    #[test]
    fn older_observation_does_not_replace_newer() {
        let mut world = World::new();
        let entity = world.spawn_empty().id();
        let mut store = KnowledgeStore::default();
        store.update(make_knowledge(entity, 20));
        store.update(make_knowledge(entity, 10));
        assert_eq!(store.get(entity).unwrap().observed_at, 20);
    }

    #[test]
    fn info_age_returns_correct_value() {
        let mut world = World::new();
        let entity = world.spawn_empty().id();
        let mut store = KnowledgeStore::default();
        store.update(make_knowledge(entity, 10));
        assert_eq!(store.info_age(entity, 25), Some(15));
    }

    #[test]
    fn info_age_returns_none_for_unknown() {
        let mut world = World::new();
        let entity = world.spawn_empty().id();
        let store = KnowledgeStore::default();
        assert_eq!(store.info_age(entity, 100), None);
    }
}
