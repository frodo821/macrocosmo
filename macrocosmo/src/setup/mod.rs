use bevy::prelude::*;

use crate::amount::Amt;
use crate::colony::{
    BuildQueue, BuildingQueue, Buildings, Colony, FoodConsumption, MaintenanceCost, Production,
    ProductionFocus, ResourceCapacity, ResourceStockpile, SystemBuildingQueue, SystemBuildings,
    DEFAULT_SYSTEM_BUILDING_SLOTS,
};
use crate::communication::CommandLog;
use crate::components::Position;
use crate::condition::ScopedFlags;
use crate::galaxy::{Planet, StarSystem, SystemAttributes};
use crate::knowledge::KnowledgeStore;
use crate::modifier::ModifiedValue;
use crate::observer::{in_observer_mode, not_in_observer_mode};
use crate::player::{Empire, Faction, PlayerEmpire};
use crate::scripting::building_api::BuildingId;
use crate::scripting::faction_api::{lookup_on_game_start, FactionRegistry};
use crate::scripting::game_start_ctx::{
    GameStartActions, GameStartCtx, PlanetAttributesSpec, PlanetRef, SpawnedPlanetSpec,
};
use crate::scripting::ScriptEngine;
use crate::ship::{spawn_ship, Owner};
use crate::ship_design::ShipDesignRegistry;
use crate::species::{ColonyJobs, ColonyPopulation, ColonySpecies};
use crate::technology::{
    EmpireModifiers, GameFlags, GlobalParams, PendingColonyTechModifiers, RecentlyResearched,
    ResearchPool, ResearchQueue, TechTree,
};

pub struct GameSetupPlugin;

impl Plugin for GameSetupPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(
            Startup,
            run_faction_on_game_start
                .after(crate::galaxy::generate_galaxy)
                .after(crate::player::spawn_player_empire)
                .after(crate::colony::spawn_capital_colony)
                .after(crate::scripting::load_all_scripts)
                .after(crate::scripting::load_faction_registry)
                .run_if(not_in_observer_mode),
        )
        .add_systems(
            Startup,
            run_all_factions_on_game_start
                .after(crate::galaxy::generate_galaxy)
                .after(crate::colony::spawn_capital_colony)
                .after(crate::scripting::load_all_scripts)
                .after(crate::scripting::load_faction_registry)
                .run_if(in_observer_mode),
        )
        .add_systems(
            Startup,
            init_observer_view
                .after(run_all_factions_on_game_start)
                .run_if(in_observer_mode),
        );
    }
}

/// Startup system (observer mode only) that sets `ObserverView.viewing` to
/// the first spawned `Empire` entity, so the top-bar selector and Governor
/// tab have a sensible default.
pub fn init_observer_view(
    mut view: ResMut<crate::observer::ObserverView>,
    empires: Query<(Entity, &Faction), With<Empire>>,
) {
    if view.viewing.is_some() {
        return;
    }
    let mut items: Vec<(Entity, String)> = empires
        .iter()
        .map(|(e, f)| (e, f.name.clone()))
        .collect();
    items.sort_by(|a, b| a.1.cmp(&b.1));
    if let Some((e, name)) = items.into_iter().next() {
        view.viewing = Some(e);
        info!("Observer mode: focus set to faction '{}'", name);
    }
}

/// Find the faction id from the player empire's `Faction` component.
fn player_faction_id(world: &mut World) -> Option<String> {
    let mut q = world.query_filtered::<&Faction, With<PlayerEmpire>>();
    let f = q.iter(world).next()?;
    Some(f.id.clone())
}

/// Build the full set of empire-level components for a spawned Empire. This
/// mirrors `crate::player::spawn_player_empire` so observer-mode empires are
/// indistinguishable from the player empire (aside from the `PlayerEmpire`
/// marker).
fn empire_bundle(
    name: String,
    faction_id: String,
    faction_name: String,
) -> impl Bundle {
    (
        Empire { name },
        Faction {
            id: faction_id,
            name: faction_name,
        },
        TechTree::default(),
        ResearchQueue::default(),
        ResearchPool::default(),
        RecentlyResearched::default(),
        crate::colony::AuthorityParams::default(),
        crate::colony::ConstructionParams::default(),
        EmpireModifiers::default(),
        GameFlags::default(),
        GlobalParams::default(),
        KnowledgeStore::default(),
        CommandLog::default(),
        ScopedFlags::default(),
        PendingColonyTechModifiers::default(),
    )
}

