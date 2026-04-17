//! Save-game deserialization (#247).
//!
//! Reads a postcard-encoded [`GameSave`] blob, overwrites persistable
//! resources in the Bevy [`World`], despawns existing persistable entities,
//! then spawns fresh entities and re-inserts their components via the
//! [`EntityMap`] rebuilt from save ids (two-pass spawn → map → insert).
//!
//! Contract:
//! - [`SAVE_VERSION`](crate::persistence::SAVE_VERSION) mismatches are a hard
//!   error ([`LoadError::VersionMismatch`]).
//! - `scripts_version` mismatches are warn-logged but loading proceeds — the
//!   live Lua registries (re-derived from `scripts/` at startup) are the
//!   source of truth for content definitions.
//! - Persistent resources not covered by the save (Lua registries,
//!   `BuildingRegistry`, `HullRegistry`, `ModuleRegistry`,
//!   `ShipDesignRegistry`, `StructureRegistry`, `SpeciesRegistry`,
//!   `JobRegistry`, `TechRegistry`, `ScriptEngine`, Bevy internals) are
//!   retained — the load does not touch them. Callers must ensure the App
//!   has already initialised these before calling [`load_game_from`].
//! - Entity references that cannot be resolved (corrupt save) fall back to
//!   `Entity::PLACEHOLDER` so a stray missing id degrades rather than panics.

use bevy::prelude::*;
use std::io::Read;
use std::path::Path;

use crate::colony::LastProductionTick;
use crate::events::EventLog;
use crate::faction::FactionRelations;
use crate::galaxy::GalaxyConfig;
use crate::knowledge::PendingFactQueue;
use crate::notifications::NotificationQueue;
use crate::scripting::game_rng::GameRng;
use crate::technology::TechTree;
use crate::time_system::{GameClock, GameSpeed};

