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
    if let Some(de) = &bag.diplomatic_event {
        ec.insert(de.clone().into_live(map));
    }
    if let Some(di) = &bag.diplomatic_inbox {
        ec.insert(di.clone().into_live(map));
    }
    // #324: Restore Extinct marker on annihilated factions.
    if let Some(ext) = &bag.extinct {
        ec.insert(ext.clone().into_live());
    }
    if bag.player.is_some() {
        ec.insert(crate::player::Player);
    }
    if let Some(r) = &bag.ruler {
        ec.insert(r.clone().into_live(map));
    }
    if let Some(er) = &bag.empire_ruler {
        ec.insert(er.clone().into_live(map));
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
    // #388 (G): Restore DockedAt harbour reference.
    if let Some(bits) = bag.docked_at {
        let harbour = map.entity(bits).unwrap_or(Entity::PLACEHOLDER);
        ec.insert(crate::ship::DockedAt(harbour));
    }
    // Restore SlotAssignment on station ships.
    if let Some(sa) = &bag.slot_assignment {
        ec.insert(sa.clone().into_live());
    }

    // Pending command entities
    if let Some(p) = &bag.pending_ship_command {
        ec.insert(p.clone().into_live(map));
    }
    // #325: PendingDiplomaticAction removed — old saves silently dropped.
    // The `pending_diplomatic_action` field is still deserialized (backward
    // compat) but its contents are not inserted into the entity.
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
    // #409: Destroyed ship registry.
    if let Some(records) = &save.resources.destroyed_ship_registry {
        let mut registry = crate::knowledge::DestroyedShipRegistry::default();
        for r in records {
            registry.records.push(r.clone().into_live(map));
        }
        world.insert_resource(registry);
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
    // Relations must be inserted before freezing, because `set` checks frozen.
    for ((from_bits, to_bits), view) in saved.relations.iter() {
        let from = map
            .entity(from_bits.to_bits())
            .unwrap_or(Entity::PLACEHOLDER);
        let to = map.entity(to_bits.to_bits()).unwrap_or(Entity::PLACEHOLDER);
        // Use direct insert to bypass frozen check during load.
        new_rel
            .relations
            .insert((from, to), view.clone().into_live());
    }
    // #324: Restore frozen state for extinct factions.
    let mut extinct_query = world.query::<(Entity, &crate::faction::Extinct)>();
    let extinct_entities: Vec<Entity> = extinct_query.iter(world).map(|(e, _)| e).collect();
    for e in extinct_entities {
        new_rel.freeze_faction(e);
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

    // #280: Post-load migration — ensure every Colony has a hub/capital in
    // slot 0. Pre-#280 saves have all-empty slot 0; patch those with
    // colony_hub_t1 (or planetary_capital_t3 for capital system colonies).
    migrate_colony_hub_slot_zero(world);

    // #388 (G): Post-load migration — auto-spawn station ships for
    // SystemBuildings that have filled slots with a ship_design_id but
    // no corresponding station Ship entity in the system.
    migrate_station_ships(world);

    // #291: Post-load migration — insert LastDockedSystem for ships that
    // were saved before this component existed.
    migrate_last_docked_system(world);

    Ok(())
}

/// Convenience wrapper that opens `path` for reading and delegates to
/// [`load_game_from_reader`].
pub fn load_game_from(world: &mut World, path: &Path) -> Result<(), LoadError> {
    let file = std::fs::File::open(path)?;
    load_game_from_reader(world, file)
}

/// #280: Post-load migration that inserts a Colony Hub / Planetary Capital in
/// slot 0 of any Colony whose slot 0 is empty. This handles saves produced
/// before the Colony Hub feature was introduced.
///
/// Detection: capital system colonies get `planetary_capital_t3`, all others
/// get `colony_hub_t1`. If a colony already has a hub/capital-type building in
/// slot 0 it is skipped (idempotent).
fn migrate_colony_hub_slot_zero(world: &mut World) {
    use crate::colony::{Buildings, Colony};
    use crate::galaxy::{Planet, StarSystem};
    use crate::scripting::building_api::BuildingId;

    // Collect capital system entities.
    let capital_systems: std::collections::HashSet<bevy::prelude::Entity> = {
        let mut q = world.query::<(Entity, &StarSystem)>();
        q.iter(world)
            .filter(|(_, s)| s.is_capital)
            .map(|(e, _)| e)
            .collect()
    };

    // Collect (colony_entity, planet_entity) for colonies with empty slot 0.
    let mut to_patch: Vec<(Entity, Entity)> = Vec::new();
    {
        let mut q = world.query::<(Entity, &Colony, &Buildings)>();
        for (ce, colony, buildings) in q.iter(world) {
            let slot_0_empty = buildings.slots.first().map(|s| s.is_none()).unwrap_or(true);
            if slot_0_empty {
                to_patch.push((ce, colony.planet));
            }
        }
    }

    if to_patch.is_empty() {
        return;
    }

    // Resolve planet -> system mapping.
    let planet_to_system: std::collections::HashMap<Entity, Entity> = {
        let mut q = world.query::<(Entity, &Planet)>();
        q.iter(world).map(|(e, p)| (e, p.system)).collect()
    };

    let mut migrated = 0usize;
    for (colony_entity, planet_entity) in to_patch {
        let is_capital = planet_to_system
            .get(&planet_entity)
            .is_some_and(|sys| capital_systems.contains(sys));
        let hub_id = if is_capital {
            "planetary_capital_t3"
        } else {
            "colony_hub_t1"
        };
        if let Some(mut buildings) = world.get_mut::<Buildings>(colony_entity) {
            if buildings.slots.is_empty() {
                buildings.slots.push(Some(BuildingId::new(hub_id)));
            } else {
                buildings.slots[0] = Some(BuildingId::new(hub_id));
            }
            migrated += 1;
        }
    }
    if migrated > 0 {
        info!(
            "#280 migration: patched {} colonies with hub/capital in slot 0",
            migrated
        );
    }
}

/// Post-load migration: ensure all station ships have a `SlotAssignment`.
/// Older saves may have station ships (from the #386/#388 migrations) without
/// `SlotAssignment`. This assigns slots based on system membership order.
fn migrate_station_ships(world: &mut World) {
    use crate::colony::SlotAssignment;
    use crate::colony::SystemBuildings;
    use crate::scripting::building_api::BuildingRegistry;
    use crate::ship::{Ship, ShipState};

    // Check if registries exist — they may not in minimal test worlds.
    let has_building_reg = world.get_resource::<BuildingRegistry>().is_some();
    if !has_building_reg {
        return;
    }

    // Build reverse index: design_id → building_id.
    let design_to_building: std::collections::HashSet<String> = {
        let building_registry = world.resource::<BuildingRegistry>();
        building_registry
            .buildings
            .values()
            .filter_map(|def| def.ship_design_id.clone())
            .collect()
    };

    if design_to_building.is_empty() {
        return;
    }

    // Collect system max_slots.
    let system_max_slots: std::collections::HashMap<Entity, usize> = {
        let mut q = world.query::<(Entity, &SystemBuildings)>();
        q.iter(world).map(|(e, sb)| (e, sb.max_slots)).collect()
    };

    // Find station ships without SlotAssignment.
    let mut to_assign: Vec<(Entity, Entity)> = Vec::new(); // (ship_entity, system_entity)
    {
        let mut q = world.query_filtered::<(Entity, &Ship, &ShipState), Without<SlotAssignment>>();
        for (entity, ship, state) in q.iter(world) {
            if !design_to_building.contains(&ship.design_id) {
                continue;
            }
            let system = match state {
                ShipState::InSystem { system } => *system,
                ShipState::Refitting { system, .. } => *system,
                _ => continue,
            };
            to_assign.push((entity, system));
        }
    }

    if to_assign.is_empty() {
        return;
    }

    // Collect already-assigned slots per system.
    let mut system_slots: std::collections::HashMap<Entity, std::collections::HashSet<usize>> =
        std::collections::HashMap::new();
    {
        let mut q = world.query::<(&ShipState, &SlotAssignment)>();
        for (state, slot) in q.iter(world) {
            let system = match state {
                ShipState::InSystem { system } => *system,
                ShipState::Refitting { system, .. } => *system,
                _ => continue,
            };
            system_slots.entry(system).or_default().insert(slot.0);
        }
    }

    let mut migrated = 0usize;
    for (ship_entity, system_entity) in to_assign {
        let max_slots = system_max_slots
            .get(&system_entity)
            .copied()
            .unwrap_or(crate::colony::DEFAULT_SYSTEM_BUILDING_SLOTS);
        let occupied = system_slots.entry(system_entity).or_default();
        let slot = (0..max_slots).find(|i| !occupied.contains(i));
        if let Some(slot_idx) = slot {
            occupied.insert(slot_idx);
            world
                .entity_mut(ship_entity)
                .insert(SlotAssignment(slot_idx));
            migrated += 1;
        } else {
            warn!(
                "migrate_station_ships: no free slot for station ship {:?} at system {:?}",
                ship_entity, system_entity
            );
        }
    }
    if migrated > 0 {
        info!(
            "SlotAssignment migration: assigned {} station ships to slots",
            migrated
        );
    }
}

/// #291: Insert [`LastDockedSystem`] for ships that were saved before this
/// component existed. Ships in `InSystem` get `Some(system)`, all others
/// get `None`.
fn migrate_last_docked_system(world: &mut World) {
    use crate::ship::ShipState;
    use crate::ship::transit_events::LastDockedSystem;

    let mut to_insert: Vec<(Entity, Option<Entity>)> = Vec::new();
    {
        let mut q = world.query_filtered::<(Entity, &ShipState), Without<LastDockedSystem>>();
        for (entity, state) in q.iter(world) {
            let system = match state {
                ShipState::InSystem { system } => Some(*system),
                _ => None,
            };
            to_insert.push((entity, system));
        }
    }
    for (entity, system) in to_insert {
        world.entity_mut(entity).insert(LastDockedSystem(system));
    }
}
