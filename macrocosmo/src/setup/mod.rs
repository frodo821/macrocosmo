use bevy::prelude::*;

use crate::amount::Amt;
use crate::colony::{
    BuildQueue, BuildingQueue, Buildings, Colony, FoodConsumption, MaintenanceCost, Production,
    ProductionFocus, ResourceCapacity, ResourceStockpile, SlotAssignment, SystemBuildingQueue,
    SystemBuildings,
};
use crate::communication::CommandLog;
use crate::components::Position;
use crate::condition::ScopedFlags;
use crate::galaxy::{Planet, StarSystem, SystemAttributes};
use crate::game_state::{GameState, GameStatePlugin, LoadSaveRequest};
use crate::knowledge::KnowledgeStore;
use crate::modifier::ModifiedValue;
use crate::observer::{in_observer_mode, not_in_observer_mode};
use crate::player::{Empire, Faction, PlayerEmpire};
use crate::scripting::ScriptEngine;
use crate::scripting::building_api::BuildingId;
use crate::scripting::faction_api::{FactionRegistry, lookup_on_game_start};
use crate::scripting::game_start_ctx::{
    GameStartActions, GameStartCtx, PlanetAttributesSpec, PlanetRef, SpawnedPlanetSpec,
};
use crate::ship::{Owner, Ship, ShipState, spawn_core_ship_from_deliverable, spawn_ship};
use crate::ship_design::ShipDesignRegistry;
use crate::species::{ColonyJobs, ColonyPopulation, ColonySpecies};
use crate::technology::{
    EmpireModifiers, GameFlags, GlobalParams, PendingColonyTechModifiers, RecentlyResearched,
    ResearchPool, ResearchQueue, TechTree,
};

pub struct GameSetupPlugin;

impl Plugin for GameSetupPlugin {
    fn build(&self, app: &mut App) {
        // #439: ensure the `GameState` state machine exists before we
        // register any `OnEnter` handlers / `dispatch_initial_state`
        // below. `GameStatePlugin` guards against double-registering
        // `StatesPlugin` so it is safe to add even when `DefaultPlugins`
        // already installed it.
        if !app.is_plugin_added::<GameStatePlugin>() {
            app.add_plugins(GameStatePlugin);
        }

        // #439 Phase 3: world-spawn systems migrated from Startup to
        // OnEnter(GameState::NewGame). The chain below mirrors the
        // previous Startup ordering; each .after(...) reference points
        // at a system that is also registered on OnEnter(NewGame) in
        // its owning plugin (see e.g. galaxy/player/colony/faction/
        // knowledge/ai/observer plugins).
        //
        // #173 / Flush-deferred-commands notes from the old Startup
        // registration still apply: `run_all_factions_on_game_start`
        // reads `HomeSystemAssignments` which `generate_galaxy` inserts
        // via `Commands`, so ApplyDeferred runs between galaxy build
        // and NPC spawn.
        app.add_systems(
            OnEnter(GameState::NewGame),
            apply_faction_presets.after(crate::player::spawn_player_empire),
        )
        .add_systems(
            OnEnter(GameState::NewGame),
            (
                bevy::ecs::schedule::ApplyDeferred,
                run_faction_on_game_start,
            )
                .chain()
                .after(crate::galaxy::generate_galaxy)
                .after(crate::player::spawn_player_empire)
                .after(crate::colony::spawn_capital_colony)
                .after(apply_faction_presets)
                .run_if(not_in_observer_mode),
        )
        .add_systems(
            OnEnter(GameState::NewGame),
            (
                bevy::ecs::schedule::ApplyDeferred,
                run_all_factions_on_game_start,
            )
                .chain()
                .after(crate::galaxy::generate_galaxy)
                .after(crate::colony::spawn_capital_colony)
                .after(run_faction_on_game_start),
        )
        .add_systems(
            OnEnter(GameState::NewGame),
            init_observer_view
                .after(run_all_factions_on_game_start)
                .run_if(in_observer_mode),
        )
        // Finalize: flip state to InGame. Runs after every other
        // OnEnter(NewGame) system so world construction is complete
        // before tick systems start.
        .add_systems(
            OnEnter(GameState::NewGame),
            finish_new_game_transition
                .after(run_all_factions_on_game_start)
                .after(init_observer_view),
        )
        // #439 Phase 1: flip the state machine into `NewGame` or
        // `LoadingSave` depending on whether a `LoadSaveRequest` was
        // inserted. This stays on Startup — the middle states consume
        // it.
        //
        // Gate on `Bootstrapping` so test harnesses that seed `InGame`
        // via `insert_state(...)` aren't dragged back through the
        // middle states on their first frame.
        .add_systems(
            Startup,
            dispatch_initial_state
                .after(crate::scripting::lifecycle::run_lifecycle_hooks)
                .run_if(in_state(GameState::Bootstrapping)),
        )
        .add_systems(OnEnter(GameState::LoadingSave), perform_load)
        // #439 Phase 4: scene teardown on `OnExit(InGame)`. Runs before
        // any `OnEnter(NewGame)` spawn system when a re-entry transition
        // fires (Bevy's `StateTransition` schedule orders exit-handlers
        // before enter-handlers). `cleanup_ingame_entities` wipes all
        // `SaveableMarker`-tagged entities (the entire game world);
        // `reset_ingame_resources` clears tick-accumulated resources and
        // removes one-shot resources the next `NewGame` spawn chain will
        // re-insert. Chained so resource resets never race with queued
        // entity despawns.
        .add_systems(
            OnExit(GameState::InGame),
            (
                crate::game_state::cleanup_ingame_entities,
                bevy::ecs::schedule::ApplyDeferred,
                crate::game_state::reset_ingame_resources,
            )
                .chain(),
        );
    }
}

