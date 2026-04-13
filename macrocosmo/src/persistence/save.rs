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

use crate::colony::{Colony, LastProductionTick, ResourceCapacity, ResourceStockpile};
use crate::components::{MovementState, Position};
use crate::faction::{FactionOwner, FactionRelations};
use crate::galaxy::{
    GalaxyConfig, HostilePresence, ObscuredByGas, Planet, PortFacility, Sovereignty, StarSystem,
    SystemAttributes,
};
use crate::player::{AboardShip, Empire, Faction, Player, PlayerEmpire, StationedAt};
use crate::scripting::game_rng::GameRng;
use crate::ship::{Cargo, Ship, ShipHitpoints, ShipState};
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
        let mut q = world.query_filtered::<
            Entity,
            Or<(
                With<StarSystem>,
                With<Planet>,
                With<Colony>,
                With<Ship>,
                With<HostilePresence>,
                With<Empire>,
                With<Faction>,
                With<Player>,
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
    if let Some(h) = e_ref.get::<HostilePresence>() {
        bag.hostile_presence = Some(SavedHostilePresence::from_live(h));
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
