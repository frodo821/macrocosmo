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
//!    grows the resolved `Region.member_systems` (and inserts
//!    `RegionMembership`) when a colony establishes in a previously-
//!    unowned system, then spawns one
//!    `ShortAgent { scope: ColonizedSystem(_) }` for that system if none
//!    exists yet. The resolved Region is selected via the 3-tier
//!    `resolve_mid_agent_for_system_world` fallback (#471) — for
//!    multi-region empires this routes per-system ShortAgents to the
//!    per-region MidAgent.
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
use crate::ai::mid_adapter::arbitrate;
use crate::ai::npc_decision::ShortAgentTickInputs;
use crate::ai::plugin::AiBusResource;
use crate::ai::short_adapter::BevyShortAgentAdapter;
use crate::ai::short_agent::{ShortAgent, ShortScope};
use crate::ai::short_stance::ShortStanceAgent;
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

/// Resolve the MidAgent that should manage a `(empire, system)` pair
/// using a 3-tier fallback (#471):
///
/// 1. **Fast path** — the system has a `RegionMembership` pointing at a
///    Region owned by `empire`. Production saves with exclusive
///    `Sovereignty` always land here.
/// 2. **Shared-galaxy fallback** — scan `RegionRegistry.by_empire[empire]`
///    for a Region whose `member_systems` contains `system`. Used by
///    test setups where two empires overlap on the same StarSystem
///    (the single-slot `RegionMembership` only tracks one of them, so
///    the second empire's lookup falls through Tier 1).
/// 3. **Primary-region fallback** — `system` is not yet a member of any
///    region for this empire (e.g. brand-new colony in unowned
///    territory before the colony hook grows the region). Use the
///    empire's primary region.
///
/// Returns `None` if the empire has no Region at all, or the resolved
/// Region's `mid_agent` slot is unset.
fn resolve_mid_agent_for_system(
    empire: Entity,
    system: Entity,
    memberships: &Query<&RegionMembership>,
    regions: &Query<&Region>,
    region_registry: Option<&RegionRegistry>,
) -> Option<Entity> {
    // Tier 1: fast path via reverse index.
    if let Ok(rm) = memberships.get(system)
        && let Ok(region) = regions.get(rm.region)
        && region.empire == empire
        && let Some(mid) = region.mid_agent
    {
        return Some(mid);
    }

    // Tier 2: scan this empire's regions for one that owns `system`.
    let registry = region_registry?;
    let empire_regions = registry.by_empire.get(&empire)?;
    for &region_entity in empire_regions {
        if let Ok(region) = regions.get(region_entity)
            && region.member_systems.contains(&system)
            && let Some(mid) = region.mid_agent
        {
            return Some(mid);
        }
    }

    // Tier 3: primary-region fallback.
    let &primary = empire_regions.first()?;
    regions.get(primary).ok().and_then(|r| r.mid_agent)
}

/// `&mut World` variant of [`resolve_mid_agent_for_system`] for the
/// colony hook (which already runs as an exclusive system to grow
/// `Region.member_systems`). Returns `(region_entity, mid_agent)` since
/// the caller needs to mutate the resolved Region directly.
fn resolve_mid_agent_for_system_world(
    world: &World,
    empire: Entity,
    system: Entity,
) -> Option<(Entity, Entity)> {
    // Tier 1: fast path via reverse index.
    if let Some(rm) = world.get::<RegionMembership>(system)
        && let Some(region) = world.get::<Region>(rm.region)
        && region.empire == empire
        && let Some(mid) = region.mid_agent
    {
        return Some((rm.region, mid));
    }

    // Tier 2: scan this empire's regions for one that owns `system`.
    let registry = world.get_resource::<RegionRegistry>()?;
    let empire_regions = registry.by_empire.get(&empire)?;
    for &region_entity in empire_regions {
        if let Some(region) = world.get::<Region>(region_entity)
            && region.member_systems.contains(&system)
            && let Some(mid) = region.mid_agent
        {
            return Some((region_entity, mid));
        }
    }

    // Tier 3: primary-region fallback.
    let &primary = empire_regions.first()?;
    let mid = world.get::<Region>(primary).and_then(|r| r.mid_agent)?;
    Some((primary, mid))
}