/// Startup system for observer mode: iterate every registered NPC faction
/// and spawn a full `Empire` entity for each, then run its `on_game_start`
/// Lua callback. The `PlayerEmpire` marker is NEVER added. Hostile /
/// passive factions (already spawned by `spawn_hostile_factions`) are
/// skipped here.
pub fn run_all_factions_on_game_start(world: &mut World) {
    // Snapshot the registry into a plain Vec so we can drop the borrow
    // before mutating the world.
    let registry_ids: Vec<(String, String, bool)> = {
        let reg = world.resource::<FactionRegistry>();
        reg.factions
            .values()
            .map(|def| (def.id.clone(), def.name.clone(), def.has_on_game_start))
            .collect()
    };

    if registry_ids.is_empty() {
        warn!("Observer mode: FactionRegistry is empty; no NPC empires spawned");
        return;
    }

    // Collect pre-existing Faction entities (e.g. spawned by faction plugin)
    // so we don't double-spawn.
    let existing_by_id: std::collections::HashMap<String, Entity> = {
        let mut q = world.query_filtered::<(Entity, &Faction), Without<Empire>>();
        q.iter(world)
            .map(|(e, f)| (f.id.clone(), e))
            .collect()
    };

    for (faction_id, faction_name, has_callback) in &registry_ids {
        // If a bare Faction entity already exists (without Empire), upgrade
        // it to an Empire by inserting the bundle. Otherwise spawn fresh.
        if let Some(entity) = existing_by_id.get(faction_id) {
            // Leave passive factions (space_creature, ancient_defense) alone
            // — they're added by FactionRelationsPlugin and shouldn't be
            // promoted to full empires.
            let is_passive = faction_id == "space_creature_faction"
                || faction_id == "ancient_defense_faction";
            if is_passive {
                continue;
            }
            // Upgrade: this branch is primarily defensive — in observer mode
            // no prior Faction entity should already exist for these ids.
            world.entity_mut(*entity).insert(empire_bundle(
                faction_name.clone(),
                faction_id.clone(),
                faction_name.clone(),
            ));
            info!(
                "Observer mode: upgraded existing Faction '{}' to full Empire",
                faction_id
            );
        } else {
            world.spawn(empire_bundle(
                faction_name.clone(),
                faction_id.clone(),
                faction_name.clone(),
            ));
            info!("Observer mode: spawned NPC Empire for faction '{}'", faction_id);
        }

        if *has_callback {
            run_on_game_start_for_faction(world, faction_id);
        }
    }
}

/// Shared helper: look up `on_game_start` for the given faction id and,
/// if present, call it and apply the resulting actions. Used by both
/// `run_faction_on_game_start` (player path) and `run_all_factions_on_game_start`
/// (observer path).
fn run_on_game_start_for_faction(world: &mut World, faction_id: &str) {
    let ctx = GameStartCtx::new(faction_id.to_string());
    let actions = {
        let engine = world.resource::<ScriptEngine>();
        let lua = engine.lua();
        let func = match lookup_on_game_start(lua, faction_id) {
            Ok(Some(f)) => f,
            Ok(None) => {
                warn!(
                    "Faction '{}' on_game_start function not found despite registry flag",
                    faction_id
                );
                return;
            }
            Err(e) => {
                warn!("Failed to look up on_game_start for '{}': {e}", faction_id);
                return;
            }
        };
        if let Err(e) = func.call::<()>(ctx.clone()) {
            warn!("on_game_start for '{}' raised an error: {e}", faction_id);
        }
        ctx.take_actions()
    };
    apply_game_start_actions(world, faction_id, actions);
}

/// Startup system that runs the player faction's `on_game_start` Lua callback,
/// then applies the recorded actions to the ECS (buildings, ships, capital marks).
pub fn run_faction_on_game_start(world: &mut World) {
    // Resolve player faction id
    let Some(faction_id) = player_faction_id(world) else {
        warn!("No PlayerEmpire/Faction found; skipping on_game_start");
        return;
    };

    // Verify faction is registered
    let has_callback = {
        let registry = world.resource::<FactionRegistry>();
        match registry.factions.get(&faction_id) {
            Some(def) => def.has_on_game_start,
            None => {
                warn!(
                    "Player faction '{}' not found in FactionRegistry; skipping on_game_start",
                    faction_id
                );
                return;
            }
        }
    };

    if !has_callback {
        info!(
            "Faction '{}' has no on_game_start callback; skipping",
            faction_id
        );
        return;
    }

    // Run the Lua callback to collect actions
    let ctx = GameStartCtx::new(faction_id.clone());
    let actions = {
        let engine = world.resource::<ScriptEngine>();
        let lua = engine.lua();
        let func = match lookup_on_game_start(lua, &faction_id) {
            Ok(Some(f)) => f,
            Ok(None) => {
                warn!(
                    "Faction '{}' on_game_start function not found despite registry flag",
                    faction_id
                );
                return;
            }
            Err(e) => {
                warn!("Failed to look up on_game_start for '{}': {e}", faction_id);
                return;
            }
        };

        if let Err(e) = func.call::<()>(ctx.clone()) {
            warn!("on_game_start for '{}' raised an error: {e}", faction_id);
            // Still apply any actions collected before the error.
        }
        ctx.take_actions()
    };

    apply_game_start_actions(world, &faction_id, actions);
}

