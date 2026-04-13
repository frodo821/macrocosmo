//! Save-game deserialization (#247, Phase A).
//!
//! Reads a postcard-encoded [`GameSave`] blob, overwrites persistable
//! resources in the Bevy [`World`], despawns existing persistable entities,
//! then spawns fresh entities and re-inserts their components via the
//! [`EntityMap`] rebuilt from save ids.
//!
//! Phase A semantics:
//! - `scripts_version` mismatch is warn-logged but loading proceeds.
//! - Persistent resources not covered by the save (Lua registries,
//!   BuildingRegistry, ShipDesignRegistry, ScriptEngine, Bevy internals) are
//!   retained — the load does not touch them.
//! - Entity references that cannot be resolved (corrupt save) fall back to
//!   `Entity::PLACEHOLDER` so a stray missing id degrades rather than panics.

use bevy::prelude::*;
use std::io::Read;
use std::path::Path;

use crate::colony::LastProductionTick;
use crate::faction::FactionRelations;
use crate::galaxy::GalaxyConfig;
use crate::scripting::game_rng::GameRng;
use crate::time_system::{GameClock, GameSpeed};

use super::remap::EntityMap;
use super::save::{GameSave, SaveId, SaveableMarker, SCRIPTS_VERSION};
use super::savebag::SavedComponentBag;

#[derive(Debug)]
pub enum LoadError {
    Io(std::io::Error),
    Postcard(postcard::Error),
    VersionMismatch { saved: u32, expected: u32 },
}

impl std::fmt::Display for LoadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LoadError::Io(e) => write!(f, "I/O error: {e}"),
            LoadError::Postcard(e) => write!(f, "postcard decode error: {e}"),
            LoadError::VersionMismatch { saved, expected } => write!(
                f,
                "save version {saved} is not supported by this build (expected {expected})"
            ),
        }
    }
}
impl std::error::Error for LoadError {}
impl From<std::io::Error> for LoadError {
    fn from(e: std::io::Error) -> Self {
        LoadError::Io(e)
    }
}
impl From<postcard::Error> for LoadError {
    fn from(e: postcard::Error) -> Self {
        LoadError::Postcard(e)
    }
}

// ---------------------------------------------------------------------------
// Load pipeline
// ---------------------------------------------------------------------------

/// Overwrite persistable resources with values from `save`.
fn apply_resources(world: &mut World, save: &GameSave) -> Result<(), LoadError> {
    // Clock — preserve the internal accumulator by constructing fresh.
    world.insert_resource(GameClock::new(save.resources.game_clock_elapsed));

    // Speed.
    world.insert_resource(GameSpeed {
        hexadies_per_second: save.resources.game_speed_hexadies_per_second,
        previous_speed: save.resources.game_speed_previous,
    });

    // Last production tick.
    world.insert_resource(LastProductionTick(save.resources.last_production_tick));

    // Galaxy config (only if present).
    if let Some(cfg) = &save.resources.galaxy_config {
        world.insert_resource(GalaxyConfig {
            radius: cfg.radius,
            num_systems: cfg.num_systems,
        });
    }

    // RNG (only if present). Restore continues the deterministic stream.
    if let Some(rng_snapshot) = &save.resources.game_rng {
        let restored: GameRng = rng_snapshot.restore()?;
        world.insert_resource(restored);
    }

    // Faction relations.
    if let Some(rel) = &save.resources.faction_relations {
        // Skip remap for now — we'll update these after entity_map is built.
        // Placeholder so the resource exists.
        let mut live = FactionRelations::new();
        // We'll rebuild this properly in a second pass in `load_game_from_reader`
        // after the EntityMap is available. For now stash raw saved views.
        for ((from_bits_a, to_bits_b), view) in rel.relations.iter() {
            // Keep the from_bits interpretation — will be remapped below. Since
            // we haven't rebuilt the map yet, we store the raw `Entity` value
            // (which is actually the *save id* encoded as an Entity).
            live.set(*from_bits_a, *to_bits_b, view.clone().into_live());
        }
        world.insert_resource(live);
    }

    Ok(())
}

/// Despawn every entity currently tagged with [`SaveableMarker`] (i.e. one
/// that was previously loaded or auto-tagged on save). This is the selective
/// despawn required by the spec — persistent non-game resources survive.
fn despawn_saveable_entities(world: &mut World) {
    let to_despawn: Vec<Entity> = {
        let mut q = world.query_filtered::<Entity, With<SaveableMarker>>();
        q.iter(world).collect()
    };
    for e in to_despawn {
        if let Ok(ec) = world.get_entity_mut(e) {
            ec.despawn();
        }
    }
}

/// Spawn fresh entities for each [`SavedEntity`], build the [`EntityMap`],
/// then insert all of their components in a second pass (so intra-save
/// references can be resolved).
fn spawn_entities_and_remap(world: &mut World, save: &GameSave) -> EntityMap {
    let mut map = EntityMap::new();

    // First pass: spawn empties and populate the map.
    let mut staged: Vec<(Entity, &SavedComponentBag, u64)> =
        Vec::with_capacity(save.entities.len());
    for saved in &save.entities {
        let e = world
            .spawn((SaveId(saved.save_id), SaveableMarker))
            .id();
        map.insert(saved.save_id, e);
        staged.push((e, &saved.components, saved.save_id));
    }

    // Second pass: insert the actual components, resolving entity refs.
    for (entity, bag, _save_id) in staged {
        apply_component_bag(world, entity, bag, &map);
    }

    map
}

