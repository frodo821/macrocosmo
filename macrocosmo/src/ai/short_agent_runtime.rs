//! `ShortAgent` spawn hooks + per-agent decision tick (#449 PR2c).
//!
//! Replaces the deleted `orchestrator_runtime` module: the per-faction
//! `OrchestratorRegistry` / `Orchestrator` cluster has been retired in
//! favor of state-on-Component (`EmpireLongTermState` from PR2a,
//! `MidAgent.state` from PR2b, `ShortAgent.state` here).
//!
//! Lifecycle:
//! 1. [`spawn_short_agent_for_new_fleets`] — `Added<Fleet>` driven; spawns
//!    one `ShortAgent { scope: Fleet(_) }` per empire-owned fleet whose
//!    flagship lives in a system with a `RegionMembership`. Wild /
//!    hostile fleets (`Owner::Neutral` or no `RegionMembership`) are
//!    skipped.
//! 2. [`spawn_short_agent_for_new_colonies`] — `Added<Colony>` driven;
//!    grows the empire's primary `Region.member_systems` (and inserts
//!    `RegionMembership`) when a colony establishes in a previously-
//!    unowned system, then spawns one
//!    `ShortAgent { scope: ColonizedSystem(_) }` for that system if none
//!    exists yet.
//! 3. [`run_short_agents`] — drives every `ShortAgent` whose
//!    `auto_managed = true` through `CampaignReactiveShort::tick`,
//!    using the agent's own `PlanState` as persistent storage. Emitted
//!    commands are pushed onto the AI bus through the same channel the
//!    deleted `run_orchestrators` used.
//! 4. [`despawn_orphaned_short_agents`] — once-per-tick reaper that
//!    removes ShortAgents whose `scope` references a despawned
//!    Fleet/StarSystem, mirroring the `prune_empty_fleets` pattern.

use bevy::prelude::*;
use macrocosmo_ai::{CampaignReactiveShort, Command, ShortContext, ShortTermAgent, ShortTermInput};

use crate::ai::convert::to_ai_faction;
use crate::ai::decomposition_rules::build_default_registry;
use crate::ai::plugin::AiBusResource;
use crate::ai::short_agent::{ShortAgent, ShortScope};
use crate::colony::Colony;
use crate::faction::FactionOwner;
use crate::galaxy::{Planet, StarSystem};
use crate::player::{Empire, PlayerEmpire};
use crate::region::{Region, RegionMembership, RegionRegistry};
use crate::ship::Owner;
use crate::ship::fleet::{Fleet, FleetMembers};
use crate::time_system::GameClock;

/// Resolve the empire that owns a fleet via its flagship.
fn fleet_empire(
    members: &FleetMembers,
    flagship: Option<Entity>,
    ships: &Query<&crate::ship::Ship>,
) -> Option<Entity> {
    let lookup = |e: Entity| {
        ships.get(e).ok().and_then(|s| match s.owner {
            Owner::Empire(emp) => Some(emp),
            Owner::Neutral => None,
        })
    };
    if let Some(f) = flagship {
        if let Some(e) = lookup(f) {
            return Some(e);
        }
    }
    for m in members.iter() {
        if let Some(e) = lookup(*m) {
            return Some(e);
        }
    }
    None
}

/// Resolve `(home_system, mid_agent)` for a fleet whose flagship is in
/// `system`. Returns `None` if no `RegionMembership` exists or the
/// region's `mid_agent` slot is unset.
fn region_for_system(
    system: Entity,
    memberships: &Query<&RegionMembership>,
    regions: &Query<&Region>,
) -> Option<(Entity, Entity)> {
    let region_entity = memberships.get(system).ok()?.region;
    let region = regions.get(region_entity).ok()?;
    let mid = region.mid_agent?;
    Some((region_entity, mid))
}

/// `Added<Fleet>` system — spawn a `ShortAgent { scope: Fleet(_) }` for
/// every newly-created empire-owned fleet whose flagship system is part
/// of a region.
pub fn spawn_short_agent_for_new_fleets(
    mut commands: Commands,
    new_fleets: Query<(Entity, &Fleet, &FleetMembers), Added<Fleet>>,
    ships: Query<&crate::ship::Ship>,
    ship_states: Query<&crate::ship::ShipState>,
    memberships: Query<&RegionMembership>,
    regions: Query<&Region>,
    player_empires: Query<&PlayerEmpire>,
) {
    for (fleet_entity, fleet, members) in &new_fleets {
        let Some(empire) = fleet_empire(members, fleet.flagship, &ships) else {
            // Wild / Owner::Neutral fleet — no ShortAgent.
            continue;
        };
        // Locate the flagship's system. Single-ship "auto" fleets
        // created by `spawn_ship` always have an `InSystem` flagship at
        // spawn time; multi-ship fleets created via `create_fleet` may
        // be assembled in flight, so we just probe each member.
        let mut system: Option<Entity> = None;
        let candidates = fleet.flagship.into_iter().chain(members.iter().copied());
        for ship_entity in candidates {
            if let Ok(state) = ship_states.get(ship_entity) {
                if let crate::ship::ShipState::InSystem { system: s } = state {
                    system = Some(*s);
                    break;
                }
            }
        }
        let Some(system) = system else {
            // Fleet is mid-transit at spawn time — defer; the
            // colony-side spawn / future region-aware system reaper can
            // patch this up if needed. For now we drop the chance to
            // attach a ShortAgent — the alternative (best-guess against
            // home_port) would attach to the wrong region in the rare
            // re-flag case.
            continue;
        };
        let Some((_region_entity, mid_agent)) = region_for_system(system, &memberships, &regions)
        else {
            // Empire's region not yet wired (early frame, or test with
            // no spawn-time region setup). The post-frame backfill (or
            // a future `Added<MidAgent>` reaper) will fix this in the
            // next tick.
            continue;
        };
        let auto_managed = player_empires.get(empire).is_err();
        commands.entity(fleet_entity).insert(ShortAgent {
            managed_by: mid_agent,
            scope: ShortScope::Fleet(fleet_entity),
            state: macrocosmo_ai::PlanState::default(),
            auto_managed,
        });
    }
}

