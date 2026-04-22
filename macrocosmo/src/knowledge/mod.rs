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
use crate::player::{Ruler, StationedAt};
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

/// #409: Record of a ship destroyed in combat, pending light-speed notification.
#[derive(Clone, Debug)]
pub struct DestroyedShipRecord {
    pub entity: Entity,
    pub destruction_pos: [f64; 3],
    pub destruction_tick: i64,
    pub name: String,
    pub design_id: String,
    pub last_known_system: Option<Entity>,
    /// Set to `true` once the "Missing" notification has been emitted.
    pub marked_missing: bool,
}

/// Grace period (in hexadies) before a destroyed ship is considered "missing."
/// Roughly 1 month of game time — the player notices "it should have returned."
pub const MISSING_GRACE_HEXADIES: i64 = 5;

/// #409: Registry of ships destroyed but whose destruction hasn't reached the
/// player yet (light-speed delay). Once light arrives, the corresponding
/// `ShipSnapshot` is marked `Destroyed` and the record is removed.
#[derive(Resource, Default, Clone, Debug)]
pub struct DestroyedShipRegistry {
    pub records: Vec<DestroyedShipRecord>,
}

/// #392: 4-tier system visibility model.
///
/// | Tier | Condition | Visible |
/// |---|---|---|
/// | Catalogued | All stars initially | Star position/type/name only |
/// | Surveyed | Survey done, no connection | Planet details/resources (frozen) |
/// | Connected | Survey + ship presence | Planet + ship + resources (light-delayed) |
/// | Local | Own ship in system | Everything (real-time) |
///
/// For V1: Local == Connected (relay/courier connection detection deferred).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum SystemVisibilityTier {
    Catalogued,
    Surveyed,
    Connected,
    Local,
}

impl SystemVisibilityTier {
    /// Can see planet details (types, habitability, resources snapshot).
    pub fn can_see_planets(&self) -> bool {
        *self >= Self::Surveyed
    }

    /// Can see ship presence and movements.
    pub fn can_see_ships(&self) -> bool {
        *self >= Self::Connected
    }

    /// Can see resource stockpile data.
    pub fn can_see_resources(&self) -> bool {
        *self >= Self::Surveyed
    }

    /// Real-time data (no light-speed delay applied).
    pub fn is_real_time(&self) -> bool {
        *self == Self::Local
    }
}

/// #392: Per-system visibility tier map, attached to each empire entity.
///
/// Systems not present in the map default to `Catalogued`.
#[derive(Component, Default, Debug, Clone)]
pub struct SystemVisibilityMap {
    tiers: HashMap<Entity, SystemVisibilityTier>,
}

impl SystemVisibilityMap {
    pub fn get(&self, system: Entity) -> SystemVisibilityTier {
        self.tiers
            .get(&system)
            .copied()
            .unwrap_or(SystemVisibilityTier::Catalogued)
    }

    pub fn set(&mut self, system: Entity, tier: SystemVisibilityTier) {
        self.tiers.insert(system, tier);
    }

    pub fn iter(&self) -> impl Iterator<Item = (&Entity, &SystemVisibilityTier)> {
        self.tiers.iter()
    }
}

/// #392: Tracks the previous system a ship was in, so `update_visibility_tiers`
/// can recalculate both the old and new system when a ship moves.
#[derive(Component, Default)]
pub struct TrackedShipSystem(pub Option<Entity>);

pub struct KnowledgePlugin;