/// Insert every populated component from `bag` onto `entity`, mapping save
/// ids back to live entities via `map`.
fn apply_component_bag(
    world: &mut World,
    entity: Entity,
    bag: &SavedComponentBag,
    map: &EntityMap,
) {
    let Ok(mut ec) = world.get_entity_mut(entity) else {
        return;
    };

    if let Some(p) = &bag.position {
        ec.insert(*p);
    }
    if let Some(m) = &bag.movement_state {
        ec.insert(m.clone().into_live(map));
    }
    if let Some(s) = &bag.star_system {
        ec.insert(s.clone().into_live());
    }
    if let Some(p) = &bag.planet {
        ec.insert(p.clone().into_live(map));
    }
    if let Some(a) = &bag.system_attributes {
        ec.insert(a.clone().into_live());
    }
    if let Some(s) = &bag.sovereignty {
        ec.insert(s.clone().into_live(map));
    }
    if let Some(h) = &bag.hostile_presence {
        ec.insert(h.clone().into_live(map));
    }
    if bag.obscured_by_gas.is_some() {
        ec.insert(crate::galaxy::ObscuredByGas);
    }
    if let Some(p) = &bag.port_facility {
        ec.insert(p.clone().into_live(map));
    }
    if let Some(c) = &bag.colony {
        ec.insert(c.clone().into_live(map));
    }
    if let Some(r) = &bag.resource_stockpile {
        ec.insert(r.clone().into_live());
    }
    if let Some(r) = &bag.resource_capacity {
        ec.insert(r.clone().into_live());
    }
    if let Some(s) = &bag.ship {
        ec.insert(s.clone().into_live(map));
    }
    if let Some(s) = &bag.ship_state {
        ec.insert(s.clone().into_live(map));
    }
    if let Some(h) = &bag.ship_hitpoints {
        ec.insert(h.clone().into_live());
    }
    if let Some(c) = &bag.cargo {
        ec.insert(c.clone().into_live());
    }
    if let Some(f) = &bag.faction_owner {
        ec.insert(f.into_live(map));
    }
    if let Some(f) = &bag.faction {
        ec.insert(f.clone().into_live());
    }
    if bag.player.is_some() {
        ec.insert(crate::player::Player);
    }
    if let Some(s) = &bag.stationed_at {
        ec.insert(s.clone().into_live(map));
    }
    if let Some(a) = &bag.aboard_ship {
        ec.insert(a.clone().into_live(map));
    }
    if let Some(em) = &bag.empire {
        ec.insert(em.clone().into_live());
    }
    if bag.player_empire.is_some() {
        ec.insert(crate::player::PlayerEmpire);
    }
}

/// Rebuild [`FactionRelations`] with the freshly-allocated entities. The
/// saved map keys are `(save_id, save_id)` pairs encoded as `Entity::from_bits`,
/// which we rewrite to the live entities.
fn remap_faction_relations(world: &mut World, save: &GameSave, map: &EntityMap) {
    let Some(saved) = save.resources.faction_relations.as_ref() else {
        return;
    };
    let mut new_rel = FactionRelations::new();
    for ((from_bits, to_bits), view) in saved.relations.iter() {
        let from = map
            .entity(from_bits.to_bits())
            .unwrap_or(Entity::PLACEHOLDER);
        let to = map.entity(to_bits.to_bits()).unwrap_or(Entity::PLACEHOLDER);
        new_rel.set(from, to, view.clone().into_live());
    }
    world.insert_resource(new_rel);
}

/// Full load pipeline: decode, apply resources, despawn-then-respawn
/// persistable entities, and remap cross-entity references.
pub fn load_game_from_reader<R: Read>(world: &mut World, mut r: R) -> Result<(), LoadError> {
    let mut bytes = Vec::new();
    r.read_to_end(&mut bytes)?;
    let save: GameSave = postcard::from_bytes(&bytes)?;

    if save.version != super::save::SAVE_VERSION {
        return Err(LoadError::VersionMismatch {
            saved: save.version,
            expected: super::save::SAVE_VERSION,
        });
    }

    if save.scripts_version != SCRIPTS_VERSION {
        warn!(
            "scripts_version mismatch: saved {} vs current {}, continuing",
            save.scripts_version, SCRIPTS_VERSION
        );
    }

    // 1. Overwrite persistent resources (clock, speed, last tick, galaxy cfg,
    //    rng, faction_relations — though relations are rewritten below once
    //    the entity map is available).
    apply_resources(world, &save)?;

    // 2. Despawn previously-persistent entities to make room for the restored
    //    ones. This leaves Lua registries, BuildingRegistry, ScriptEngine,
    //    etc. untouched because those are resources, not entities.
    despawn_saveable_entities(world);

    // 3. Spawn fresh entities and remap intra-save references.
    let map = spawn_entities_and_remap(world, &save);

    // 4. Final remap pass for resources whose values carry entity references
    //    (FactionRelations).
    remap_faction_relations(world, &save, &map);

    Ok(())
}

/// Convenience wrapper that opens `path` for reading and delegates to
/// [`load_game_from_reader`].
pub fn load_game_from(world: &mut World, path: &Path) -> Result<(), LoadError> {
    let file = std::fs::File::open(path)?;
    load_game_from_reader(world, file)
}
