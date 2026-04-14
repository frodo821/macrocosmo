//! Save-game serialization (#247, Phase A).
//!
//! Walks the Bevy [`World`], snapshots selected resources and persistable
//! entities into a [`GameSave`] root struct, then postcard-encodes the result
//! to a writer or on-disk path. The sibling [`super::load`] module performs
//! the inverse operation.
//!
//! Phase A persists: galaxy (StarSystem/Planet/attributes/sovereignty/hostile/
//! ports), colonies (Colony/stockpile/capacity), ship basics
//! (Ship/ShipState/HP/cargo), faction identity (FactionOwner/Faction), player
//! location (Player/StationedAt/AboardShip/Empire/PlayerEmpire), galaxy config,
//! game clock + speed, production tick, game RNG stream, and faction relations.
//!
//! Deferred to Phase B/C: ship command queues, colony build queues, deep-space
//! structures, knowledge store, tech tree, pending commands, event/notification
//! logs, Lua registries (re-derived from scripts on load).

use bevy::prelude::*;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::io::Write;
use std::path::Path;

use crate::colony::{
    AuthorityParams, BuildQueue, Buildings, BuildingQueue, Colony, ColonizationQueue,
    ColonyJobRates, ConstructionParams, DeliverableStockpile, FoodConsumption, LastProductionTick,
    MaintenanceCost, PendingColonizationOrder, Production, ProductionFocus, ResourceCapacity,
    ResourceStockpile, SystemBuildingQueue, SystemBuildings,
};
use crate::communication::{CommandLog, PendingCommand};
use crate::components::{MovementState, Position};
use crate::condition::ScopedFlags;
use crate::deep_space::{
    ConstructionPlatform, DeepSpaceStructure, FTLCommRelay, LifetimeCost, Scrapyard,
    StructureHitpoints,
};
use crate::empire::CommsParams;
use crate::events::EventLog;
use crate::faction::{FactionOwner, FactionRelations, PendingDiplomaticAction};
use crate::galaxy::{
    Anomalies, AtSystem, ForbiddenRegion, GalaxyConfig, Hostile, HostileHitpoints, HostileStats,
    ObscuredByGas, Planet, PortFacility, Sovereignty, StarSystem, SystemAttributes,
};
use crate::knowledge::{KnowledgeStore, PendingFactQueue};
use crate::notifications::NotificationQueue;
use crate::player::{AboardShip, Empire, Faction, Player, PlayerEmpire, StationedAt};
use crate::scripting::game_rng::GameRng;
use crate::ship::scout::ScoutReport;
use crate::ship::{
    Cargo, CommandQueue, CourierRoute, DetectedHostiles, Fleet, FleetMembership,
    PendingShipCommand, RulesOfEngagement, Ship, ShipHitpoints, ShipModifiers, ShipState,
    SurveyData,
};
use crate::species::{ColonyJobs, ColonyPopulation};
use crate::technology::{
    EmpireModifiers, GameFlags, GlobalParams, PendingColonyTechModifiers,
    PendingKnowledgePropagation, PendingResearch, RecentlyResearched, ResearchPool, ResearchQueue,
    TechKnowledge, TechTree,
};
use crate::time_system::{GameClock, GameSpeed};

use super::remap::{entity_pair_map_serde, EntityMap};
use super::rng_serde::SavedGameRng;
use super::savebag::*;

/// Save format wire version. Bump on breaking changes.
pub const SAVE_VERSION: u32 = 1;

/// Script content fingerprint. On load, a mismatch is warn-logged but loading
/// proceeds. Bump the minor to signal breaking Lua-registry changes to players.
pub const SCRIPTS_VERSION: &str = "0.1";

/// Marker component inserted on every load-created entity so a subsequent
/// save knows which entities are game-owned (vs. engine-/editor-owned).
/// Phase A always assigns [`SaveId`] as well; this marker is reserved for
/// selective despawn on load.
#[derive(Component, Debug, Clone, Copy)]
pub struct SaveableMarker;

/// Stable per-entity save identifier. Assigned by the save pipeline if not
/// already present on the entity; surfaced on load so a subsequent save keeps
/// ids stable (needed to diff saves and to preserve entity identity in logs).
#[derive(Component, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct SaveId(pub u64);