/// #439 Phase 1 — Startup terminal system. Decides which middle state to
/// enter once engine bootstrap is complete: if a [`LoadSaveRequest`] was
/// inserted (by a future `--load` flag or the main menu), go to
/// [`GameState::LoadingSave`]; otherwise go to [`GameState::NewGame`].
///
/// Phase 3 will keep this transition trigger but the `OnEnter` handlers
/// will do the heavy lifting instead of the current pass-through stubs.
pub fn dispatch_initial_state(
    mut next: ResMut<NextState<GameState>>,
    load_request: Option<Res<LoadSaveRequest>>,
) {
    if load_request.is_some() {
        next.set(GameState::LoadingSave);
    } else {
        next.set(GameState::NewGame);
    }
}

/// #439 Phase 3 — terminal `OnEnter(NewGame)` system. Runs after the
/// full world-spawn chain (see `GameSetupPlugin::build`) and flips the
/// state to `InGame` so game-tick systems under
/// `run_if(in_state(InGame))` begin running.
pub fn finish_new_game_transition(mut next: ResMut<NextState<GameState>>) {
    next.set(GameState::InGame);
}

/// #439 Phase 3 — `OnEnter(LoadingSave)` system. Decodes the save file
/// referenced by the [`LoadSaveRequest`] resource (inserted by `--load`
/// or a future load-game screen), applies it to the world via
/// [`crate::persistence::load::load_game_from`], and transitions into
/// `GameState::InGame`.
///
/// On failure the world may be in a partially-applied state, so we do
/// **not** continue to `InGame` — instead we fall back to
/// `GameState::NewGame` so the user sees a playable world rather than a
/// broken one, and the decode error is surfaced in the log. A future
/// UI-level "save corrupt" surface (main-menu work) will replace this
/// silent fallback with a user-facing diagnostic.
pub fn perform_load(world: &mut World) {
    let request = match world.get_resource::<LoadSaveRequest>() {
        Some(req) => req.clone(),
        None => {
            warn!("perform_load: no LoadSaveRequest resource; falling back to NewGame");
            world
                .resource_mut::<NextState<GameState>>()
                .set(GameState::NewGame);
            return;
        }
    };
    match crate::persistence::load::load_game_from(world, &request.path) {
        Ok(()) => {
            info!("Loaded save from {:?}", request.path);
            world
                .resource_mut::<NextState<GameState>>()
                .set(GameState::InGame);
        }
        Err(e) => {
            error!("Failed to load save from {:?}: {e}", request.path);
            world
                .resource_mut::<NextState<GameState>>()
                .set(GameState::NewGame);
        }
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
    let mut items: Vec<(Entity, String)> =
        empires.iter().map(|(e, f)| (e, f.name.clone())).collect();
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

/// Startup system that patches existing `Faction` components with preset
/// fields from `FactionDefinition`. This covers factions spawned before the
/// registry was loaded (e.g. the player empire from `spawn_player_empire`
/// and hostile factions from `spawn_hostile_factions`).
pub fn apply_faction_presets(mut factions: Query<&mut Faction>, registry: Res<FactionRegistry>) {
    for mut faction in &mut factions {
        if let Some(def) = registry.factions.get(&faction.id) {
            faction.can_diplomacy = def.can_diplomacy;
            faction.allowed_diplomatic_options =
                def.allowed_diplomatic_options.iter().cloned().collect();
        }
    }
}

/// Build the full set of empire-level components for a spawned Empire. This
/// mirrors `crate::player::spawn_player_empire` so observer-mode empires are
/// indistinguishable from the player empire (aside from the `PlayerEmpire`
/// marker).
fn empire_bundle(
    name: String,
    faction_id: String,
    faction_name: String,
    can_diplomacy: bool,
    allowed_diplomatic_options: std::collections::HashSet<String>,
) -> impl Bundle {
    (
        (
            Empire { name },
            Faction {
                id: faction_id,
                name: faction_name,
                can_diplomacy,
                allowed_diplomatic_options,
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
            crate::knowledge::SystemVisibilityMap::default(),
            CommandLog::default(),
            ScopedFlags::default(),
            PendingColonyTechModifiers::default(),
            // Round 9 PR #1 Step 2: NPC empires now also get a
            // `CommsParams` (previously only the player empire had
            // it — `compute_fact_arrival` for NPC-issued facts in
            // Step 3 needs it) and an empty per-empire
            // `PendingFactQueue`. Step 3 wires production callsites
            // to `record_for` so the queue actually fills.
            crate::empire::CommsParams::default(),
            crate::knowledge::PendingFactQueue::default(),
            // #464: Per-empire faction discovery — every NPC empire
            // builds its own KnownFactions independently.
            crate::faction::KnownFactions::default(),
            // #449 PR2a: empire-wide strategic memory (migrated out of
            // the engine-agnostic `OrchestratorState.long_state`).
            crate::region::EmpireLongTermState::default(),
        ),
    )
}

/// Startup system for both player and observer modes (#173): iterate every
/// registered non-passive faction and spawn a full `Empire` entity for each
/// missing one, then run its `on_game_start` Lua callback. The `PlayerEmpire`
/// marker is NEVER added here — the player empire is spawned earlier by
/// `spawn_player_empire` and skipped below. Hostile / passive factions
/// (already spawned by `spawn_hostile_factions`) are skipped too.
pub fn run_all_factions_on_game_start(world: &mut World) {
    // Snapshot the registry into a plain Vec so we can drop the borrow
    // before mutating the world.
    struct FactionSnapshot {
        id: String,
        name: String,
        has_on_game_start: bool,
        is_passive: bool,
        can_diplomacy: bool,
        allowed_diplomatic_options: std::collections::HashSet<String>,
    }
    let registry_ids: Vec<FactionSnapshot> = {
        let reg = world.resource::<FactionRegistry>();
        reg.factions
            .values()
            .map(|def| FactionSnapshot {
                id: def.id.clone(),
                name: def.name.clone(),
                has_on_game_start: def.has_on_game_start,
                is_passive: def.is_passive,
                can_diplomacy: def.can_diplomacy,
                allowed_diplomatic_options: def
                    .allowed_diplomatic_options
                    .iter()
                    .cloned()
                    .collect(),
            })
            .collect()
    };

    if registry_ids.is_empty() {
        warn!("Setup: FactionRegistry is empty; no NPC empires spawned");
        return;
    }

    // Collect pre-existing bare Faction entities (e.g. spawned by the
    // faction plugin as hostile) so we don't double-spawn.
    let existing_by_id: std::collections::HashMap<String, Entity> = {
        let mut q = world.query_filtered::<(Entity, &Faction), Without<Empire>>();
        q.iter(world).map(|(e, f)| (f.id.clone(), e)).collect()
    };

    // #173: Collect pre-existing Empire entities (the player empire, or any
    // Empire already promoted by a prior run) so we skip them.
    let existing_empire_ids: std::collections::HashSet<String> = {
        let mut q = world.query_filtered::<&Faction, With<Empire>>();
        q.iter(world).map(|f| f.id.clone()).collect()
    };

    for snap in &registry_ids {
        // #173: Skip factions that already have an Empire (e.g. the player
        // empire spawned by `spawn_player_empire`). Don't re-run their
        // `on_game_start` — that is handled by `run_faction_on_game_start`.
        if existing_empire_ids.contains(&snap.id) {
            continue;
        }
        // If a bare Faction entity already exists (without Empire), upgrade
        // it to an Empire by inserting the bundle. Otherwise spawn fresh.
        if let Some(entity) = existing_by_id.get(&snap.id) {
            // Leave passive factions alone — they're added by
            // FactionRelationsPlugin and shouldn't be promoted to full
            // empires. `is_passive` is a preset field from the faction type.
            if snap.is_passive {
                continue;
            }
            // Upgrade: defensive branch — normally no prior Faction entity
            // exists for an empire id that isn't passive.
            world.entity_mut(*entity).insert(empire_bundle(
                snap.name.clone(),
                snap.id.clone(),
                snap.name.clone(),
                snap.can_diplomacy,
                snap.allowed_diplomatic_options.clone(),
            ));
            info!(
                "Setup: upgraded existing Faction '{}' to full Empire",
                snap.id
            );
        } else {
            world.spawn(empire_bundle(
                snap.name.clone(),
                snap.id.clone(),
                snap.name.clone(),
                snap.can_diplomacy,
                snap.allowed_diplomatic_options.clone(),
            ));
            info!("Setup: spawned NPC Empire for faction '{}'", snap.id);
        }

        if snap.has_on_game_start {
            run_on_game_start_for_faction(world, &snap.id);
        }

        // #429: Insert HomeSystem component on the empire entity.
        insert_home_system_on_empire(world, &snap.id);

        // #449 PR2a: spawn the empire's initial Region anchored at its
        // HomeSystem. Must run AFTER `insert_home_system_on_empire`
        // (reads `HomeSystem`) and BEFORE PR2b's MidAgent spawn.
        spawn_initial_region_for_faction(world, &snap.id);

        // #421: Spawn a Ruler entity for this NPC empire.
        spawn_npc_ruler(world, &snap.id, &snap.name);
    }
}

/// #421: Spawn a Ruler entity for a newly created NPC empire. Finds the empire
/// entity by faction id, then picks the first colony's system as the home, or
/// falls back to the global capital system.
fn spawn_npc_ruler(world: &mut World, faction_id: &str, faction_name: &str) {
    use crate::player::{EmpireRuler, Ruler, StationedAt};

    // Find the empire entity for this faction.
    let empire_entity = {
        let mut q = world.query_filtered::<(Entity, &Faction), With<Empire>>();
        q.iter(world)
            .find(|(_, f)| f.id == faction_id)
            .map(|(e, _)| e)
    };
    let Some(empire_entity) = empire_entity else {
        warn!(
            "Setup: could not find Empire for faction '{}' to spawn Ruler",
            faction_id
        );
        return;
    };

    // #429: Find a home system for this empire. Priority:
    // 1. HomeSystem component on the empire entity
    // 2. HomeSystemAssignments resource
    // 3. Colony with FactionOwner → planet → system
    // 4. Global is_capital fallback
    let home_system = world
        .get::<crate::galaxy::HomeSystem>(empire_entity)
        .map(|h| h.0)
        .or_else(|| {
            world
                .get_resource::<crate::galaxy::HomeSystemAssignments>()
                .and_then(|a| a.assignments.get(faction_id).copied())
        })
        .or_else(|| {
            let mut colony_q =
                world.query::<(&crate::colony::Colony, &crate::faction::FactionOwner)>();
            let mut planet_q = world.query::<&crate::galaxy::Planet>();
            let colony_planet = colony_q
                .iter(world)
                .find(|(_, fo)| fo.0 == empire_entity)
                .map(|(c, _)| c.planet);
            colony_planet.and_then(|planet_e| planet_q.get(world, planet_e).ok().map(|p| p.system))
        })
        .or_else(|| {
            let mut sys_q = world.query::<(Entity, &StarSystem)>();
            sys_q
                .iter(world)
                .find(|(_, s)| s.is_capital)
                .map(|(e, _)| e)
        });

    let Some(home_system) = home_system else {
        warn!(
            "Setup: no home system found for faction '{}'; skipping Ruler spawn",
            faction_id
        );
        return;
    };

    let ruler = world
        .spawn((
            Ruler {
                name: faction_name.to_string(),
                empire: empire_entity,
            },
            StationedAt {
                system: home_system,
            },
        ))
        .id();
    world.entity_mut(empire_entity).insert(EmpireRuler(ruler));
    info!("Setup: spawned Ruler for NPC faction '{}'", faction_id);
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

    // #429: Insert HomeSystem component on the player empire entity.
    insert_home_system_on_empire(world, &faction_id);

    // #449 PR2a: spawn the empire's initial Region anchored at its
    // HomeSystem. Must run AFTER `insert_home_system_on_empire` (we
    // read `HomeSystem`) and BEFORE PR2b's MidAgent spawn (which fills
    // `Region.mid_agent`).
    spawn_initial_region_for_faction(world, &faction_id);
}

/// #449 PR2a: spawn the empire's initial `Region` anchored at its
/// `HomeSystem`. Idempotent — if a `RegionMembership` already exists on
/// the home system (e.g. a hot-reloaded scene path), this skips. The
/// `RegionRegistry` resource is created on-demand if missing (defensive:
/// `GameSetupPlugin` does not currently register it as a resource, but
/// the Region spawn path must work even when the registry was not
/// pre-installed by a plugin).
fn spawn_initial_region_for_faction(world: &mut World, faction_id: &str) {
    // Find the empire entity for this faction.
    let empire_entity = {
        let mut q = world.query_filtered::<(Entity, &Faction), With<Empire>>();
        q.iter(world)
            .find(|(_, f)| f.id == faction_id)
            .map(|(e, _)| e)
    };
    let Some(empire) = empire_entity else {
        warn!(
            "Setup: spawn_initial_region: no Empire for faction '{}'",
            faction_id
        );
        return;
    };

    // Read the HomeSystem inserted by `insert_home_system_on_empire`.
    let Some(home_system) = world.get::<crate::galaxy::HomeSystem>(empire).map(|h| h.0) else {
        warn!(
            "Setup: spawn_initial_region: empire '{}' has no HomeSystem; skipping",
            faction_id
        );
        return;
    };

    // Defensive: ensure the registry resource exists. PR2a does not yet
    // have a dedicated `RegionPlugin` so the resource is created here on
    // first use.
    if world
        .get_resource::<crate::region::RegionRegistry>()
        .is_none()
    {
        world.insert_resource(crate::region::RegionRegistry::default());
    }

    // Idempotent: skip if the home system already has a RegionMembership
    // (e.g. re-entry into NewGame).
    if world
        .get::<crate::region::RegionMembership>(home_system)
        .is_some()
    {
        return;
    }

    let _region = crate::region::spawn_initial_region(world, empire, home_system);
}

/// #429: Look up the faction's assigned home system and insert a `HomeSystem`
/// component on the corresponding Empire entity.
fn insert_home_system_on_empire(world: &mut World, faction_id: &str) {
    let home_entity = world
        .get_resource::<crate::galaxy::HomeSystemAssignments>()
        .and_then(|a| a.assignments.get(faction_id).copied());
    let Some(home) = home_entity else {
        return;
    };
    let empire_entity = {
        let mut q = world.query_filtered::<(Entity, &Faction), With<Empire>>();
        q.iter(world)
            .find(|(_, f)| f.id == faction_id)
            .map(|(e, _)| e)
    };
    if let Some(empire) = empire_entity {
        world
            .entity_mut(empire)
            .insert(crate::galaxy::HomeSystem(home));
    }
}

/// Resolve a `PlanetRef` to a planet `Entity` using the existing-planets list and the
/// freshly spawned-planets list. Returns `None` if the index is out of range.
fn resolve_planet_ref(pref: PlanetRef, existing: &[Entity], spawned: &[Entity]) -> Option<Entity> {
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
///
/// #297 (S-2): `faction_entity` is the administrative owner attached as a
/// [`crate::faction::FactionOwner`] component. `None` skips attachment
/// (warn-and-skip defensive pattern), matching `spawn_capital_colony` when
/// no `PlayerEmpire` / matching `Faction` exists.
fn spawn_colony_on_planet(
    world: &mut World,
    planet_entity: Entity,
    num_slots: usize,
    faction_entity: Option<Entity>,
) -> Entity {
    let colony = world
        .spawn((
            Colony {
                planet: planet_entity,
                growth_rate: 0.01,
            },
            // #250: zero-base production; all output comes from building/job
            // modifiers via the sync pipeline.
            Production {
                minerals_per_hexadies: ModifiedValue::new(Amt::ZERO),
                energy_per_hexadies: ModifiedValue::new(Amt::ZERO),
                research_per_hexadies: ModifiedValue::new(Amt::ZERO),
                food_per_hexadies: ModifiedValue::new(Amt::ZERO),
            },
            BuildQueue::default(),
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
                growth_accumulator: 0.0,
            },
            ColonyJobs::default(),
            crate::colony::ColonyJobRates::default(),
        ))
        .id();
    if let Some(empire) = faction_entity {
        world
            .entity_mut(colony)
            .insert(crate::faction::FactionOwner(empire));
    }
    colony
}

/// Apply the actions recorded by a faction's `on_game_start` callback to the ECS.
/// Operates on the faction's assigned home system (from `HomeSystemAssignments`)
/// and its planets. Falls back to the global `is_capital` system when no assignment
/// exists (backward compatibility).
pub fn apply_game_start_actions(world: &mut World, faction_id: &str, actions: GameStartActions) {
    // #429: Look up the faction's assigned home system from HomeSystemAssignments.
    // Fall back to the global is_capital system for backward compat.
    let (capital_entity, capital_pos, capital_name, mut existing_planets) = {
        let home_entity = world
            .get_resource::<crate::galaxy::HomeSystemAssignments>()
            .and_then(|assignments| assignments.assignments.get(faction_id).copied());

        let mut sys_q = world.query::<(Entity, &StarSystem, &Position)>();
        let capital = if let Some(home) = home_entity {
            sys_q
                .iter(world)
                .find(|(e, _, _)| *e == home)
                .map(|(e, s, p)| (e, *p, s.name.clone()))
        } else {
            sys_q
                .iter(world)
                .find(|(_, s, _)| s.is_capital)
                .map(|(e, s, p)| (e, *p, s.name.clone()))
        };
        let Some((entity, pos, name)) = capital else {
            warn!(
                "No home system found while applying on_game_start for '{}'",
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

    // #297 (S-2): Resolve the faction entity for this `faction_id` so
    // colonies / SystemBuildings / ships spawned by this on_game_start
    // callback all carry the same `FactionOwner`. Precedence matches the
    // existing ship-owner lookup below (L619-641): prefer an Empire whose
    // Faction id matches, fall back to PlayerEmpire, otherwise None.
    let faction_entity: Option<Entity> = {
        let mut q = world.query_filtered::<(Entity, &Faction), With<Empire>>();
        let by_faction = q
            .iter(world)
            .find(|(_, f)| f.id == faction_id)
            .map(|(e, _)| e);
        if by_faction.is_some() {
            by_faction
        } else {
            let mut pe_q = world.query_filtered::<Entity, With<PlayerEmpire>>();
            pe_q.iter(world).next()
        }
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
                    // #297 (S-2): Pass the resolved faction entity so the
                    // new Colony carries a `FactionOwner`. `None` skips
                    // attachment with a warning (see helper doc).
                    let _ = spawn_colony_on_planet(world, planet_entity, num_slots, faction_entity);
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
                            SystemBuildings::default(),
                            SystemBuildingQueue::default(),
                        ));
                        // #297 (S-2): SystemBuildings on the StarSystem entity
                        // implies administrative ownership — attach
                        // FactionOwner on the same entity (plan §2C).
                        if let Some(empire) = faction_entity {
                            world
                                .entity_mut(capital_entity)
                                .insert(crate::faction::FactionOwner(empire));
                        }
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

    // Apply system_buildings — spawn station ships with SlotAssignment for each
    // building. The BuildingRegistry is consulted to find ship_design_id.
    if !actions.system_buildings.is_empty() {
        let max_slots = world
            .get::<SystemBuildings>(capital_entity)
            .map(|sb| sb.max_slots)
            .unwrap_or(0);
        if max_slots == 0 {
            warn!(
                "on_game_start for '{}': capital has no SystemBuildings component",
                faction_id
            );
        } else {
            // Count already-occupied slots (existing station ships at this system).
            let mut occupied_slots: std::collections::HashSet<usize> =
                std::collections::HashSet::new();
            {
                let mut q = world.query::<(&Ship, &ShipState, &SlotAssignment)>();
                for (_, state, slot) in q.iter(world) {
                    if let ShipState::InSystem { system } = state {
                        if *system == capital_entity {
                            occupied_slots.insert(slot.0);
                        }
                    }
                }
            }

            // For each system building, find the ship_design_id and spawn.
            let building_to_design: std::collections::HashMap<String, (String, String)> =
                if let Some(building_registry) =
                    world.get_resource::<crate::colony::BuildingRegistry>()
                {
                    building_registry
                        .buildings
                        .values()
                        .filter_map(|def| {
                            def.ship_design_id
                                .as_ref()
                                .map(|did| (def.id.clone(), (did.clone(), def.name.clone())))
                        })
                        .collect()
                } else {
                    std::collections::HashMap::new()
                };

            let mut next_slot = 0usize;
            let owner = faction_entity.map(Owner::Empire).unwrap_or(Owner::Neutral);

            for building_id in &actions.system_buildings {
                // Find next free slot.
                while next_slot < max_slots && occupied_slots.contains(&next_slot) {
                    next_slot += 1;
                }
                if next_slot >= max_slots {
                    warn!(
                        "on_game_start for '{}' add system building '{}': no free system slot",
                        faction_id, building_id
                    );
                    break;
                }
                let slot_idx = next_slot;
                occupied_slots.insert(slot_idx);
                next_slot += 1;

                // Spawn station ship if there's a design mapping.
                if let Some((design_id, station_name)) = building_to_design.get(building_id) {
                    let mut state: bevy::ecs::system::SystemState<(
                        Commands,
                        Res<ShipDesignRegistry>,
                    )> = bevy::ecs::system::SystemState::new(world);
                    {
                        let (mut commands, registry) = state.get_mut(world);
                        let ship_entity = spawn_ship(
                            &mut commands,
                            design_id,
                            station_name.clone(),
                            capital_entity,
                            capital_pos,
                            owner,
                            &registry,
                        );
                        commands
                            .entity(ship_entity)
                            .insert(SlotAssignment(slot_idx));
                    }
                    state.apply(world);
                } else {
                    warn!(
                        "on_game_start for '{}': system building '{}' has no ship_design_id mapping",
                        faction_id, building_id
                    );
                }
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
        let mut state: bevy::ecs::system::SystemState<(Commands, Res<ShipDesignRegistry>)> =
            bevy::ecs::system::SystemState::new(world);
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

    // #299 (S-5): Spawn an Infrastructure Core at the capital when requested
    // via `ctx.system:spawn_core()`. This uses the canonical
    // `infrastructure_core_v1` design and the same owner resolution as ships.
    if actions.spawn_core {
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
                            "No Empire found for faction '{}'; Core will be Neutral (no sovereignty)",
                            faction_id
                        );
                        Owner::Neutral
                    }
                }
            }
        };
        let core_pos = crate::galaxy::system_inner_orbit_position(capital_entity, world);
        let mut state: bevy::ecs::system::SystemState<(Commands, Res<ShipDesignRegistry>)> =
            bevy::ecs::system::SystemState::new(world);
        {
            let (mut commands, registry) = state.get_mut(world);
            spawn_core_ship_from_deliverable(
                &mut commands,
                "infrastructure_core_v1",
                format!("Infrastructure Core ({})", faction_id),
                capital_entity,
                Position::from(core_pos),
                owner,
                &registry,
            );
        }
        state.apply(world);
        info!(
            "Spawned Infrastructure Core from on_game_start of '{}' at {}",
            faction_id, capital_name
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
                SystemBuildings::default(),
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
                growth_rate: 0.01,
            },
            Production {
                minerals_per_hexadies: ModifiedValue::new(Amt::units(5)),
                energy_per_hexadies: ModifiedValue::new(Amt::units(5)),
                research_per_hexadies: ModifiedValue::new(Amt::units(1)),
                food_per_hexadies: ModifiedValue::new(Amt::units(5)),
            },
            BuildQueue::default(),
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
                Faction::new("test_faction", "Test"),
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
                crate::knowledge::SystemVisibilityMap::default(),
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
            is_direct_buildable: true,
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

        // Insert a BuildingRegistry with "shipyard" that has a ship_design_id.
        let mut building_registry = crate::colony::BuildingRegistry::default();
        building_registry.insert(crate::scripting::building_api::BuildingDefinition {
            id: "shipyard".to_string(),
            name: "Shipyard".to_string(),
            description: String::new(),
            minerals_cost: Amt::ZERO,
            energy_cost: Amt::ZERO,
            build_time: 30,
            maintenance: Amt::ZERO,
            production_bonus_minerals: Amt::ZERO,
            production_bonus_energy: Amt::ZERO,
            production_bonus_research: Amt::ZERO,
            production_bonus_food: Amt::ZERO,
            modifiers: Vec::new(),
            is_system_building: true,
            capabilities: std::collections::HashMap::new(),
            upgrade_to: Vec::new(),
            is_direct_buildable: true,
            prerequisites: None,
            on_built: None,
            on_upgraded: None,
            dismantlable: true,
            ship_design_id: Some("station_shipyard_v1".to_string()),
            colony_slots: None,
        });
        world.insert_resource(building_registry);

        // Add a matching ship design.
        let mut design_reg = world.resource_mut::<ShipDesignRegistry>();
        design_reg.insert(ShipDesignDefinition {
            id: "station_shipyard_v1".into(),
            name: "Shipyard Station".into(),
            description: String::new(),
            hull_id: "station".into(),
            modules: Vec::new(),
            can_survey: false,
            can_colonize: false,
            maintenance: Amt::ZERO,
            build_cost_minerals: Amt::ZERO,
            build_cost_energy: Amt::ZERO,
            build_time: 0,
            hp: 100.0,
            sublight_speed: 0.0,
            ftl_range: 0.0,
            revision: 0,
            is_direct_buildable: false,
        });

        let mut actions = GameStartActions::default();
        actions.system_buildings.push("shipyard".into());

        apply_game_start_actions(&mut world, "test_faction", actions);

        // Verify a station ship with SlotAssignment(0) was spawned at the capital.
        let mut q = world.query::<(&Ship, &ShipState, &SlotAssignment)>();
        let found = q.iter(&world).any(|(ship, state, slot)| {
            ship.design_id == "station_shipyard_v1"
                && matches!(state, ShipState::InSystem { system } if *system == capital)
                && slot.0 == 0
        });
        assert!(
            found,
            "Station ship with SlotAssignment(0) should be spawned at capital"
        );
    }

    #[test]
    fn apply_actions_spawns_ships_at_capital() {
        let (mut world, capital, _planet) = setup_world();

        let mut actions = GameStartActions::default();
        actions
            .ships
            .push(("explorer_mk1".into(), "Explorer-1".into()));
        actions
            .ships
            .push(("explorer_mk1".into(), "Explorer-2".into()));

        apply_game_start_actions(&mut world, "test_faction", actions);

        let mut q = world.query::<(&Ship, &ShipState)>();
        let ships: Vec<_> = q.iter(&world).collect();
        assert_eq!(ships.len(), 2);
        for (ship, state) in &ships {
            assert!(matches!(ship.owner, Owner::Empire(_)));
            match state {
                ShipState::InSystem { system } => assert_eq!(*system, capital),
                _ => panic!("Expected InSystem state at capital"),
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

        // Shipyard station ship at system level (if BuildingRegistry is set up)
        // In this test, BuildingRegistry might not have "shipyard" with
        // ship_design_id, so the station ship might not be spawned.
        // Verify SystemBuildings still exists with max_slots.
        let sys_b = world.get::<SystemBuildings>(capital).unwrap();
        assert!(sys_b.max_slots > 0);

        // Ships spawned (explorer + possibly station ship)
        let mut sq = world.query::<&Ship>();
        assert!(sq.iter(&world).count() >= 1);
    }
}