/// Spawn-or-backfill system — install one `ShortAgent { scope:
/// Fleet(_) }` per empire-owned `Fleet` whose flagship system is part
/// of a Region.
///
/// Filters by `Without<ShortAgent>` (rather than `Added<Fleet>`) so a
/// Fleet that is spawned **before** its empire's `Region` / `MidAgent`
/// land — typical of integration tests that hand-spawn Empire +
/// Fleet + system in one batch and rely on `npc_decision::backfill_*`
/// to wire the Region the next tick — gets backfilled on the first
/// tick its `RegionMembership` resolves. Without this, the
/// `Added<Fleet>` flag would clear the very first Update after spawn
/// and the fleet would never get a ShortAgent.
///
/// The body is idempotent against re-runs: the `Without<ShortAgent>`
/// filter means a fleet that *did* succeed on its first attempt is
/// excluded, so the per-tick cost is a single tail-iter scan over
/// fleets that have not yet been wired (typically empty after the
/// startup tick).
pub fn spawn_short_agent_for_new_fleets(
    mut commands: Commands,
    new_fleets: Query<(Entity, &Fleet, &FleetMembers), Without<ShortAgent>>,
    ships: Query<&crate::ship::Ship>,
    ship_states: Query<&crate::ship::ShipState>,
    memberships: Query<&RegionMembership>,
    regions: Query<&Region>,
    region_registry: Option<Res<RegionRegistry>>,
    player_empires: Query<&PlayerEmpire>,
    ai_controlled: Query<&super::npc_decision::AiControlled>,
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
        // #471: resolve the MidAgent that owns the flagship's *current*
        // region, with a 3-tier fallback (system-membership → empire
        // region scan → empire primary). This routes per-region
        // ShortAgents to the per-region Mid for multi-region empires,
        // while still degrading gracefully in shared-galaxy test
        // setups where two empires overlap on the same StarSystem (the
        // single-slot `RegionMembership` only tracks one region — Tier
        // 2's empire-scoped scan covers the second empire).
        let mid_agent = resolve_mid_agent_for_system(
            empire,
            system,
            &memberships,
            &regions,
            region_registry.as_deref(),
        );
        let Some(mid_agent) = mid_agent else {
            // Empire's region not yet wired (early frame, or test with
            // no spawn-time region setup). The post-frame backfill
            // will fix this on a later tick — the `Without<ShortAgent>`
            // filter on this query keeps the fleet eligible.
            continue;
        };
        // `auto_managed = true` whenever the AI is allowed to drive
        // this empire — i.e. NPC empires (no `PlayerEmpire`) AND
        // player empires that are explicitly `AiControlled` (test
        // setups using `AiPlayerMode(true)` flow through this path
        // via `mark_player_ai_controlled`). The `MidAgent`-side
        // backfill follows the same rule, so the two layers stay
        // consistent.
        let is_player = player_empires.get(empire).is_ok();
        let is_ai_controlled = ai_controlled.get(empire).is_ok();
        let auto_managed = !is_player || is_ai_controlled;
        commands.entity(fleet_entity).insert(ShortAgent {
            managed_by: mid_agent,
            scope: ShortScope::Fleet(fleet_entity),
            state: macrocosmo_ai::PlanState::default(),
            auto_managed,
        });
    }
}