impl Plugin for KnowledgePlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<RelayNetwork>()
            .init_resource::<DestroyedShipRegistry>()
            .add_systems(
                Startup,
                (initialize_capital_knowledge, initialize_visibility_tiers)
                    .chain()
                    .after(crate::galaxy::generate_galaxy)
                    .after(crate::player::spawn_player)
                    .after(crate::player::spawn_player_empire)
                    .after(crate::setup::run_all_factions_on_game_start),
            )
            .add_systems(Update, propagate_knowledge)
            .add_systems(
                Update,
                update_destroyed_ship_knowledge
                    .after(propagate_knowledge),
            )
            .add_systems(
                Update,
                (rebuild_relay_network, snapshot_production_knowledge)
                    .after(crate::time_system::advance_game_time),
            )
            .add_systems(
                Update,
                (
                    ensure_tracked_ship_system,
                    bevy::ecs::schedule::ApplyDeferred,
                    update_visibility_tiers,
                )
                    .chain()
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
    /// #430: Whether this system is a capital, propagated through KnowledgeStore
    /// so UI/visualization can gate display on knowledge tier.
    pub is_capital: bool,
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
    /// #409: Ship has not returned by expected time — presumed lost.
    Missing,
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

/// #392: Auto-add `TrackedShipSystem` to any ship entity that has `Ship` +
/// `ShipState` but is missing the tracker. Runs every frame but the `Without`
/// filter keeps it cheap after the initial pass.
pub fn ensure_tracked_ship_system(
    mut commands: Commands,
    ships: Query<(Entity, &ShipState), (With<Ship>, Without<TrackedShipSystem>)>,
) {
    for (entity, state) in &ships {
        let current_system = system_from_ship_state(state);
        commands
            .entity(entity)
            .insert(TrackedShipSystem(current_system));
    }
}

/// Extract the system entity a ship is currently in, if any.
fn system_from_ship_state(state: &ShipState) -> Option<Entity> {
    match state {
        ShipState::InSystem { system } => Some(*system),
        ShipState::Surveying { target_system, .. } => Some(*target_system),
        ShipState::Settling { system, .. } => Some(*system),
        ShipState::Refitting { system, .. } => Some(*system),
        ShipState::Scouting { target_system, .. } => Some(*target_system),
        ShipState::SubLight { .. } | ShipState::InFTL { .. } | ShipState::Loitering { .. } => None,
    }
}

/// #392: Determine the visibility tier for a given system from the player's perspective.
///
/// `star_surveyed` indicates whether the live `StarSystem.surveyed` flag is set
/// (ground truth). This is checked alongside the KnowledgeStore snapshot so
/// that newly-surveyed systems (where the snapshot hasn't arrived yet) still
/// get the correct tier.
fn determine_tier_for_system(
    system: Entity,
    player_empire_entity: Entity,
    ships: &Query<(Entity, &Ship, &ShipState)>,
    knowledge: &KnowledgeStore,
    star_surveyed: bool,
) -> SystemVisibilityTier {
    // Check if any player-owned ship is physically present in the system.
    let has_own_ship = ships.iter().any(|(_, ship, state)| {
        let is_player_ship =
            matches!(ship.owner, crate::ship::Owner::Empire(e) if e == player_empire_entity);
        if !is_player_ship {
            return false;
        }
        system_from_ship_state(state) == Some(system)
    });

    if has_own_ship {
        // V1: Local == Connected when ship is present.
        return SystemVisibilityTier::Local;
    }

    // Check if system was surveyed — either from live StarSystem component
    // or from KnowledgeStore snapshot (whichever is true).
    let is_surveyed = star_surveyed
        || knowledge
            .get(system)
            .map(|k| k.data.surveyed)
            .unwrap_or(false);

    if is_surveyed {
        SystemVisibilityTier::Surveyed
    } else {
        SystemVisibilityTier::Catalogued
    }
}

/// #392: Initialize visibility tiers for all star systems at game start.
/// Runs after `initialize_capital_knowledge` so the KnowledgeStore already
/// has the capital entry.
fn initialize_visibility_tiers(
    mut empire_q: Query<
        (Entity, &KnowledgeStore, &mut SystemVisibilityMap),
        With<crate::player::Empire>,
    >,
    ships: Query<(Entity, &Ship, &ShipState)>,
    systems: Query<(Entity, &StarSystem)>,
) {
    for (empire_entity, knowledge, mut vis_map) in &mut empire_q {
        for (system_entity, star) in &systems {
            let tier = determine_tier_for_system(
                system_entity,
                empire_entity,
                &ships,
                knowledge,
                star.surveyed,
            );
            vis_map.set(system_entity, tier);
        }
    }

    info!(
        "System visibility tiers initialized for {} systems across {} empires",
        systems.iter().count(),
        empire_q.iter().count(),
    );
}

/// #392: Event-driven visibility tier update. Runs when any ship's `ShipState`
/// changes, recalculating tiers only for the affected systems (old + new).
pub fn update_visibility_tiers(
    mut empire_q: Query<
        (Entity, &KnowledgeStore, &mut SystemVisibilityMap),
        With<crate::player::Empire>,
    >,
    mut changed_ships: Query<
        (Entity, &Ship, &ShipState, &mut TrackedShipSystem),
        Or<(Changed<ShipState>, Added<TrackedShipSystem>)>,
    >,
    all_ships: Query<(Entity, &Ship, &ShipState)>,
    star_systems: Query<&StarSystem>,
) {
    // Collect systems that need recalculation from changed ships.
    let mut affected_systems: Vec<Entity> = Vec::new();

    for (_entity, _ship, state, mut tracked) in &mut changed_ships {
        let new_system = system_from_ship_state(state);
        let old_system = tracked.0;

        // Update tracker.
        tracked.0 = new_system;

        if let Some(old) = old_system {
            if !affected_systems.contains(&old) {
                affected_systems.push(old);
            }
        }
        if let Some(new) = new_system {
            if !affected_systems.contains(&new) {
                affected_systems.push(new);
            }
        }
    }

    if affected_systems.is_empty() {
        return;
    }

    // Recalculate tier for each affected system, for every empire.
    for (empire_entity, knowledge, mut vis_map) in &mut empire_q {
        for &system in &affected_systems {
            let star_surveyed = star_systems
                .get(system)
                .map(|s| s.surveyed)
                .unwrap_or(false);
            let tier = determine_tier_for_system(
                system,
                empire_entity,
                &all_ships,
                knowledge,
                star_surveyed,
            );
            vis_map.set(system, tier);
        }
    }
}

fn initialize_capital_knowledge(
    mut commands: Commands,
    mut empire_q: Query<(Entity, &mut KnowledgeStore), With<crate::player::Empire>>,
    ruler_q: Query<&StationedAt, Or<(With<Ruler>, With<crate::player::Player>)>>,
    systems: Query<(Entity, &StarSystem, &Position)>,
) {
    // Resolve the capital system: prefer any ruler's StationedAt, fall back
    // to the first StarSystem with `is_capital`.
    let capital_entity = ruler_q
        .iter()
        .next()
        .map(|s| s.system)
        .or_else(|| {
            systems
                .iter()
                .find(|(_, s, _)| s.is_capital)
                .map(|(e, _, _)| e)
        });
    let Some(capital_entity) = capital_entity else {
        warn!("Knowledge init: no capital system found");
        return;
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
        is_capital: true,
        ..default()
    };

    for (empire_entity, mut store) in &mut empire_q {
        store.update(SystemKnowledge {
            system: capital_entity,
            observed_at: 0,
            received_at: 0,
            data: snapshot.clone(),
            source: ObservationSource::Direct,
        });
        // Set the empire's viewer system for light-speed delay calculations.
        commands
            .entity(empire_entity)
            .insert(crate::player::EmpireViewerSystem(capital_entity));
    }

    info!(
        "Knowledge initialized for {} empires: capital '{}'",
        empire_q.iter().count(),
        capital.name
    );
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
        Option<&'static crate::species::ColonyPopulation>,
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
        .any(|(_, c, _, _, _, _, _, _)| c.system(planets) == Some(entity));

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
        is_capital: star.is_capital,
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
    for (colony_entity, colony, production, buildings, bq, maintenance, food, col_pop) in colonies.iter() {
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
            population: col_pop.map(|p| p.total() as f64).unwrap_or(0.0),
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
    ruler_q: Query<&StationedAt, Or<(With<Ruler>, With<crate::player::Player>)>>,
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
    mut empire_q: Query<
        (
            Entity,
            &mut KnowledgeStore,
            Option<&SystemVisibilityMap>,
            Option<&crate::player::EmpireViewerSystem>,
        ),
        With<crate::player::Empire>,
    >,
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
    ships: Query<(Entity, &Ship, &ShipState, &crate::ship::ShipHitpoints)>,
    building_registry: Res<crate::colony::BuildingRegistry>,
) {
    crate::prof_span!("propagate_knowledge");

    // Collect empire data to avoid borrow conflicts during iteration.
    // Fall back to any ruler's StationedAt system for empires without
    // EmpireViewerSystem (e.g. test setups that predate the component).
    let ruler_fallback = ruler_q.iter().next().map(|s| s.system);
    let empire_list: Vec<(Entity, Entity)> = empire_q
        .iter()
        .filter_map(|(e, _, _, viewer)| {
            let system = viewer.map(|v| v.0).or(ruler_fallback)?;
            Some((e, system))
        })
        .collect();

    for (empire_entity, viewer_system) in &empire_list {
        let Ok(viewer_pos) = positions.get(*viewer_system) else {
            continue;
        };

        let Ok((_, mut store, vis_map_opt, _)) = empire_q.get_mut(*empire_entity) else {
            continue;
        };

        // #293: Build per-empire hostile system lookup, filtered by faction
        // relations so only factions this empire considers hostile count.
        let mut hostile_map: HashMap<Entity, f64> = HashMap::new();
        for (at_system, stats, owner) in &hostiles {
            let include = match owner {
                Some(o) => faction_relations
                    .get_or_default(*empire_entity, o.0)
                    .can_attack_aggressive(),
                None => true,
            };
            if include {
                *hostile_map.entry(at_system.0).or_insert(0.0) += stats.strength;
            }
        }

    for (entity, star, sys_pos, stockpile) in &systems {
        // #392: Only propagate system knowledge to systems with tier >= Surveyed.
        // Catalogued-only systems (no survey data) get no knowledge snapshots.
        // V1: Surveyed/Connected/Local all receive light-speed updates;
        // the Surveyed-vs-Connected distinction (frozen vs. live) is deferred
        // to V2 when relay/courier connection detection is implemented.
        //
        // When the visibility map has no entry for a system (e.g. before
        // `update_visibility_tiers` has processed it), fall back to the
        // star's live `surveyed` flag so new/migrated entities still receive
        // knowledge on the first tick.
        if let Some(vis_map) = vis_map_opt {
            let tier = vis_map.get(entity);
            if !tier.can_see_planets() && !star.surveyed {
                continue;
            }
        }

        let distance = physics::distance_ly(viewer_pos, sys_pos);
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
    // Ships are visible based on the light delay from their position to the
    // empire's viewer system.
    let viewer_pos_arr = viewer_pos.as_array();
    for (ship_entity, ship, state, hp) in &ships {
        // Compute the ship's current world position as an [f64; 3].
        // NOTE (#392 V2): Ship snapshot visibility gating by tier (>= Connected)
        // is deferred. V1 continues to propagate all ship snapshots via
        // light-speed delay. The display layer handles tier-based filtering.
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

        let distance = physics::distance_ly_arr(viewer_pos_arr, ship_pos_arr);
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

    } // end empire loop
}

/// #409: Check destroyed ship records. Two-phase transition:
/// 1. After `MISSING_GRACE_HEXADIES` → mark snapshot as Missing, emit ShipMissing event
/// 2. After full light-speed delay → mark snapshot as Destroyed, remove record
pub fn update_destroyed_ship_knowledge(
    clock: Res<GameClock>,
    positions: Query<&Position>,
    mut empire_q: Query<
        (Entity, &mut KnowledgeStore, &crate::player::EmpireViewerSystem),
        With<crate::player::Empire>,
    >,
    mut registry: ResMut<DestroyedShipRegistry>,
    mut events: MessageWriter<crate::events::GameEvent>,
    mut next_id: ResMut<NextEventId>,
    player_empire_q: Query<Entity, With<crate::player::PlayerEmpire>>,
) {
    // Collect empire viewer positions up front to avoid borrow conflicts.
    let empire_viewers: Vec<(Entity, [f64; 3])> = empire_q
        .iter()
        .filter_map(|(entity, _, viewer)| {
            positions.get(viewer.0).ok().map(|p| (entity, p.as_array()))
        })
        .collect();

    if empire_viewers.is_empty() {
        return;
    }

    let player_empire = player_empire_q.iter().next();

    registry.records.retain_mut(|record| {
        let mut all_received_destruction = true;

        for &(empire_entity, viewer_pos_arr) in &empire_viewers {
            let distance = physics::distance_ly_arr(viewer_pos_arr, record.destruction_pos);
            let delay = physics::light_delay_hexadies(distance);
            let arrives_at = record.destruction_tick + delay;

            if clock.elapsed >= arrives_at {
                // This empire can now learn about the destruction.
                if let Ok((_, mut store, _)) = empire_q.get_mut(empire_entity) {
                    store.update_ship(ShipSnapshot {
                        entity: record.entity,
                        name: record.name.clone(),
                        design_id: record.design_id.clone(),
                        last_known_state: ShipSnapshotState::Destroyed,
                        last_known_system: record.last_known_system,
                        observed_at: record.destruction_tick,
                        hp: 0.0,
                        hp_max: 0.0,
                        source: ObservationSource::Direct,
                    });
                }
            } else {
                all_received_destruction = false;

                // ShipMissing event — only emit for the player empire.
                if !record.marked_missing
                    && clock.elapsed >= record.destruction_tick + MISSING_GRACE_HEXADIES
                    && Some(empire_entity) == player_empire
                {
                    if let Ok((_, mut store, _)) = empire_q.get_mut(empire_entity) {
                        store.update_ship(ShipSnapshot {
                            entity: record.entity,
                            name: record.name.clone(),
                            design_id: record.design_id.clone(),
                            last_known_state: ShipSnapshotState::Missing,
                            last_known_system: record.last_known_system,
                            observed_at: clock.elapsed,
                            hp: 0.0,
                            hp_max: 0.0,
                            source: ObservationSource::Direct,
                        });
                    }
                    record.marked_missing = true;
                    events.write(crate::events::GameEvent::new(
                        &mut next_id,
                        clock.elapsed,
                        crate::events::GameEventKind::ShipMissing,
                        format!("{} has not returned — presumed missing", record.name),
                        record.last_known_system,
                    ));
                }
            }
        }

        // Only remove the record when ALL empires have received the destruction.
        !all_received_destruction
    });
}

/// #176: Separate system to snapshot production rates into KnowledgeStore.
/// Runs after colony production systems to avoid query conflicts with &mut Production.
pub fn snapshot_production_knowledge(
    clock: Res<GameClock>,
    positions: Query<&Position>,
    mut empire_q: Query<
        &mut KnowledgeStore,
        With<crate::player::Empire>,
    >,
    colonies: Query<(
        &crate::colony::Colony,
        Option<&crate::colony::Production>,
        Option<&crate::colony::MaintenanceCost>,
    )>,
    planets: Query<&crate::galaxy::Planet>,
) {

    for mut store in &mut empire_q {
        // For each system that already has a knowledge entry, update production data.
        // Collect keys first to avoid borrow issues.
        let system_entities: Vec<Entity> = store.iter().map(|(_, k)| k.system).collect();

        for system_entity in system_entities {
            let mut prod_minerals = Amt::ZERO;
            let mut prod_energy = Amt::ZERO;
            let mut prod_food = Amt::ZERO;
            let mut prod_research = Amt::ZERO;
            let mut maint_energy = Amt::ZERO;

            for (colony, production, maintenance) in colonies.iter() {
                if colony.system(&planets) == Some(system_entity) {
                    if let Some(prod) = production {
                        prod_minerals =
                            prod_minerals.add(prod.minerals_per_hexadies.final_value());
                        prod_energy = prod_energy.add(prod.energy_per_hexadies.final_value());
                        prod_food = prod_food.add(prod.food_per_hexadies.final_value());
                        prod_research =
                            prod_research.add(prod.research_per_hexadies.final_value());
                    }
                    if let Some(maint) = maintenance {
                        maint_energy =
                            maint_energy.add(maint.energy_per_hexadies.final_value());
                    }
                }
            }

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

    // --- #392: SystemVisibilityTier tests ---

    #[test]
    fn visibility_tier_ordering() {
        assert!(SystemVisibilityTier::Catalogued < SystemVisibilityTier::Surveyed);
        assert!(SystemVisibilityTier::Surveyed < SystemVisibilityTier::Connected);
        assert!(SystemVisibilityTier::Connected < SystemVisibilityTier::Local);
    }

    #[test]
    fn visibility_tier_capabilities() {
        let c = SystemVisibilityTier::Catalogued;
        assert!(!c.can_see_planets());
        assert!(!c.can_see_ships());
        assert!(!c.can_see_resources());
        assert!(!c.is_real_time());

        let s = SystemVisibilityTier::Surveyed;
        assert!(s.can_see_planets());
        assert!(!s.can_see_ships());
        assert!(s.can_see_resources());
        assert!(!s.is_real_time());

        let conn = SystemVisibilityTier::Connected;
        assert!(conn.can_see_planets());
        assert!(conn.can_see_ships());
        assert!(conn.can_see_resources());
        assert!(!conn.is_real_time());

        let local = SystemVisibilityTier::Local;
        assert!(local.can_see_planets());
        assert!(local.can_see_ships());
        assert!(local.can_see_resources());
        assert!(local.is_real_time());
    }

    #[test]
    fn visibility_map_defaults_to_catalogued() {
        let mut world = World::new();
        let entity = world.spawn_empty().id();
        let map = SystemVisibilityMap::default();
        assert_eq!(map.get(entity), SystemVisibilityTier::Catalogued);
    }

    #[test]
    fn visibility_map_set_and_get() {
        let mut world = World::new();
        let sys_a = world.spawn_empty().id();
        let sys_b = world.spawn_empty().id();
        let mut map = SystemVisibilityMap::default();
        map.set(sys_a, SystemVisibilityTier::Surveyed);
        map.set(sys_b, SystemVisibilityTier::Local);
        assert_eq!(map.get(sys_a), SystemVisibilityTier::Surveyed);
        assert_eq!(map.get(sys_b), SystemVisibilityTier::Local);
    }

    #[test]
    fn determine_tier_surveyed_when_no_ship_but_surveyed() {
        let mut world = World::new();
        let system = world.spawn_empty().id();
        let mut store = KnowledgeStore::default();
        let mut snapshot = SystemSnapshot::default();
        snapshot.surveyed = true;
        store.update(SystemKnowledge {
            system,
            observed_at: 0,
            received_at: 0,
            data: snapshot,
            source: ObservationSource::Direct,
        });

        assert!(store.get(system).unwrap().data.surveyed);
        // With no ship present and surveyed=true in KnowledgeStore,
        // determine_tier_for_system would return Surveyed.
    }

    #[test]
    fn system_from_ship_state_extracts_system() {
        let mut world = World::new();
        let sys = world.spawn_empty().id();

        assert_eq!(
            system_from_ship_state(&ShipState::InSystem { system: sys }),
            Some(sys)
        );
        assert_eq!(
            system_from_ship_state(&ShipState::Surveying {
                target_system: sys,
                started_at: 0,
                completes_at: 10,
            }),
            Some(sys)
        );
        assert_eq!(
            system_from_ship_state(&ShipState::Settling {
                system: sys,
                planet: None,
                started_at: 0,
                completes_at: 10,
            }),
            Some(sys)
        );
        assert_eq!(
            system_from_ship_state(&ShipState::SubLight {
                origin: [0.0, 0.0, 0.0],
                destination: [1.0, 1.0, 0.0],
                target_system: Some(sys),
                departed_at: 0,
                arrival_at: 100,
            }),
            None
        );
        assert_eq!(
            system_from_ship_state(&ShipState::Loitering {
                position: [5.0, 5.0, 0.0],
            }),
            None
        );
    }
}