/// Resolve a `PlanetRef` to a planet `Entity` using the existing-planets list and the
/// freshly spawned-planets list. Returns `None` if the index is out of range.
fn resolve_planet_ref(
    pref: PlanetRef,
    existing: &[Entity],
    spawned: &[Entity],
) -> Option<Entity> {
    match pref {
        PlanetRef::Existing(idx) => {
            if idx == 0 || idx > existing.len() {
                None
            } else {
                Some(existing[idx - 1])
            }
        }
        PlanetRef::Spawned(idx) => {
            if idx == 0 || idx > spawned.len() {
                None
            } else {
                Some(spawned[idx - 1])
            }
        }
    }
}

/// Build a `SystemAttributes` from a `PlanetAttributesSpec`, falling back to sensible
/// defaults for any field the Lua side did not set.
fn attributes_from_spec(spec: &PlanetAttributesSpec) -> SystemAttributes {
    SystemAttributes {
        habitability: spec.habitability.unwrap_or(0.5),
        mineral_richness: spec.mineral_richness.unwrap_or(0.5),
        energy_potential: spec.energy_potential.unwrap_or(0.5),
        research_potential: spec.research_potential.unwrap_or(0.5),
        max_building_slots: spec.max_building_slots.unwrap_or(4),
    }
}

/// Spawn a new planet entity (Planet + SystemAttributes + Position) under the given system.
fn spawn_planet_entity(
    world: &mut World,
    system_entity: Entity,
    system_pos: Position,
    spec: &SpawnedPlanetSpec,
) -> Entity {
    let attrs = attributes_from_spec(&spec.attributes);
    world
        .spawn((
            Planet {
                name: spec.name.clone(),
                system: system_entity,
                planet_type: spec.planet_type.clone(),
            },
            attrs,
            system_pos,
        ))
        .id()
}

/// Spawn a fresh capital colony scaffold on the given planet. Mirrors the body of
/// `spawn_capital_colony` but without the capital-search logic.
fn spawn_colony_on_planet(world: &mut World, planet_entity: Entity, num_slots: usize) -> Entity {
    world
        .spawn((
            Colony {
                planet: planet_entity,
                population: 100.0,
                growth_rate: 0.01,
            },
            Production {
                minerals_per_hexadies: ModifiedValue::new(Amt::units(5)),
                energy_per_hexadies: ModifiedValue::new(Amt::units(5)),
                research_per_hexadies: ModifiedValue::new(Amt::units(1)),
                food_per_hexadies: ModifiedValue::new(Amt::units(5)),
            },
            BuildQueue { queue: Vec::new() },
            Buildings {
                slots: vec![None; num_slots],
            },
            BuildingQueue::default(),
            ProductionFocus::default(),
            MaintenanceCost::default(),
            FoodConsumption::default(),
            ColonyPopulation {
                species: vec![ColonySpecies {
                    species_id: "human".to_string(),
                    population: 100,
                }],
            },
            ColonyJobs::default(),
            crate::colony::ColonyJobRates::default(),
        ))
        .id()
}