/// Spawn-or-backfill colony hook — when a colony establishes in a
/// system the empire does not yet hold, grow the empire's *resolved*
/// `Region` (via the 3-tier `resolve_mid_agent_for_system_world`
/// fallback, #471) and install a `ColonizedSystem` `ShortAgent`.
///
/// Filters by "no ColonizedSystem ShortAgent for this system yet"
/// (rather than `Added<Colony>`) for the same reason
/// [`spawn_short_agent_for_new_fleets`] does: a colony spawned
/// **before** backfill wires the empire's Region would otherwise miss
/// the `Added<>` window. The body's existing idempotency guard
/// (`already` check) makes the per-tick scan a no-op once every
/// colony has been wired.
///
/// Uses `&mut World` so we can read `Colony` / `Planet` / `StarSystem`
/// and mutate `Region.member_systems` + `RegionMembership` + spawn the
/// new `ShortAgent` in one pass without query conflicts.
pub fn spawn_short_agent_for_new_colonies(world: &mut World) {
    // Collect every Colony up-front (read-only) so the rest of this
    // body can mutate the world freely. Per-system idempotency is
    // enforced inside the loop.
    let mut newly_added: Vec<Entity> = Vec::new();
    {
        let mut q = world.query_filtered::<Entity, With<Colony>>();
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

        // #471: resolve the colony's home region using the same 3-tier
        // fallback as the fleet hook. For multi-region empires, this
        // routes the ColonizedSystem ShortAgent to the per-region Mid
        // — Tier 1 (system's own RegionMembership) catches re-colonized
        // systems already owned by some region of this empire; Tier 2
        // catches systems already in the empire's `member_systems`
        // (Region.member_systems but RegionMembership pointing
        // elsewhere); Tier 3 falls back to the empire's primary region
        // for brand-new colonies in unowned territory.
        let Some((region_entity, mid_agent)) =
            resolve_mid_agent_for_system_world(world, owner, system)
        else {
            // Empire has no region with a Mid yet — defer to the next
            // backfill pass.
            continue;
        };
        // Grow the owner's Region to include this system if it is not
        // already a member, and refresh the `RegionMembership` reverse
        // index. We deliberately overwrite an existing `RegionMembership`
        // here only when the system was previously unowned-by-anyone —
        // i.e. not when another empire had already claimed it. Today
        // production saves never produce that overlap (Sovereignty is
        // exclusive), so the simpler "insert if absent" path is enough.
        if let Some(mut region) = world.get_mut::<Region>(region_entity) {
            if !region.member_systems.contains(&system) {
                region.member_systems.push(system);
            }
        }
        if world.get::<StarSystem>(system).is_some()
            && world.get::<RegionMembership>(system).is_none()
        {
            world.entity_mut(system).insert(RegionMembership {
                region: region_entity,
            });
        }

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

        // Mirror the fleet-scope path: `auto_managed = true` whenever
        // the AI may drive this empire (NPC, or player-empire with
        // `AiControlled`).
        let is_player = world.get::<PlayerEmpire>(owner).is_some();
        let is_ai_controlled = world
            .get::<super::npc_decision::AiControlled>(owner)
            .is_some();
        let auto_managed = !is_player || is_ai_controlled;
        // Spawn the ShortAgent as a standalone entity (parallel to
        // MidAgent). The `scope: ColonizedSystem(system)` field is the
        // back-reference; we do not insert the component on the
        // StarSystem itself so multi-colony systems can hold a single
        // shared agent independently of system component storage.
        world.spawn(ShortAgent {
            managed_by: mid_agent,
            scope: ShortScope::ColonizedSystem(system),
            state: macrocosmo_ai::PlanState::default(),
            auto_managed,
        });
    }
}