/// `Added<Colony>` system — when a colony establishes in a system the
/// empire does not yet hold, grow the empire's primary `Region` and
/// install a `ColonizedSystem` `ShortAgent`.
///
/// Uses `&mut World` so we can read `Colony` / `Planet` / `StarSystem`
/// and mutate `Region.member_systems` + `RegionMembership` + spawn the
/// new `ShortAgent` in one pass without query conflicts.
pub fn spawn_short_agent_for_new_colonies(world: &mut World) {
    // Collect newly-added colonies up-front (read-only) so the rest of
    // this body can mutate the world freely.
    let mut newly_added: Vec<Entity> = Vec::new();
    {
        let mut q = world.query_filtered::<Entity, Added<Colony>>();
        for e in q.iter(world) {
            newly_added.push(e);
        }
    }
    if newly_added.is_empty() {
        return;
    }

    for colony_entity in newly_added {
        // Resolve owner empire + system from Colony + Planet.
        let Some(owner) = world.get::<FactionOwner>(colony_entity).map(|fo| fo.0) else {
            // Un-tagged colony (legacy save / test). Skip.
            continue;
        };
        let Some(planet_entity) = world.get::<Colony>(colony_entity).map(|c| c.planet) else {
            continue;
        };
        let Some(system) = world.get::<Planet>(planet_entity).map(|p| p.system) else {
            continue;
        };

        // Skip if no `Empire` actually exists for the owner (defensive
        // — production saves always do, but tests can spawn colonies
        // without empires).
        if world.get::<Empire>(owner).is_none() {
            continue;
        }

        // Region resolution. Two cases:
        //   (a) `system` already has `RegionMembership` and the region
        //       belongs to `owner` — reuse it.
        //   (b) Otherwise we extend the empire's primary Region (the
        //       first entry in `RegionRegistry.by_empire`) by pushing
        //       `system` into `member_systems` and inserting a fresh
        //       `RegionMembership`.
        // Both paths require an active MidAgent on the resolved region.
        let existing_membership = world.get::<RegionMembership>(system).map(|m| m.region);
        let (region_entity, mid_agent) = match existing_membership {
            Some(region_entity) => {
                // Confirm same-empire ownership; if a different empire
                // already controls the system, the colony spawn is
                // anomalous (overlapping claims) and we silently skip
                // the ShortAgent for this colony.
                let Some(region) = world.get::<Region>(region_entity) else {
                    continue;
                };
                if region.empire != owner {
                    continue;
                }
                let Some(mid) = region.mid_agent else {
                    continue;
                };
                (region_entity, mid)
            }
            None => {
                // Take the empire's primary region. If none exists yet
                // (initial-region spawn raced this colony), skip — the
                // backfill in `npc_decision::backfill_mid_agents_for_ai_controlled`
                // will pick it up later.
                let Some(region_entity) = world
                    .get_resource::<RegionRegistry>()
                    .and_then(|r| r.by_empire.get(&owner).and_then(|v| v.first().copied()))
                else {
                    continue;
                };
                let Some(mid) = world.get::<Region>(region_entity).and_then(|r| r.mid_agent) else {
                    continue;
                };
                // Mutate the region: append `system` to
                // `member_systems` (idempotent — push only if absent).
                if let Some(mut region) = world.get_mut::<Region>(region_entity) {
                    if !region.member_systems.contains(&system) {
                        region.member_systems.push(system);
                    }
                }
                // Drop the StarSystem-component check (purely defensive
                // — every `system` resolved through Planet.system must
                // carry one), then attach the reverse index.
                if world.get::<StarSystem>(system).is_some() {
                    world.entity_mut(system).insert(RegionMembership {
                        region: region_entity,
                    });
                }
                (region_entity, mid)
            }
        };

        // Idempotent: skip if a `ColonizedSystem` ShortAgent already
        // exists for this system. A `ColonizedSystem(_)` agent is
        // shared across multiple colonies in the same system (the
        // ResourceStockpile-on-StarSystem pattern), so the second
        // colony of a system reuses the first's agent.
        let already = {
            let mut q = world.query::<&ShortAgent>();
            q.iter(world).any(|sa| {
                matches!(sa.scope, ShortScope::ColonizedSystem(s) if s == system)
                    && sa.managed_by == mid_agent
            })
        };
        if already {
            continue;
        }

        let auto_managed = world.get::<PlayerEmpire>(owner).is_none();
        // Spawn the ShortAgent as a standalone entity (parallel to
        // MidAgent). The `scope: ColonizedSystem(system)` field is the
        // back-reference; we do not insert the component on the
        // StarSystem itself so multi-colony systems can hold a single
        // shared agent independently of system component storage.
        let _ = region_entity;
        world.spawn(ShortAgent {
            managed_by: mid_agent,
            scope: ShortScope::ColonizedSystem(system),
            state: macrocosmo_ai::PlanState::default(),
            auto_managed,
        });
    }
}