/// Apply the actions recorded by a faction's `on_game_start` callback to the ECS.
/// Operates on the capital StarSystem and its planets.
pub fn apply_game_start_actions(world: &mut World, faction_id: &str, actions: GameStartActions) {
    // Find the capital system entity, its position, and the list of existing planets
    // (sorted by name so PlanetRef::Existing(idx) resolves deterministically).
    let (capital_entity, capital_pos, capital_name, mut existing_planets) = {
        let mut sys_q = world.query::<(Entity, &StarSystem, &Position)>();
        let capital = sys_q
            .iter(world)
            .find(|(_, s, _)| s.is_capital)
            .map(|(e, s, p)| (e, *p, s.name.clone()));
        let Some((entity, pos, name)) = capital else {
            warn!(
                "No capital StarSystem found while applying on_game_start for '{}'",
                faction_id
            );
            return;
        };

        let mut planet_q = world.query::<(Entity, &Planet)>();
        let mut planets: Vec<(Entity, String)> = planet_q
            .iter(world)
            .filter(|(_, p)| p.system == entity)
            .map(|(e, p)| (e, p.name.clone()))
            .collect();
        planets.sort_by(|a, b| a.1.cmp(&b.1));
        let entities: Vec<Entity> = planets.into_iter().map(|(e, _)| e).collect();
        (entity, pos, name, entities)
    };

    // Apply system-level attributes from set_attributes(...)
    if let Some(spec) = &actions.system_attributes {
        if let Some(mut star) = world.get_mut::<StarSystem>(capital_entity) {
            if let Some(name) = &spec.name {
                star.name = name.clone();
            }
            if let Some(st) = &spec.star_type {
                star.star_type = st.clone();
            }
            if let Some(s) = spec.surveyed {
                star.surveyed = s;
            }
        }
    }

    // Apply mark_capital / mark_surveyed (mark_capital is generally redundant since
    // the capital is selected during galaxy generation, but support the API).
    if actions.mark_capital || actions.mark_surveyed {
        if let Some(mut star) = world.get_mut::<StarSystem>(capital_entity) {
            if actions.mark_capital {
                star.is_capital = true;
            }
            if actions.mark_surveyed {
                star.surveyed = true;
            }
        }
    }

    // If clear_planets was requested, despawn all existing planets of the capital
    // AND any existing colonies attached to those planets. The Lua callback is
    // expected to spawn replacement planets via spawn_planet().
    if actions.clear_planets {
        // Collect colonies whose planet entity is in the existing list.
        let colony_entities: Vec<Entity> = {
            let mut q = world.query::<(Entity, &Colony)>();
            q.iter(world)
                .filter(|(_, c)| existing_planets.contains(&c.planet))
                .map(|(e, _)| e)
                .collect()
        };
        for e in colony_entities {
            world.despawn(e);
        }
        for e in &existing_planets {
            world.despawn(*e);
        }
        existing_planets.clear();
    }

    // Spawn new planets requested via spawn_planet(...).
    let mut spawned_planets: Vec<Entity> = Vec::with_capacity(actions.spawned_planets.len());
    for spec in &actions.spawned_planets {
        let e = spawn_planet_entity(world, capital_entity, capital_pos, spec);
        spawned_planets.push(e);
    }

    // Apply per-planet attribute overrides (set_attributes on existing planets or
    // post-spawn tweaks on freshly spawned ones).
    for (pref, spec) in &actions.planet_attribute_overrides {
        let Some(planet_entity) = resolve_planet_ref(*pref, &existing_planets, &spawned_planets)
        else {
            warn!(
                "on_game_start for '{}' set_attributes skipped: planet ref {:?} out of range",
                faction_id, pref
            );
            continue;
        };
        let Some(mut attrs) = world.get_mut::<SystemAttributes>(planet_entity) else {
            warn!(
                "on_game_start for '{}' set_attributes: planet entity has no SystemAttributes",
                faction_id
            );
            continue;
        };
        if let Some(v) = spec.habitability {
            attrs.habitability = v;
        }
        if let Some(v) = spec.mineral_richness {
            attrs.mineral_richness = v;
        }
        if let Some(v) = spec.energy_potential {
            attrs.energy_potential = v;
        }
        if let Some(v) = spec.research_potential {
            attrs.research_potential = v;
        }
        if let Some(v) = spec.max_building_slots {
            attrs.max_building_slots = v;
        }
    }

    // Handle colonize_planet — if no colony exists for the target planet (e.g., the
    // original capital colony was wiped by clear_planets), spawn a fresh scaffold.
    if let Some(pref) = actions.colonize_planet {
        match resolve_planet_ref(pref, &existing_planets, &spawned_planets) {
            None => {
                warn!(
                    "on_game_start for '{}' colonize_planet: planet ref {:?} out of range",
                    faction_id, pref
                );
            }
            Some(planet_entity) => {
                let already_has_colony = {
                    let mut q = world.query::<(Entity, &Colony)>();
                    q.iter(world).any(|(_, c)| c.planet == planet_entity)
                };
                if !already_has_colony {
                    let num_slots = world
                        .get::<SystemAttributes>(planet_entity)
                        .map(|a| a.max_building_slots as usize)
                        .unwrap_or(4);
                    let _ = spawn_colony_on_planet(world, planet_entity, num_slots);
                    // Ensure the system also has resource stockpile / system buildings if
                    // they were not created (shouldn't happen normally but be defensive).
                    if world.get::<ResourceStockpile>(capital_entity).is_none() {
                        world.entity_mut(capital_entity).insert((
                            ResourceStockpile {
                                minerals: Amt::units(500),
                                energy: Amt::units(500),
                                research: Amt::ZERO,
                                food: Amt::units(200),
                                authority: Amt::ZERO,
                            },
                            ResourceCapacity::default(),
                            SystemBuildings {
                                slots: vec![None; DEFAULT_SYSTEM_BUILDING_SLOTS],
                            },
                            SystemBuildingQueue::default(),
                        ));
                    }
                }
            }
        }
    }

    // Apply planet_buildings — each entry is (planet_ref, building_id). Find the
    // colony attached to that planet and add the building to its first empty slot.
    for (pref, building_id) in &actions.planet_buildings {
        let Some(planet_entity) = resolve_planet_ref(*pref, &existing_planets, &spawned_planets)
        else {
            warn!(
                "on_game_start for '{}' add_building skipped: planet ref {:?} out of range",
                faction_id, pref
            );
            continue;
        };
        // Find the Colony entity whose planet matches.
        let colony_entity = {
            let mut q = world.query::<(Entity, &Colony)>();
            q.iter(world)
                .find(|(_, c)| c.planet == planet_entity)
                .map(|(e, _)| e)
        };
        let Some(colony_entity) = colony_entity else {
            warn!(
                "on_game_start for '{}' add_building '{}' skipped: no colony on planet ref {:?}",
                faction_id, building_id, pref
            );
            continue;
        };
        let Some(mut buildings) = world.get_mut::<Buildings>(colony_entity) else {
            warn!(
                "on_game_start for '{}' add_building '{}' skipped: colony has no Buildings component",
                faction_id, building_id
            );
            continue;
        };
        if let Some(slot) = buildings.slots.iter_mut().find(|s| s.is_none()) {
            *slot = Some(BuildingId::new(building_id));
        } else {
            warn!(
                "on_game_start for '{}' add_building '{}': no free planet slot on planet ref {:?}",
                faction_id, building_id, pref
            );
        }
    }

    // Apply system_buildings — add to first empty slot of SystemBuildings on the capital.
    if !actions.system_buildings.is_empty() {
        let Some(mut sys_b) = world.get_mut::<SystemBuildings>(capital_entity) else {
            warn!(
                "on_game_start for '{}': capital has no SystemBuildings component",
                faction_id
            );
            return;
        };
        for building_id in &actions.system_buildings {
            if let Some(slot) = sys_b.slots.iter_mut().find(|s| s.is_none()) {
                *slot = Some(BuildingId::new(building_id));
            } else {
                warn!(
                    "on_game_start for '{}' add system building '{}': no free system slot",
                    faction_id, building_id
                );
            }
        }
    }

    // Spawn ships at the capital. Prefer the Empire tagged with a matching
    // Faction (works in observer mode where no PlayerEmpire exists), fall
    // back to PlayerEmpire, and finally Neutral.
    if !actions.ships.is_empty() {
        let owner = {
            let empire_by_faction: Option<Entity> = {
                let mut q = world.query_filtered::<(Entity, &Faction), With<Empire>>();
                q.iter(world)
                    .find(|(_, f)| f.id == faction_id)
                    .map(|(e, _)| e)
            };
            if let Some(e) = empire_by_faction {
                Owner::Empire(e)
            } else {
                let mut q = world.query_filtered::<Entity, With<PlayerEmpire>>();
                match q.iter(world).next() {
                    Some(e) => Owner::Empire(e),
                    None => {
                        warn!(
                            "No Empire found for faction '{}'; ships will be Neutral",
                            faction_id
                        );
                        Owner::Neutral
                    }
                }
            }
        };
        // Use SystemState to obtain a Commands queue from the world.
        let mut state: bevy::ecs::system::SystemState<(
            Commands,
            Res<ShipDesignRegistry>,
        )> = bevy::ecs::system::SystemState::new(world);
        {
            let (mut commands, registry) = state.get_mut(world);
            for (design_id, name) in &actions.ships {
                spawn_ship(
                    &mut commands,
                    design_id,
                    name.clone(),
                    capital_entity,
                    capital_pos,
                    owner,
                    &registry,
                );
            }
        }
        state.apply(world);
        info!(
            "Spawned {} ships from on_game_start of '{}' at {}",
            actions.ships.len(),
            faction_id,
            capital_name
        );
    }

    info!(
        "Applied on_game_start actions for faction '{}' at capital {}",
        faction_id, capital_name
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::amount::Amt;
    use crate::colony::{
        BuildQueue, BuildingQueue, Colony, FoodConsumption, MaintenanceCost, Production,
        ProductionFocus, ResourceCapacity, ResourceStockpile, SystemBuildingQueue,
        DEFAULT_SYSTEM_BUILDING_SLOTS,
    };
    use crate::components::Position;
    use crate::condition::ScopedFlags;
    use crate::galaxy::{Anomalies, Sovereignty, SystemAttributes, SystemModifiers};
    use crate::knowledge::KnowledgeStore;
    use crate::modifier::ModifiedValue;
    use crate::player::Empire;
    use crate::ship::{Ship, ShipState};
    use crate::ship_design::{ShipDesignDefinition, ShipDesignRegistry};
    use crate::technology::{
        EmpireModifiers, GameFlags, GlobalParams, PendingColonyTechModifiers, RecentlyResearched,
        ResearchPool, ResearchQueue, TechKnowledge, TechTree,
    };

    fn setup_world() -> (World, Entity, Entity) {
        let mut world = World::new();

        // Spawn capital StarSystem with stockpile, system buildings, etc.
        let capital = world
            .spawn((
                StarSystem {
                    name: "Sol".into(),
                    surveyed: true,
                    is_capital: true,
                    star_type: "yellow_dwarf".into(),
                },
                Position::from([0.0, 0.0, 0.0]),
                Sovereignty::default(),
                TechKnowledge::default(),
                SystemModifiers::default(),
                Anomalies::default(),
                ResourceStockpile {
                    minerals: Amt::units(500),
                    energy: Amt::units(500),
                    research: Amt::ZERO,
                    food: Amt::units(200),
                    authority: Amt::ZERO,
                },
                ResourceCapacity::default(),
                SystemBuildings {
                    slots: vec![None; DEFAULT_SYSTEM_BUILDING_SLOTS],
                },
                SystemBuildingQueue::default(),
            ))
            .id();

        // Planet 1 (Sol I) — sorted alphabetically as "Sol I" < "Sol II"
        let planet = world
            .spawn((
                Planet {
                    name: "Sol I".into(),
                    system: capital,
                    planet_type: "terrestrial".into(),
                },
                SystemAttributes {
                    habitability: 1.0,
                    mineral_richness: 0.5,
                    energy_potential: 0.5,
                    research_potential: 0.5,
                    max_building_slots: 5,
                },
                Position::from([0.0, 0.0, 0.0]),
            ))
            .id();

        // Colony with empty buildings
        world.spawn((
            Colony {
                planet,
                population: 100.0,
                growth_rate: 0.01,
            },
            Production {
                minerals_per_hexadies: ModifiedValue::new(Amt::units(5)),
                energy_per_hexadies: ModifiedValue::new(Amt::units(5)),
                research_per_hexadies: ModifiedValue::new(Amt::units(1)),
                food_per_hexadies: ModifiedValue::new(Amt::units(5)),
            },
            BuildQueue { queue: Vec::new() },
            crate::colony::Buildings {
                slots: vec![None; 5],
            },
            BuildingQueue::default(),
            ProductionFocus::default(),
            MaintenanceCost::default(),
            FoodConsumption::default(),
            crate::colony::ColonyJobRates::default(),
        ));

        // Player empire entity (needed for ship owner resolution)
        world.spawn((
            (
                Empire {
                    name: "Test Empire".into(),
                },
                PlayerEmpire,
                Faction {
                    id: "test_faction".into(),
                    name: "Test".into(),
                },
                TechTree::default(),
                ResearchQueue::default(),
                ResearchPool::default(),
                RecentlyResearched::default(),
                crate::colony::AuthorityParams::default(),
                crate::colony::ConstructionParams::default(),
            ),
            (
                EmpireModifiers::default(),
                GameFlags::default(),
                GlobalParams::default(),
                KnowledgeStore::default(),
                crate::communication::CommandLog::default(),
                ScopedFlags::default(),
                PendingColonyTechModifiers::default(),
            ),
        ));

        // Ship design registry with a single explorer design.
        let mut registry = ShipDesignRegistry::default();
        registry.insert(ShipDesignDefinition {
            id: "explorer_mk1".into(),
            name: "Explorer Mk.I".into(),
            description: String::new(),
            hull_id: "corvette".into(),
            modules: Vec::new(),
            can_survey: true,
            can_colonize: false,
            maintenance: Amt::new(0, 500),
            build_cost_minerals: Amt::units(200),
            build_cost_energy: Amt::units(100),
            build_time: 60,
            hp: 50.0,
            sublight_speed: 0.75,
            ftl_range: 10.0,
            revision: 0,
        });
        world.insert_resource(registry);

        (world, capital, planet)
    }

    #[test]
    fn apply_actions_adds_planet_buildings() {
        let (mut world, _capital, planet) = setup_world();

        let mut actions = GameStartActions::default();
        actions
            .planet_buildings
            .push((PlanetRef::Existing(1), "mine".into()));
        actions
            .planet_buildings
            .push((PlanetRef::Existing(1), "power_plant".into()));

        apply_game_start_actions(&mut world, "test_faction", actions);

        // Find the colony and verify its buildings
        let mut q = world.query::<(&Colony, &crate::colony::Buildings)>();
        let mut found = false;
        for (colony, buildings) in q.iter(&world) {
            if colony.planet == planet {
                let names: Vec<String> = buildings
                    .slots
                    .iter()
                    .filter_map(|s| s.as_ref().map(|b| b.0.clone()))
                    .collect();
                assert_eq!(names, vec!["mine".to_string(), "power_plant".to_string()]);
                found = true;
            }
        }
        assert!(found, "colony not found");
    }

    #[test]
    fn apply_actions_adds_system_buildings() {
        let (mut world, capital, _planet) = setup_world();

        let mut actions = GameStartActions::default();
        actions.system_buildings.push("shipyard".into());

        apply_game_start_actions(&mut world, "test_faction", actions);

        let sys_b = world
            .get::<SystemBuildings>(capital)
            .expect("capital has SystemBuildings");
        let names: Vec<String> = sys_b
            .slots
            .iter()
            .filter_map(|s| s.as_ref().map(|b| b.0.clone()))
            .collect();
        assert_eq!(names, vec!["shipyard".to_string()]);
    }

    #[test]
    fn apply_actions_spawns_ships_at_capital() {
        let (mut world, capital, _planet) = setup_world();

        let mut actions = GameStartActions::default();
        actions.ships.push(("explorer_mk1".into(), "Explorer-1".into()));
        actions.ships.push(("explorer_mk1".into(), "Explorer-2".into()));

        apply_game_start_actions(&mut world, "test_faction", actions);

        let mut q = world.query::<(&Ship, &ShipState)>();
        let ships: Vec<_> = q.iter(&world).collect();
        assert_eq!(ships.len(), 2);
        for (ship, state) in &ships {
            assert!(matches!(ship.owner, Owner::Empire(_)));
            match state {
                ShipState::Docked { system } => assert_eq!(*system, capital),
                _ => panic!("Expected Docked state at capital"),
            }
        }
        let names: Vec<String> = ships.iter().map(|(s, _)| s.name.clone()).collect();
        assert!(names.contains(&"Explorer-1".to_string()));
        assert!(names.contains(&"Explorer-2".to_string()));
    }

    #[test]
    fn apply_actions_set_capital_and_surveyed() {
        let (mut world, capital, _planet) = setup_world();

        // Set surveyed=false so we can verify mark_surveyed flips it back on.
        // is_capital must remain true so apply_game_start_actions can locate it.
        if let Some(mut star) = world.get_mut::<StarSystem>(capital) {
            star.surveyed = false;
        }

        let actions = GameStartActions {
            mark_capital: true,
            mark_surveyed: true,
            ..Default::default()
        };
        apply_game_start_actions(&mut world, "test_faction", actions);

        let star = world.get::<StarSystem>(capital).unwrap();
        assert!(star.is_capital);
        assert!(star.surveyed);
    }

    #[test]
    fn apply_actions_oob_planet_index_logs_warning_no_panic() {
        let (mut world, _capital, _planet) = setup_world();
        let mut actions = GameStartActions::default();
        actions
            .planet_buildings
            .push((PlanetRef::Existing(99), "mine".into()));
        // Should not panic — just warn.
        apply_game_start_actions(&mut world, "test_faction", actions);
    }

    #[test]
    fn apply_actions_clear_planets_removes_existing_planets_and_colony() {
        let (mut world, capital, planet) = setup_world();

        let actions = GameStartActions {
            clear_planets: true,
            ..Default::default()
        };
        apply_game_start_actions(&mut world, "test_faction", actions);

        // The original planet entity should be despawned.
        assert!(world.get::<Planet>(planet).is_none());

        // No planets should remain on the capital.
        let mut q = world.query::<&Planet>();
        let remaining = q.iter(&world).filter(|p| p.system == capital).count();
        assert_eq!(remaining, 0);

        // The original colony should also be gone.
        let mut cq = world.query::<&Colony>();
        assert_eq!(cq.iter(&world).count(), 0);
    }

    #[test]
    fn apply_actions_spawn_planet_creates_planet_and_attributes() {
        let (mut world, capital, _planet) = setup_world();

        let actions = GameStartActions {
            clear_planets: true,
            spawned_planets: vec![SpawnedPlanetSpec {
                name: "Earth".into(),
                planet_type: "terrestrial".into(),
                attributes: PlanetAttributesSpec {
                    habitability: Some(1.0),
                    mineral_richness: Some(0.7),
                    energy_potential: Some(0.5),
                    research_potential: Some(0.5),
                    max_building_slots: Some(6),
                },
            }],
            colonize_planet: Some(PlanetRef::Spawned(1)),
            planet_buildings: vec![(PlanetRef::Spawned(1), "mine".to_string())],
            ..Default::default()
        };
        apply_game_start_actions(&mut world, "test_faction", actions);

        // A single planet named "Earth" should belong to the capital.
        let mut q = world.query::<(&Planet, &SystemAttributes)>();
        let mut found_earth = None;
        for (p, attrs) in q.iter(&world) {
            if p.system == capital {
                assert!(found_earth.is_none(), "more than one planet on capital");
                found_earth = Some((p.name.clone(), attrs.clone()));
            }
        }
        let (name, attrs) = found_earth.expect("Earth not spawned");
        assert_eq!(name, "Earth");
        assert!((attrs.habitability - 1.0).abs() < 1e-9);
        assert!((attrs.mineral_richness - 0.7).abs() < 1e-9);
        assert_eq!(attrs.max_building_slots, 6);

        // A fresh colony should exist on the new planet, with the requested building.
        let mut cq = world.query::<(&Colony, &Buildings)>();
        let colonies: Vec<_> = cq.iter(&world).collect();
        assert_eq!(colonies.len(), 1);
        let names: Vec<String> = colonies[0]
            .1
            .slots
            .iter()
            .filter_map(|s| s.as_ref().map(|b| b.0.clone()))
            .collect();
        assert_eq!(names, vec!["mine".to_string()]);
        // Slot count should match planet.max_building_slots
        assert_eq!(colonies[0].1.slots.len(), 6);
    }

    #[test]
    fn apply_actions_planet_set_attributes_overrides_existing() {
        let (mut world, _capital, planet) = setup_world();

        let mut actions = GameStartActions::default();
        actions.planet_attribute_overrides.push((
            PlanetRef::Existing(1),
            PlanetAttributesSpec {
                habitability: Some(0.42),
                research_potential: Some(0.99),
                max_building_slots: Some(8),
                ..Default::default()
            },
        ));
        apply_game_start_actions(&mut world, "test_faction", actions);

        let attrs = world
            .get::<SystemAttributes>(planet)
            .expect("planet has attributes");
        assert!((attrs.habitability - 0.42).abs() < 1e-9);
        assert!((attrs.research_potential - 0.99).abs() < 1e-9);
        assert_eq!(attrs.max_building_slots, 8);
        // Untouched fields keep their original values
        assert!((attrs.mineral_richness - 0.5).abs() < 1e-9);
        assert!((attrs.energy_potential - 0.5).abs() < 1e-9);
    }

    #[test]
    fn apply_actions_system_set_attributes_overrides_star() {
        let (mut world, capital, _planet) = setup_world();

        let actions = GameStartActions {
            system_attributes: Some(crate::scripting::game_start_ctx::SystemAttributesSpec {
                name: Some("Sol".into()),
                star_type: Some("yellow_dwarf".into()),
                surveyed: Some(false),
            }),
            ..Default::default()
        };
        apply_game_start_actions(&mut world, "test_faction", actions);

        let star = world.get::<StarSystem>(capital).unwrap();
        assert_eq!(star.name, "Sol");
        assert_eq!(star.star_type, "yellow_dwarf");
        assert!(!star.surveyed);
    }

    #[test]
    fn apply_actions_full_humanity_flow() {
        let (mut world, capital, _planet) = setup_world();

        // Simulate: clear planets, spawn Earth+Mars, colonize Earth, add buildings,
        // add system shipyard, spawn one ship.
        let actions = GameStartActions {
            clear_planets: true,
            spawned_planets: vec![
                SpawnedPlanetSpec {
                    name: "Earth".into(),
                    planet_type: "terrestrial".into(),
                    attributes: PlanetAttributesSpec {
                        habitability: Some(1.0),
                        mineral_richness: Some(0.7),
                        energy_potential: Some(0.5),
                        research_potential: Some(0.7),
                        max_building_slots: Some(6),
                    },
                },
                SpawnedPlanetSpec {
                    name: "Mars".into(),
                    planet_type: "terrestrial".into(),
                    attributes: PlanetAttributesSpec {
                        habitability: Some(0.4),
                        mineral_richness: Some(0.6),
                        max_building_slots: Some(3),
                        ..Default::default()
                    },
                },
            ],
            colonize_planet: Some(PlanetRef::Spawned(1)),
            planet_buildings: vec![
                (PlanetRef::Spawned(1), "mine".into()),
                (PlanetRef::Spawned(1), "farm".into()),
            ],
            system_buildings: vec!["shipyard".into()],
            ships: vec![("explorer_mk1".into(), "Explorer-1".into())],
            ..Default::default()
        };
        apply_game_start_actions(&mut world, "test_faction", actions);

        // Two planets on the capital
        let mut pq = world.query::<&Planet>();
        let mut planet_names: Vec<String> = pq
            .iter(&world)
            .filter(|p| p.system == capital)
            .map(|p| p.name.clone())
            .collect();
        planet_names.sort();
        assert_eq!(planet_names, vec!["Earth".to_string(), "Mars".to_string()]);

        // One colony, with two buildings
        let mut cq = world.query::<(&Colony, &Buildings)>();
        let colonies: Vec<_> = cq.iter(&world).collect();
        assert_eq!(colonies.len(), 1);
        let names: Vec<String> = colonies[0]
            .1
            .slots
            .iter()
            .filter_map(|s| s.as_ref().map(|b| b.0.clone()))
            .collect();
        assert_eq!(names, vec!["mine".to_string(), "farm".to_string()]);

        // Shipyard added at system level
        let sys_b = world.get::<SystemBuildings>(capital).unwrap();
        let sys_names: Vec<String> = sys_b
            .slots
            .iter()
            .filter_map(|s| s.as_ref().map(|b| b.0.clone()))
            .collect();
        assert_eq!(sys_names, vec!["shipyard".to_string()]);

        // One ship spawned
        let mut sq = world.query::<&Ship>();
        assert_eq!(sq.iter(&world).count(), 1);
    }
}