use super::remap::EntityMap;
use super::save::{GameSave, SCRIPTS_VERSION, SaveId, SaveableMarker};
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

    // Phase B — event log (Resource).
    if let Some(log) = &save.resources.event_log {
        // EventLog contains entity references we want to remap after
        // entities spawn; stash a placeholder now and refresh later.
        world.insert_resource(EventLog {
            entries: Vec::new(),
            max_entries: if log.max_entries == 0 {
                50
            } else {
                log.max_entries
            },
        });
    }
    // Phase B — notification queue.
    if let Some(nq) = &save.resources.notification_queue {
        let mut q = NotificationQueue::new();
        if nq.max_items > 0 {
            q.max_items = nq.max_items;
        }
        world.insert_resource(q);
    }
    // Phase B — pending fact queue (placeholder; filled in after entity map).
    if save.resources.pending_fact_queue.is_some() {
        world.insert_resource(PendingFactQueue::default());
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
        let e = world.spawn((SaveId(saved.save_id), SaveableMarker)).id();
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
    if let Some(b) = &bag.biome {
        ec.insert(b.clone().into_live());
    }
    if let Some(a) = &bag.system_attributes {
        ec.insert(a.clone().into_live());
    }
    if let Some(s) = &bag.sovereignty {
        ec.insert(s.clone().into_live(map));
    }
    // #293: Hostile entity decomposed components.
    if let Some(at) = &bag.at_system {
        ec.insert(at.clone().into_live(map));
    }
    if let Some(hp) = &bag.hostile_hitpoints {
        ec.insert(hp.clone().into_live());
    }
    if let Some(stats) = &bag.hostile_stats {
        ec.insert(stats.clone().into_live());
    }
    if bag.hostile_marker.is_some() {
        ec.insert(crate::galaxy::Hostile);
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

    // --- Phase B extensions ---

    // Galaxy extensions
    if let Some(a) = &bag.anomalies {
        ec.insert(a.clone().into_live());
    }
    if let Some(r) = &bag.forbidden_region {
        ec.insert(r.clone().into_live());
    }

    // Colony extensions
    if let Some(b) = &bag.buildings {
        ec.insert(b.clone().into_live());
    }
    if let Some(q) = &bag.building_queue {
        ec.insert(q.clone().into_live());
    }
    if let Some(q) = &bag.build_queue {
        ec.insert(q.clone().into_live());
    }
    if let Some(sb) = &bag.system_buildings {
        ec.insert(sb.clone().into_live());
    }
    if let Some(sbq) = &bag.system_building_queue {
        ec.insert(sbq.clone().into_live());
    }
    if let Some(p) = &bag.production {
        ec.insert(p.clone().into_live());
    }
    if let Some(f) = &bag.production_focus {
        ec.insert(f.clone().into_live());
    }
    if let Some(j) = &bag.colony_jobs {
        ec.insert(j.clone().into_live());
    }
    if let Some(j) = &bag.colony_job_rates {
        ec.insert(j.clone().into_live());
    }
    if let Some(p) = &bag.colony_population {
        ec.insert(p.clone().into_live());
    }
    if let Some(m) = &bag.maintenance_cost {
        ec.insert(m.clone().into_live());
    }
    if let Some(f) = &bag.food_consumption {
        ec.insert(f.clone().into_live());
    }
    if let Some(d) = &bag.deliverable_stockpile {
        ec.insert(d.clone().into_live());
    }
    if let Some(c) = &bag.colonization_queue {
        ec.insert(c.clone().into_live(map));
    }
    if let Some(p) = &bag.pending_colonization_order {
        ec.insert(p.clone().into_live(map));
    }

    // Empire / player-empire attached
    if let Some(p) = &bag.authority_params {
        ec.insert(p.clone().into_live());
    }
    if let Some(p) = &bag.construction_params {
        ec.insert(p.clone().into_live());
    }
    if let Some(p) = &bag.comms_params {
        ec.insert(p.clone().into_live());
    }
    if let Some(m) = &bag.empire_modifiers {
        ec.insert(m.clone().into_live());
    }
    if let Some(p) = &bag.global_params {
        ec.insert(p.clone().into_live());
    }
    if let Some(f) = &bag.game_flags {
        ec.insert(f.clone().into_live());
    }
    if let Some(f) = &bag.scoped_flags {
        ec.insert(f.clone().into_live());
    }
    if let Some(t) = &bag.tech_tree {
        // TechTree is normally populated from Lua; here we restore only the
        // researched set. If the live entity already has a TechTree (added
        // by another load path), merge into it; otherwise attach a minimal
        // tree carrying just the researched ids.
        let tree = t.clone().into_live_minimal();
        ec.insert(tree);
    }
    if let Some(k) = &bag.tech_knowledge {
        ec.insert(k.clone().into_live());
    }
    if let Some(q) = &bag.research_queue {
        ec.insert(q.clone().into_live());
    }
    if let Some(p) = &bag.research_pool {
        ec.insert(p.clone().into_live());
    }
    if let Some(r) = &bag.recently_researched {
        ec.insert(r.clone().into_live());
    }
    if let Some(ks) = &bag.knowledge_store {
        ec.insert(ks.clone().into_live(map));
    }
    if let Some(cl) = &bag.command_log {
        ec.insert(cl.clone().into_live());
    }
    if let Some(p) = &bag.pending_colony_tech_modifiers {
        ec.insert(p.clone().into_live());
    }

    // Ship extensions
    if let Some(cq) = &bag.command_queue {
        ec.insert(cq.clone().into_live(map));
    }
    if let Some(sm) = &bag.ship_modifiers {
        ec.insert(sm.clone().into_live());
    }
    if let Some(cr) = &bag.courier_route {
        ec.insert(cr.clone().into_live(map));
    }
    if let Some(sd) = &bag.survey_data {
        ec.insert(sd.clone().into_live(map));
    }
    if let Some(sr) = &bag.scout_report {
        ec.insert(sr.clone().into_live(map));
    }
    if let Some(f) = &bag.fleet {
        ec.insert(f.clone().into_live(map));
    }
    if let Some(m) = &bag.fleet_members {
        ec.insert(m.clone().into_live(map));
    }
    if let Some(d) = &bag.detected_hostiles {
        ec.insert(d.clone().into_live(map));
    }
    if let Some(roe) = &bag.rules_of_engagement {
        ec.insert(crate::ship::RulesOfEngagement::from(*roe));
    }
    // #296 (S-3): Restore the CoreShip marker on Infrastructure Core ships.
    if bag.core_ship.is_some() {
        ec.insert(crate::ship::CoreShip);
    }
    // #300 (S-6): Restore Defense Fleet marker on fleet entities.
    if let Some(df) = &bag.defense_fleet {
        ec.insert(df.clone().into_live(map));
    }
    // #298 (S-4): Restore ConqueredCore state.
    if let Some(c) = &bag.conquered_core {
        ec.insert(c.clone().into_live(map));
    }

    // Pending command entities
    if let Some(p) = &bag.pending_ship_command {
        ec.insert(p.clone().into_live(map));
    }
    if let Some(p) = &bag.pending_diplomatic_action {
        ec.insert(p.clone().into_live(map));
    }
    if let Some(p) = &bag.pending_command {
        ec.insert(p.clone().into_live(map));
    }
    if let Some(p) = &bag.pending_research {
        ec.insert(p.clone().into_live());
    }
    if let Some(p) = &bag.pending_knowledge_propagation {
        ec.insert(p.clone().into_live(map));
    }

    // Deep space
    if let Some(s) = &bag.deep_space_structure {
        ec.insert(s.clone().into_live(map));
    }
    if let Some(r) = &bag.ftl_comm_relay {
        ec.insert(r.clone().into_live(map));
    }
    if let Some(h) = &bag.structure_hitpoints {
        ec.insert(h.clone().into_live());
    }
    if let Some(cp) = &bag.construction_platform {
        ec.insert(cp.clone().into_live());
    }
    if let Some(s) = &bag.scrapyard {
        ec.insert(s.clone().into_live());
    }
    if let Some(l) = &bag.lifetime_cost {
        ec.insert(l.clone().into_live());
    }
}

/// Apply Phase-B resource-level payloads that reference entities, after the
/// entity map has been built. Overwrites any placeholder inserted in
/// `apply_resources`.
fn apply_deferred_resources(world: &mut World, save: &GameSave, map: &EntityMap) {
    if let Some(log) = &save.resources.event_log {
        world.insert_resource(log.clone().into_live(map));
    }
    if let Some(nq) = &save.resources.notification_queue {
        world.insert_resource(nq.clone().into_live(map));
    }
    if let Some(fq) = &save.resources.pending_fact_queue {
        world.insert_resource(fq.clone().into_live(map));
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
    //    (FactionRelations, Phase-B resources).
    remap_faction_relations(world, &save, &map);
    apply_deferred_resources(world, &save, &map);

    // Suppress "TechTree may be a Resource" variable drift — the resource
    // form is re-derived from Lua scripts on next startup, not saved.
    let _ = world.get_resource::<TechTree>();

    Ok(())
}

/// Convenience wrapper that opens `path` for reading and delegates to
/// [`load_game_from_reader`].
pub fn load_game_from(world: &mut World, path: &Path) -> Result<(), LoadError> {
    let file = std::fs::File::open(path)?;
    load_game_from_reader(world, file)
}