/// Per-tick driver for every registered `ShortAgent`. Replaces
/// `orchestrator_runtime::run_orchestrators`: instead of one
/// `Orchestrator` per faction, we iterate every `ShortAgent` and call
/// `CampaignReactiveShort::tick` directly, using the agent's own
/// `PlanState` as persistent storage.
///
/// Active campaigns are passed as an empty slice today — campaign
/// state lives on the orchestrator that PR2c retires; PR2d / a later
/// round wires per-MidAgent campaigns through to the relevant Short
/// children. Decomposition is still wired (every Short uses the same
/// game-side registry) so the H1 decomposition e2e test still observes
/// the full primitive chain when its `PlanState` is seeded directly.
///
/// Skips when `GameClock` has not advanced since last call (matches the
/// run-once-per-hexadies cadence the deleted `run_orchestrators` used).
pub fn run_short_agents(
    mut bus: ResMut<AiBusResource>,
    mut agents: Query<(Entity, &mut ShortAgent)>,
    regions: Query<&Region>,
    mid_agents: Query<&super::mid_agent::MidAgent>,
    clock: Res<GameClock>,
    mut last_tick: Local<i64>,
) {
    let now = clock.elapsed;
    if now <= *last_tick {
        return;
    }
    *last_tick = now;

    // Game-side decomposition rules — the same registry the deleted
    // `FactionOrchestrator::new_demo` installed via
    // `Orchestrator::with_decomposition`. Building once per call is
    // cheap (a 2-rule `StaticDecompositionRegistry`).
    let decomp = build_default_registry();
    let mut short = CampaignReactiveShort::new();
    let mut pending_emit: Vec<Command> = Vec::new();

    for (_agent_entity, mut agent) in agents.iter_mut() {
        if !agent.auto_managed {
            continue;
        }
        // Resolve `agent → mid_agent → region → empire` to get the
        // FactionId for the bus emit. Missing back-references mean a
        // partial despawn / load — skip silently.
        let Ok(mid) = mid_agents.get(agent.managed_by) else {
            continue;
        };
        let Ok(region) = regions.get(mid.region) else {
            continue;
        };
        let faction = to_ai_faction(region.empire);

        // ShortContext label: the deleted orchestrator used a single
        // `"faction"` slot; we keep that label here so PlanState slots
        // seeded by tests (or future macros) line up. A future PR may
        // promote this to per-fleet / per-system labels once the rule
        // pipeline migrates onto ShortAgent.
        let ctx = ShortContext::from("faction");
        let input = ShortTermInput {
            bus: &bus.0,
            faction,
            context: ctx,
            active_campaigns: &[],
            now,
            plan_state: &mut agent.state,
            decomp: Some(&decomp),
        };
        let out = short.tick(input);
        pending_emit.extend(out.commands);
    }

    for cmd in pending_emit {
        bus.0.emit_command(cmd);
    }
}

/// Per-tick reaper: despawn `ShortAgent` entities whose scope target
/// has been despawned (Fleet pruned, StarSystem removed, etc.). Keeps
/// the agent set in sync without forcing every despawn site to know
/// about ShortAgent — same approach as `prune_empty_fleets`.
pub fn despawn_orphaned_short_agents(
    mut commands: Commands,
    agents: Query<(Entity, &ShortAgent)>,
    fleets: Query<(), With<Fleet>>,
    systems: Query<(), With<StarSystem>>,
    mid_agents: Query<(), With<super::mid_agent::MidAgent>>,
) {
    for (entity, agent) in &agents {
        // Owning Mid gone → reaper despawns. The next tick's
        // colony/fleet `Added<>` system can re-spawn against a fresh
        // Mid if appropriate.
        if mid_agents.get(agent.managed_by).is_err() {
            commands.entity(entity).despawn();
            continue;
        }
        match agent.scope {
            ShortScope::Fleet(f) => {
                if fleets.get(f).is_err() {
                    commands.entity(entity).despawn();
                }
            }
            ShortScope::ColonizedSystem(s) => {
                if systems.get(s).is_err() {
                    commands.entity(entity).despawn();
                }
            }
        }
    }
}