// ---------------------------------------------------------------------------
// Root save structs
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GameSave {
    pub version: u32,
    pub scripts_version: String,
    pub resources: SavedResources,
    pub entities: Vec<SavedEntity>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SavedEntity {
    pub save_id: u64,
    pub components: SavedComponentBag,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SavedResources {
    pub game_clock_elapsed: i64,
    pub game_speed_hexadies_per_second: f64,
    pub game_speed_previous: f64,
    pub last_production_tick: i64,
    pub galaxy_config: Option<SavedGalaxyConfig>,
    pub game_rng: Option<SavedGameRng>,
    pub faction_relations: Option<SavedFactionRelations>,
    /// Phase B — knowledge fact queue (Resource form; usually attached to
    /// empire entity as Component too; the Resource copy is the primary).
    pub pending_fact_queue: Option<SavedPendingFactQueue>,
    /// Phase B — persistable event log (Resource).
    pub event_log: Option<SavedEventLog>,
    /// Phase B — on-screen notification banners (Resource).
    pub notification_queue: Option<SavedNotificationQueue>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SavedGalaxyConfig {
    pub radius: f64,
    pub num_systems: usize,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SavedFactionRelations {
    #[serde(with = "entity_pair_map_serde")]
    pub relations: HashMap<(Entity, Entity), SavedFactionView>,
}

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

#[derive(Debug)]
pub enum SaveError {
    Io(std::io::Error),
    Postcard(postcard::Error),
}

impl std::fmt::Display for SaveError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SaveError::Io(e) => write!(f, "I/O error: {e}"),
            SaveError::Postcard(e) => write!(f, "postcard encode error: {e}"),
        }
    }
}
impl std::error::Error for SaveError {}
impl From<std::io::Error> for SaveError {
    fn from(e: std::io::Error) -> Self {
        SaveError::Io(e)
    }
}
impl From<postcard::Error> for SaveError {
    fn from(e: postcard::Error) -> Self {
        SaveError::Postcard(e)
    }
}

// ---------------------------------------------------------------------------
// Save pipeline
// ---------------------------------------------------------------------------

/// Assign a [`SaveId`] to every persistable entity that lacks one.
///
/// Phase A uses the live `Entity::to_bits()` value as the save id. This is
/// stable for the duration of the save (entities are not despawned between
/// assignment and snapshot) and means that raw `to_bits()` references
/// elsewhere in the component graph already match the EntityMap's keys —
/// callers don't need to translate references through a second indirection.
///
/// On load, a fresh EntityMap is built that maps these bit values to the
/// freshly allocated `Entity`s.
fn assign_save_ids(world: &mut World) {
    let mut to_assign: Vec<Entity> = Vec::new();
    {
        // First OR bundle (up to 15 filters per Or tuple).
        let mut q = world.query_filtered::<
            Entity,
            Or<(
                With<StarSystem>,
                With<Planet>,
                With<Colony>,
                With<Ship>,
                With<Hostile>,
                With<Empire>,
                With<Faction>,
                With<Player>,
                With<DeepSpaceStructure>,
                With<Fleet>,
                With<PendingShipCommand>,
                With<PendingDiplomaticAction>,
                With<PendingCommand>,
                With<PendingResearch>,
                With<PendingKnowledgePropagation>,
            )>,
        >();
        for e in q.iter(world) {
            to_assign.push(e);
        }
    }
    {
        // Second bundle — additional types split for the 15-tuple limit.
        let mut q = world.query_filtered::<
            Entity,
            Or<(
                With<PendingColonizationOrder>,
                With<ForbiddenRegion>,
                With<PortFacility>,
            )>,
        >();
        for e in q.iter(world) {
            to_assign.push(e);
        }
    }

    for e in to_assign {
        if world.entity(e).get::<SaveId>().is_none() {
            let id = e.to_bits();
            world
                .entity_mut(e)
                .insert((SaveId(id), SaveableMarker));
        }
    }
}

/// Build an [`EntityMap`] from the current world's [`SaveId`] components.
fn build_entity_map(world: &mut World) -> EntityMap {
    let mut map = EntityMap::new();
    let mut q = world.query::<(Entity, &SaveId)>();
    for (e, sid) in q.iter(world) {
        map.insert(sid.0, e);
    }
    map
}

/// Snapshot persistable resources into [`SavedResources`].
///
/// Resource fields that carry `Entity` references (currently
/// [`FactionRelations`]) are rewritten to save-id-encoded keys via the
/// supplied [`EntityMap`] so load can resolve them after entities are
/// re-spawned. Entities that lack a SaveId are skipped to avoid encoding
/// stale references.
fn capture_resources(world: &World, entity_map: &EntityMap) -> Result<SavedResources, SaveError> {
    let clock = world.get_resource::<GameClock>();
    let speed = world.get_resource::<GameSpeed>();
    let last_tick = world.get_resource::<LastProductionTick>();
    let galaxy = world.get_resource::<GalaxyConfig>();
    let rng = world.get_resource::<GameRng>();
    let relations = world.get_resource::<FactionRelations>();
    let fact_queue = world.get_resource::<PendingFactQueue>();
    let event_log = world.get_resource::<EventLog>();
    let notifications = world.get_resource::<NotificationQueue>();

    Ok(SavedResources {
        game_clock_elapsed: clock.map(|c| c.elapsed).unwrap_or(0),
        game_speed_hexadies_per_second: speed.map(|s| s.hexadies_per_second).unwrap_or(0.0),
        game_speed_previous: speed.map(|s| s.previous_speed).unwrap_or(1.0),
        last_production_tick: last_tick.map(|t| t.0).unwrap_or(0),
        galaxy_config: galaxy.map(|g| SavedGalaxyConfig {
            radius: g.radius,
            num_systems: g.num_systems,
        }),
        game_rng: match rng {
            Some(r) => Some(SavedGameRng::capture(r)?),
            None => None,
        },
        pending_fact_queue: fact_queue.map(SavedPendingFactQueue::from_live),
        event_log: event_log.map(SavedEventLog::from_live),
        notification_queue: notifications.map(SavedNotificationQueue::from_live),
        faction_relations: relations.map(|rel| {
            let mut out = HashMap::new();
            for ((from, to), view) in rel.relations.iter() {
                // Only encode pairs where both endpoints are persistable.
                if let (Some(from_id), Some(to_id)) =
                    (entity_map.save_id(*from), entity_map.save_id(*to))
                {
                    // Encode save-id as an Entity via from_bits so we can
                    // reuse `entity_pair_map_serde` without a bespoke wire
                    // type. Load rebuilds live Entities via EntityMap.
                    out.insert(
                        (Entity::from_bits(from_id), Entity::from_bits(to_id)),
                        SavedFactionView::from_live(view),
                    );
                }
            }
            SavedFactionRelations { relations: out }
        }),
    })
}

/// Build a [`SavedComponentBag`] from the current component state of `entity`.
fn capture_entity_components(world: &World, entity: Entity) -> SavedComponentBag {
    let mut bag = SavedComponentBag::default();
    let e_ref = world.entity(entity);

    if let Some(p) = e_ref.get::<Position>() {
        bag.position = Some(*p);
    }
    if let Some(m) = e_ref.get::<MovementState>() {
        bag.movement_state = Some(SavedMovementState::from_live(m));
    }
    if let Some(s) = e_ref.get::<StarSystem>() {
        bag.star_system = Some(SavedStarSystem::from_live(s));
    }
    if let Some(p) = e_ref.get::<Planet>() {
        bag.planet = Some(SavedPlanet::from_live(p));
    }
    if let Some(a) = e_ref.get::<SystemAttributes>() {
        bag.system_attributes = Some(SavedSystemAttributes::from_live(a));
    }
    if let Some(s) = e_ref.get::<Sovereignty>() {
        bag.sovereignty = Some(SavedSovereignty::from_live(s));
    }
    // #293: hostile entities serialized via decomposed components.
    if let Some(at) = e_ref.get::<AtSystem>() {
        bag.at_system = Some(SavedAtSystem::from_live(at));
    }
    if let Some(hp) = e_ref.get::<HostileHitpoints>() {
        bag.hostile_hitpoints = Some(SavedHostileHitpoints::from_live(hp));
    }
    if let Some(stats) = e_ref.get::<HostileStats>() {
        bag.hostile_stats = Some(SavedHostileStats::from_live(stats));
    }
    if e_ref.get::<Hostile>().is_some() {
        bag.hostile_marker = Some(SavedHostileMarker);
    }
    if e_ref.get::<ObscuredByGas>().is_some() {
        bag.obscured_by_gas = Some(SavedObscuredByGas);
    }
    if let Some(p) = e_ref.get::<PortFacility>() {
        bag.port_facility = Some(SavedPortFacility::from_live(p));
    }
    if let Some(c) = e_ref.get::<Colony>() {
        bag.colony = Some(SavedColony::from_live(c));
    }
    if let Some(r) = e_ref.get::<ResourceStockpile>() {
        bag.resource_stockpile = Some(SavedResourceStockpile::from_live(r));
    }
    if let Some(r) = e_ref.get::<ResourceCapacity>() {
        bag.resource_capacity = Some(SavedResourceCapacity::from_live(r));
    }
    if let Some(s) = e_ref.get::<Ship>() {
        bag.ship = Some(SavedShip::from_live(s));
    }
    if let Some(s) = e_ref.get::<ShipState>() {
        bag.ship_state = Some(SavedShipState::from_live(s));
    }
    if let Some(h) = e_ref.get::<ShipHitpoints>() {
        bag.ship_hitpoints = Some(SavedShipHitpoints::from_live(h));
    }
    if let Some(c) = e_ref.get::<Cargo>() {
        bag.cargo = Some(SavedCargo::from_live(c));
    }
    if let Some(f) = e_ref.get::<FactionOwner>() {
        bag.faction_owner = Some(SavedFactionOwner::from_live(f));
    }
    if let Some(f) = e_ref.get::<Faction>() {
        bag.faction = Some(SavedFaction::from_live(f));
    }
    if e_ref.get::<Player>().is_some() {
        bag.player = Some(SavedPlayer);
    }
    if let Some(s) = e_ref.get::<StationedAt>() {
        bag.stationed_at = Some(SavedStationedAt::from_live(s));
    }
    if let Some(a) = e_ref.get::<AboardShip>() {
        bag.aboard_ship = Some(SavedAboardShip::from_live(a));
    }
    if let Some(em) = e_ref.get::<Empire>() {
        bag.empire = Some(SavedEmpire::from_live(em));
    }
    if e_ref.get::<PlayerEmpire>().is_some() {
        bag.player_empire = Some(SavedPlayerEmpire);
    }

    // --- Phase B extensions ---

    // Galaxy extensions
    if let Some(a) = e_ref.get::<Anomalies>() {
        bag.anomalies = Some(SavedAnomalies::from_live(a));
    }
    if let Some(r) = e_ref.get::<ForbiddenRegion>() {
        bag.forbidden_region = Some(SavedForbiddenRegion::from_live(r));
    }

    // Colony extensions
    if let Some(b) = e_ref.get::<Buildings>() {
        bag.buildings = Some(SavedBuildings::from_live(b));
    }
    if let Some(q) = e_ref.get::<BuildingQueue>() {
        bag.building_queue = Some(SavedBuildingQueue::from_live(q));
    }
    if let Some(q) = e_ref.get::<BuildQueue>() {
        bag.build_queue = Some(SavedBuildQueue::from_live(q));
    }
    if let Some(sb) = e_ref.get::<SystemBuildings>() {
        bag.system_buildings = Some(SavedSystemBuildings::from_live(sb));
    }
    if let Some(sbq) = e_ref.get::<SystemBuildingQueue>() {
        bag.system_building_queue = Some(SavedSystemBuildingQueue::from_live(sbq));
    }
    if let Some(p) = e_ref.get::<Production>() {
        bag.production = Some(SavedProduction::from_live(p));
    }
    if let Some(f) = e_ref.get::<ProductionFocus>() {
        bag.production_focus = Some(SavedProductionFocus::from_live(f));
    }
    if let Some(j) = e_ref.get::<ColonyJobs>() {
        bag.colony_jobs = Some(SavedColonyJobs::from_live(j));
    }
    if let Some(j) = e_ref.get::<ColonyJobRates>() {
        bag.colony_job_rates = Some(SavedColonyJobRates::from_live(j));
    }
    if let Some(p) = e_ref.get::<ColonyPopulation>() {
        bag.colony_population = Some(SavedColonyPopulation::from_live(p));
    }
    if let Some(m) = e_ref.get::<MaintenanceCost>() {
        bag.maintenance_cost = Some(SavedMaintenanceCost::from_live(m));
    }
    if let Some(f) = e_ref.get::<FoodConsumption>() {
        bag.food_consumption = Some(SavedFoodConsumption::from_live(f));
    }
    if let Some(d) = e_ref.get::<DeliverableStockpile>() {
        bag.deliverable_stockpile = Some(SavedDeliverableStockpile::from_live(d));
    }
    if let Some(c) = e_ref.get::<ColonizationQueue>() {
        bag.colonization_queue = Some(SavedColonizationQueue::from_live(c));
    }
    if let Some(p) = e_ref.get::<PendingColonizationOrder>() {
        bag.pending_colonization_order = Some(SavedPendingColonizationOrder::from_live(p));
    }

    // Empire / player-empire attached
    if let Some(p) = e_ref.get::<AuthorityParams>() {
        bag.authority_params = Some(SavedAuthorityParams::from_live(p));
    }
    if let Some(p) = e_ref.get::<ConstructionParams>() {
        bag.construction_params = Some(SavedConstructionParams::from_live(p));
    }
    if let Some(p) = e_ref.get::<CommsParams>() {
        bag.comms_params = Some(SavedCommsParams::from_live(p));
    }
    if let Some(m) = e_ref.get::<EmpireModifiers>() {
        bag.empire_modifiers = Some(SavedEmpireModifiers::from_live(m));
    }
    if let Some(p) = e_ref.get::<GlobalParams>() {
        bag.global_params = Some(SavedGlobalParams::from_live(p));
    }
    if let Some(f) = e_ref.get::<GameFlags>() {
        bag.game_flags = Some(SavedGameFlags::from_live(f));
    }
    if let Some(f) = e_ref.get::<ScopedFlags>() {
        bag.scoped_flags = Some(SavedScopedFlags::from_live(f));
    }
    if let Some(t) = e_ref.get::<TechTree>() {
        bag.tech_tree = Some(SavedTechTree::from_live(t));
    }
    if let Some(k) = e_ref.get::<TechKnowledge>() {
        bag.tech_knowledge = Some(SavedTechKnowledge::from_live(k));
    }
    if let Some(q) = e_ref.get::<ResearchQueue>() {
        bag.research_queue = Some(SavedResearchQueue::from_live(q));
    }
    if let Some(p) = e_ref.get::<ResearchPool>() {
        bag.research_pool = Some(SavedResearchPool::from_live(p));
    }
    if let Some(r) = e_ref.get::<RecentlyResearched>() {
        bag.recently_researched = Some(SavedRecentlyResearched::from_live(r));
    }
    if let Some(ks) = e_ref.get::<KnowledgeStore>() {
        bag.knowledge_store = Some(SavedKnowledgeStore::from_live(ks));
    }
    if let Some(cl) = e_ref.get::<CommandLog>() {
        bag.command_log = Some(SavedCommandLog::from_live(cl));
    }
    if let Some(p) = e_ref.get::<PendingColonyTechModifiers>() {
        bag.pending_colony_tech_modifiers = Some(SavedPendingColonyTechModifiers::from_live(p));
    }

    // Ship extensions
    if let Some(cq) = e_ref.get::<CommandQueue>() {
        bag.command_queue = Some(SavedCommandQueue::from_live(cq));
    }
    if let Some(sm) = e_ref.get::<ShipModifiers>() {
        bag.ship_modifiers = Some(SavedShipModifiers::from_live(sm));
    }
    if let Some(cr) = e_ref.get::<CourierRoute>() {
        bag.courier_route = Some(SavedCourierRoute::from_live(cr));
    }
    if let Some(sd) = e_ref.get::<SurveyData>() {
        bag.survey_data = Some(SavedSurveyData::from_live(sd));
    }
    if let Some(sr) = e_ref.get::<ScoutReport>() {
        bag.scout_report = Some(SavedScoutReport::from_live(sr));
    }
    if let Some(f) = e_ref.get::<Fleet>() {
        bag.fleet = Some(SavedFleet::from_live(f));
    }
    if let Some(m) = e_ref.get::<FleetMembership>() {
        bag.fleet_membership = Some(SavedFleetMembership::from_live(m));
    }
    if let Some(d) = e_ref.get::<DetectedHostiles>() {
        bag.detected_hostiles = Some(SavedDetectedHostiles::from_live(d));
    }
    if let Some(roe) = e_ref.get::<RulesOfEngagement>() {
        bag.rules_of_engagement = Some(roe.into());
    }

    // Pending command entities
    if let Some(p) = e_ref.get::<PendingShipCommand>() {
        bag.pending_ship_command = Some(SavedPendingShipCommand::from_live(p));
    }
    if let Some(p) = e_ref.get::<PendingDiplomaticAction>() {
        bag.pending_diplomatic_action = Some(SavedPendingDiplomaticAction::from_live(p));
    }
    if let Some(p) = e_ref.get::<PendingCommand>() {
        bag.pending_command = Some(SavedPendingCommand::from_live(p));
    }
    if let Some(p) = e_ref.get::<PendingResearch>() {
        bag.pending_research = Some(SavedPendingResearch::from_live(p));
    }
    if let Some(p) = e_ref.get::<PendingKnowledgePropagation>() {
        bag.pending_knowledge_propagation = Some(SavedPendingKnowledgePropagation::from_live(p));
    }

    // Deep space
    if let Some(s) = e_ref.get::<DeepSpaceStructure>() {
        bag.deep_space_structure = Some(SavedDeepSpaceStructure::from_live(s));
    }
    if let Some(r) = e_ref.get::<FTLCommRelay>() {
        bag.ftl_comm_relay = Some(SavedFTLCommRelay::from_live(r));
    }
    if let Some(h) = e_ref.get::<StructureHitpoints>() {
        bag.structure_hitpoints = Some(SavedStructureHitpoints::from_live(h));
    }
    if let Some(cp) = e_ref.get::<ConstructionPlatform>() {
        bag.construction_platform = Some(SavedConstructionPlatform::from_live(cp));
    }
    if let Some(s) = e_ref.get::<Scrapyard>() {
        bag.scrapyard = Some(SavedScrapyard::from_live(s));
    }
    if let Some(l) = e_ref.get::<LifetimeCost>() {
        bag.lifetime_cost = Some(SavedLifetimeCost::from_live(l));
    }

    bag
}

/// Capture a full [`GameSave`] snapshot of the current world.
///
/// Mutable access is required because Phase A auto-assigns [`SaveId`] to any
/// persistable entity that lacks one.
pub fn capture_save(world: &mut World) -> Result<GameSave, SaveError> {
    assign_save_ids(world);
    let entity_map = build_entity_map(world);

    let resources = capture_resources(world, &entity_map)?;

    let mut entities: Vec<SavedEntity> = Vec::with_capacity(entity_map.len());
    // Iterate over the save_id → entity map so ordering is deterministic by id.
    let mut all: Vec<(u64, Entity)> = Vec::new();
    {
        let mut q = world.query::<(Entity, &SaveId)>();
        for (e, sid) in q.iter(world) {
            all.push((sid.0, e));
        }
    }
    all.sort_by_key(|(id, _)| *id);

    for (save_id, entity) in all {
        let components = capture_entity_components(world, entity);
        entities.push(SavedEntity { save_id, components });
    }

    Ok(GameSave {
        version: SAVE_VERSION,
        scripts_version: SCRIPTS_VERSION.to_string(),
        resources,
        entities,
    })
}

/// Postcard-encode a world snapshot and write to `w`.
pub fn save_game_to_writer<W: Write>(world: &mut World, mut w: W) -> Result<(), SaveError> {
    let save = capture_save(world)?;
    let bytes = postcard::to_stdvec(&save)?;
    w.write_all(&bytes)?;
    Ok(())
}

/// Postcard-encode a world snapshot and write it to `path`. Creates parent
/// directories if needed.
pub fn save_game_to(world: &mut World, path: &Path) -> Result<(), SaveError> {
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent)?;
        }
    }
    let file = std::fs::File::create(path)?;
    save_game_to_writer(world, file)
}