/// Per-tick rehoming for Fleet-scope ShortAgents (#471).
///
/// When a Fleet moves between regions of the same empire (e.g. a
/// courier flies from region A's home to region B's home), its
/// `ShortAgent.managed_by` must follow so the per-region MidAgent owns
/// the fleet's tactics. ColonizedSystem agents don't move, so this
/// system only walks Fleet-scope agents.
///
/// Mid-transit fleets (no `ShipState::InSystem` on any member) keep
/// their current `managed_by` until they land — there's no meaningful
/// region for them while they're between systems.
pub fn rehome_fleet_short_agents(
    mut fleets: Query<(&Fleet, &FleetMembers, &mut ShortAgent)>,
    ships: Query<&crate::ship::Ship>,
    ship_states: Query<&crate::ship::ShipState>,
    memberships: Query<&RegionMembership>,
    regions: Query<&Region>,
    region_registry: Option<Res<RegionRegistry>>,
) {
    for (fleet, members, mut agent) in fleets.iter_mut() {
        // Only Fleet-scope agents — ColonizedSystem agents are tied to
        // a specific StarSystem entity which never moves.
        if !matches!(agent.scope, ShortScope::Fleet(_)) {
            continue;
        }
        let Some(empire) = fleet_empire(members, fleet.flagship, &ships) else {
            continue;
        };
        // Resolve the flagship's *current* system. Probe flagship first,
        // then walk members so multi-ship fleets with a despawned
        // flagship still rehome correctly.
        let mut system: Option<Entity> = None;
        let candidates = fleet.flagship.into_iter().chain(members.iter().copied());
        for ship_entity in candidates {
            if let Ok(crate::ship::ShipState::InSystem { system: s }) = ship_states.get(ship_entity)
            {
                system = Some(*s);
                break;
            }
        }
        let Some(system) = system else {
            // Mid-transit — defer until landing.
            continue;
        };
        let Some(target_mid) = resolve_mid_agent_for_system(
            empire,
            system,
            &memberships,
            &regions,
            region_registry.as_deref(),
        ) else {
            continue;
        };
        if agent.managed_by != target_mid {
            agent.managed_by = target_mid;
        }
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
    short_inputs: Res<ShortAgentTickInputs>,
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

    // Empty fallback slices for ShortGameAdapter scopes that don't
    // need fleet- or system-specific data. Allocated once outside the
    // loop so the per-agent borrows are cheap.
    let empty: Vec<Entity> = Vec::new();
    // Per-tick survey-target claim set: as one Fleet ShortAgent emits
    // a `survey_system` command for a target, every later Fleet
    // ShortAgent in the same empire must skip that target. Pre-PR2d
    // the Mid loop emitted at most one survey per target through its
    // ship-zip-target zip; PR2d's per-fleet split would otherwise let
    // two single-ship fleets in the same empire double-claim the same
    // unsurveyed system within one tick, before
    // `pending_survey_targets` (which is built from outbox + handler
    // markers, both populated _after_ this tick's emits) catches up.
    // Keyed by `(empire, target_system)` so each empire's claims stay
    // independent.
    let mut claimed_survey_targets: std::collections::HashSet<(Entity, Entity)> =
        std::collections::HashSet::new();

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
        let empire = region.empire;
        let faction = to_ai_faction(empire);

        // PR2d: route this agent through `ShortStanceAgent` (Rules 2
        // and 5b). Inputs are sourced from the per-empire scratch
        // populated by `npc_decision_tick` upstream — Bug A dedup
        // (`pending_survey_targets`) and the empire's `member_systems`
        // intersection are already applied there. The Mid-side empire
        // scratch may be missing if the empire's MidAgent was skipped
        // this frame (player-empire with `auto_managed = false`); in
        // that case `idle_surveyors` / `unsurveyed_targets` collapse
        // to empty slices and `ShortStanceAgent` stays silent.
        let inputs = short_inputs.per_empire.get(&empire);
        // Per-fleet view of `unsurveyed_targets`: filter out anything
        // a sibling Fleet in this empire already claimed earlier in
        // the loop. Allocated owned (not borrowed from `inputs`) so
        // the trait method `unsurveyed_targets()` can hand out a
        // slice that lives for the call.
        let unsurveyed_filtered: Vec<Entity>;
        let (idle_surveyors_for_scope, unsurveyed_targets): (&[Entity], &[Entity]) =
            match agent.scope {
                ShortScope::Fleet(fleet) => {
                    let surveyors = inputs
                        .and_then(|i| i.idle_surveyors_by_fleet.get(&fleet))
                        .map(|v| v.as_slice())
                        .unwrap_or(empty.as_slice());
                    let raw_targets = inputs
                        .map(|i| i.unsurveyed_targets.as_slice())
                        .unwrap_or(empty.as_slice());
                    unsurveyed_filtered = raw_targets
                        .iter()
                        .copied()
                        .filter(|t| !claimed_survey_targets.contains(&(empire, *t)))
                        .collect();
                    (surveyors, unsurveyed_filtered.as_slice())
                }
                ShortScope::ColonizedSystem(_) => {
                    unsurveyed_filtered = Vec::new();
                    (empty.as_slice(), empty.as_slice())
                }
            };
        let (free_building_slots, net_production_energy, net_production_food) = match agent.scope {
            ShortScope::ColonizedSystem(_) => inputs
                .map(|i| {
                    (
                        i.free_building_slots,
                        i.net_production_energy,
                        i.net_production_food,
                    )
                })
                .unwrap_or((0.0, 0.0, 0.0)),
            ShortScope::Fleet(_) => (0.0, 0.0, 0.0),
        };
        let adapter = BevyShortAgentAdapter {
            empire,
            scope: agent.scope,
            idle_surveyors: idle_surveyors_for_scope,
            unsurveyed_targets,
            free_building_slots,
            net_production_energy,
            net_production_food,
        };
        let proposals = ShortStanceAgent::decide(&adapter, faction, now);
        // Record this fleet's survey claims so sibling Fleet
        // ShortAgents within the same empire skip the same target
        // later in this loop iteration. We read directly off the
        // proposals (the locality is `Locality::System(_)`) — the
        // arbiter strips locality, so we'd lose this signal once it
        // returned `Vec<Command>`.
        if let ShortScope::Fleet(_) = agent.scope {
            for proposal in &proposals {
                if proposal.command.kind != crate::ai::schema::ids::command::survey_system() {
                    continue;
                }
                if let macrocosmo_ai::Locality::System(sys_ref) = proposal.locality {
                    let target = crate::ai::convert::from_ai_system(sys_ref);
                    claimed_survey_targets.insert((empire, target));
                }
            }
        }
        pending_emit.extend(arbitrate(proposals));

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
