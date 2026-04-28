//! NPC decision tick — hook point for pluggable per-faction AI policies (#173).
//!
//! `AiPlugin` registers [`npc_decision_tick`] under [`AiTickSet::Reason`].
//! Each tick the system builds a per-empire [`NpcContext`], wraps it in a
//! [`super::mid_adapter::BevyMidGameAdapter`], and routes the decision
//! through [`super::mid_stance::MidStanceAgent::decide`]. All 8 rules
//! (attack, survey, colonize, research, shipyard, slot fill, fleet
//! composition, retreat, fortify) live in the layered Mid agent.
//!
//! Scope note: this module intentionally carries **no** dependency on the
//! optional `macrocosmo_ai::mock` feature. The dev-dependency in
//! `macrocosmo/Cargo.toml` activates `mock` for the integration test
//! binary only, so callers of the production game crate never pay for the
//! feature.
//!
//! See `docs/plan-173-npc-empire-mock-ai.md` for the rollout plan.
//!
//! [`AiTickSet::Reason`]: super::AiTickSet::Reason

use bevy::prelude::*;

use crate::ai::assignments::{AssignmentKind, AssignmentTarget, PendingAssignment};
use crate::ai::command_outbox::AiCommandOutbox;
use crate::ai::convert::{from_ai_system, to_ai_faction};
use crate::ai::plugin::AiBusResource;
use crate::ai::schema::ids::command as cmd_ids;
use crate::knowledge::KnowledgeStore;
use crate::player::{AboardShip, Empire, EmpireRuler, Faction, PlayerEmpire, Ruler, StationedAt};
use crate::technology::ResearchQueue;
use crate::time_system::GameClock;

/// Marker component: this empire's decisions are made by the AI policy.
/// Applied to NPC empires automatically, and optionally to the player
/// empire when `--ai-player` is passed or `AiPlayerMode(true)` is set.
#[derive(Component, Reflect)]
#[reflect(Component)]
pub struct AiControlled;

/// Resource that opts the player empire into AI control.
/// Default is `false` — normal gameplay where the player makes decisions.
/// Set to `true` to let the AI policy drive the player empire.
#[derive(Resource, Debug, Clone, Copy, Default, Reflect)]
#[reflect(Resource)]
pub struct AiPlayerMode(pub bool);

/// System that marks all NPC empires (those with `Empire` but without
/// `PlayerEmpire`) with `AiControlled`. Runs every frame to catch newly
/// spawned empires.
pub fn mark_npc_empires_ai_controlled(
    mut commands: Commands,
    empires: Query<Entity, (With<Empire>, Without<PlayerEmpire>, Without<AiControlled>)>,
) {
    for entity in &empires {
        commands.entity(entity).insert(AiControlled);
    }
}

/// System that marks the player empire with `AiControlled` when
/// `AiPlayerMode(true)` is set.
pub fn mark_player_ai_controlled(
    mut commands: Commands,
    mode: Res<AiPlayerMode>,
    player: Query<Entity, (With<PlayerEmpire>, Without<AiControlled>)>,
) {
    if mode.0 {
        for entity in &player {
            commands.entity(entity).insert(AiControlled);
        }
    }
}

/// #449 PR2b: backfill a default `Region` + `MidAgent` pair for every
/// `AiControlled` empire that does not yet own one. The production
/// spawn path (`crate::setup::spawn_initial_region_for_faction`) builds
/// these eagerly during `OnEnter(NewGame)`, but several integration
/// tests (e.g. `ai_npc_*`, `ai_player_e2e`, `ai_command_lightspeed`)
/// hand-spawn `Empire` entities without going through `GameSetupPlugin`
/// — those tests would otherwise see `npc_decision_tick` skip every
/// empire because the per-MidAgent loop has nothing to iterate. The
/// backfill keeps test setups working without each test needing to
/// know about the Region/MidAgent split.
///
/// Production setups never trigger this code path (the spawn pipeline
/// already populated `Region.mid_agent`), so the cost is exactly the
/// per-frame query and a no-op iter for fully-spawned worlds.
pub fn backfill_mid_agents_for_ai_controlled(world: &mut World) {
    // Collect empires that need a backfill: AiControlled, but not in
    // any RegionRegistry entry. Snapshot before mutating to avoid
    // query-during-mutation.
    let registry_known: std::collections::HashSet<Entity> = world
        .get_resource::<crate::region::RegionRegistry>()
        .map(|r| r.by_empire.keys().copied().collect())
        .unwrap_or_default();
    let needs_backfill: Vec<(Entity, bool)> = {
        let mut q = world
            .query_filtered::<(Entity, Option<&PlayerEmpire>), (With<Empire>, With<AiControlled>)>(
            );
        q.iter(world)
            .filter(|(e, _)| !registry_known.contains(e))
            .map(|(e, p)| (e, p.is_some()))
            .collect()
    };
    if needs_backfill.is_empty() {
        return;
    }

    // Defensive: ensure the registry resource exists.
    if world
        .get_resource::<crate::region::RegionRegistry>()
        .is_none()
    {
        world.insert_resource(crate::region::RegionRegistry::default());
    }

    for (empire, is_player) in needs_backfill {
        // Pick a home system. Test setups don't always insert
        // `HomeSystem`; in that case fall back to "every visible
        // StarSystem belongs to this empire's region" — same scope
        // the legacy per-empire decision path used. The first owned
        // colony's system is the next-best signal; if neither exists,
        // we collect every StarSystem entity in the world and use
        // them as the implicit region scope.
        let home_system = world
            .get::<crate::galaxy::HomeSystem>(empire)
            .map(|h| h.0)
            .or_else(|| {
                let mut colony_q =
                    world.query::<(&crate::colony::Colony, &crate::faction::FactionOwner)>();
                let mut planet_q = world.query::<&crate::galaxy::Planet>();
                let colony_planet = colony_q
                    .iter(world)
                    .find(|(_, fo)| fo.0 == empire)
                    .map(|(c, _)| c.planet);
                colony_planet
                    .and_then(|planet_e| planet_q.get(world, planet_e).ok().map(|p| p.system))
            });

        let member_systems: Vec<Entity> = {
            let mut q = world.query_filtered::<Entity, With<crate::galaxy::StarSystem>>();
            q.iter(world).collect()
        };
        if member_systems.is_empty() {
            // No systems exist yet — defer to a later frame.
            continue;
        }
        // Skip if a Region already exists owned by this empire — a
        // previous backfill or the production spawn path created
        // one. Looking at `Region.empire` directly (rather than the
        // per-system `RegionMembership` reverse index) avoids false
        // positives in test setups where multiple empires share the
        // same star systems: each gets its own Region, and the
        // capital-only `RegionMembership` we attach below is enough
        // to satisfy the invariant for production callers.
        let already_has_region = {
            let mut q = world.query::<&crate::region::Region>();
            q.iter(world).any(|r| r.empire == empire)
        };
        if already_has_region {
            continue;
        }

        let capital = home_system.unwrap_or(member_systems[0]);
        let region = world
            .spawn(crate::region::Region {
                empire,
                member_systems: member_systems.clone(),
                capital_system: capital,
                mid_agent: None,
            })
            .id();
        // RegionMembership reverse index: only attach if the capital
        // does not already carry one (a sibling empire's backfill
        // may have claimed it first in shared-galaxy tests). The
        // per-system membership invariant is best-effort for the
        // backfill path — production callers maintain it strictly
        // via `spawn_initial_region`.
        if world
            .get::<crate::region::RegionMembership>(capital)
            .is_none()
        {
            world
                .entity_mut(capital)
                .insert(crate::region::RegionMembership { region });
        }
        world
            .resource_mut::<crate::region::RegionRegistry>()
            .by_empire
            .entry(empire)
            .or_default()
            .push(region);

        // Backfill always sets `auto_managed = true` — by the time
        // this system runs the empire is already `AiControlled`,
        // which is the upstream "AI may drive this" signal. The
        // production spawn path keeps the player-empire default at
        // `false` so the player retains manual control until they
        // opt in via the #452 UI; tests opting into AiPlayerMode
        // implicitly want the player MidAgent to tick too.
        let _ = is_player;
        let mid_agent = world
            .spawn(super::mid_agent::MidAgent {
                region,
                state: macrocosmo_ai::MidTermState::default(),
                auto_managed: true,
            })
            .id();
        if let Some(mut region_comp) = world.get_mut::<crate::region::Region>(region) {
            region_comp.mid_agent = Some(mid_agent);
        }
    }
}

/// Per-ship summary extracted from ECS for the NPC policy.
pub struct ShipInfo {
    pub entity: Entity,
    pub design_id: String,
    /// The system the ship is currently docked at, or `None` if in transit.
    pub system: Option<Entity>,
    /// `true` when the ship is `InSystem` with an empty command queue.
    pub is_idle: bool,
    pub can_survey: bool,
    pub can_colonize: bool,
    /// `true` when the ship is not a dedicated survey/colony vessel — i.e.
    /// it can participate in combat.
    pub is_combat: bool,
    pub ftl_range: f64,
}

/// Read-only context data extracted from ECS for the NPC policy.
///
/// This keeps the policy trait free of Bevy `Query` types, making it
/// testable without a full Bevy app.
pub struct NpcContext {
    /// Systems with hostile entities present (from KnowledgeStore).
    pub hostile_systems: Vec<Entity>,
    /// Known systems that have not yet been surveyed.
    pub unsurveyed_systems: Vec<Entity>,
    /// Surveyed systems that are not yet colonized (potential colony targets).
    pub colonizable_systems: Vec<Entity>,
    /// All ships owned by the empire being decided for.
    pub ships: Vec<ShipInfo>,
    /// `true` when the empire has an active research target in its queue.
    pub is_researching: bool,
    /// The Ruler entity for this empire, if one exists.
    pub ruler_entity: Option<Entity>,
    /// The system the Ruler is currently stationed at.
    pub ruler_system: Option<Entity>,
    /// Whether the Ruler is currently aboard a ship.
    pub ruler_aboard: bool,
}

/// System run under [`AiTickSet::Reason`](super::AiTickSet::Reason):
/// walk every empire marked [`AiControlled`] and route the per-empire
/// decision through [`super::mid_stance::MidStanceAgent::decide`].
/// NPC empires are auto-marked by [`mark_npc_empires_ai_controlled`].
/// The player empire is also marked when [`AiPlayerMode`]`(true)` is set.
/// Tracks the last game tick at which AI decisions were made, so the
/// policy runs once per hexadies advance, not every render frame.
#[derive(Resource, Default, Reflect)]
#[reflect(Resource)]
pub struct LastAiDecisionTick(pub i64);

/// Rank candidate unsurveyed systems by "accessibility" — an approximation
/// of travel time that's more useful than raw distance.
///
/// FTL routing ([`crate::ship::movement::plan_ftl_route`]) rejects
/// unsurveyed destinations, so reaching an unsurveyed star always ends in
/// a sublight leg from the nearest surveyed waypoint. That sublight gap
/// dominates the surveyor's travel time in the common case, so we rank
/// targets by:
///   1. `gap` — distance from the target to the nearest surveyed system
///      (smaller = closer to the frontier of known space).
///   2. `home_dist` — distance from the target to the empire's reference
///      position (ruler's home, or fallback). Tie-breaks same-gap targets
///      by "prefer systems closer to our base."
///
/// When the empire has no surveyed systems at all (fresh start, pre-capital),
/// gap collapses to raw distance from the reference position — good enough
/// for the first dispatch.
///
/// Returns entity ids in rank order (best first).
pub fn rank_survey_targets(
    candidates: &[(Entity, [f64; 3])],
    surveyed_positions: &[[f64; 3]],
    reference_pos: [f64; 3],
) -> Vec<Entity> {
    let mut scored: Vec<(Entity, f64, f64)> = candidates
        .iter()
        .map(|(e, pos)| {
            let gap = if surveyed_positions.is_empty() {
                crate::physics::distance_ly_arr(*pos, reference_pos)
            } else {
                surveyed_positions
                    .iter()
                    .map(|sp| crate::physics::distance_ly_arr(*pos, *sp))
                    .fold(f64::INFINITY, f64::min)
            };
            let home_dist = crate::physics::distance_ly_arr(*pos, reference_pos);
            (*e, gap, home_dist)
        })
        .collect();
    scored.sort_by(|a, b| {
        a.1.partial_cmp(&b.1)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then(a.2.partial_cmp(&b.2).unwrap_or(std::cmp::Ordering::Equal))
    });
    scored.into_iter().map(|(e, _, _)| e).collect()
}

pub fn npc_decision_tick(
    clock: Res<GameClock>,
    mut last_tick: ResMut<LastAiDecisionTick>,
    mut bus: ResMut<AiBusResource>,
    npcs: Query<
        (
            Entity,
            &Faction,
            &KnowledgeStore,
            Option<&crate::knowledge::SystemVisibilityMap>,
        ),
        With<AiControlled>,
    >,
    // The Mid layer needs to know which systems exist at all — the
    // KnowledgeStore only carries entries the empire has already
    // surveyed / been told about (one entry per owned capital at spawn),
    // so `unsurveyed_systems` derived from it was always empty for
    // fresh empires, freezing Explorers in dock.
    //
    // Position is pulled alongside so we can rank survey targets by
    // "accessibility" — distance alone is a poor proxy for travel time
    // in a universe with FTL, obscured regions, and a surveyed-frontier
    // requirement.
    star_positions: Query<(Entity, &crate::components::Position), With<crate::galaxy::StarSystem>>,
    all_ships: Query<(
        Entity,
        &crate::ship::Ship,
        &crate::ship::ShipState,
        &crate::ship::CommandQueue,
    )>,
    research_queues: Query<&ResearchQueue, With<Empire>>,
    design_registry: Option<Res<crate::ship_design::ShipDesignRegistry>>,
    empire_rulers: Query<&EmpireRuler, With<Empire>>,
    ruler_q: Query<(&StationedAt, Option<&AboardShip>), With<Ruler>>,
    // Round 9 PR #2 Step 4: dedup against in-flight survey dispatches so
    // we don't double-assign two surveyors to the same target across
    // overlapping decision ticks. Marker is per-faction.
    pending_assignments: Query<(Entity, &PendingAssignment)>,
    // Round 11 Bug A: also dedup against commands sitting in the
    // light-speed outbox — between `bus.emit_command` (here) and
    // handler insert (`drain_ai_commands` → `PendingAssignment`),
    // a `survey_system` / `colonize_system` may live in
    // `AiCommandOutbox.entries` for hundreds of hexadies. Without
    // this, `mid_cadence=2` re-fires every other tick onto the same
    // target. See `tests/ai_npc_outbox_dedup.rs` for the regression.
    outbox: Res<AiCommandOutbox>,
    // #299 / #446 short-term loop fix: only systems hosting one of the
    // empire's own Cores are colonizable. Without this filter the AI
    // re-emits `colonize_system` every tick for systems where the
    // settling handler will reject the order on Core sovereignty grounds.
    // Long-term plan (#446 / #447): give the AI explicit `deploy_core`
    // commands and let the Short layer decompose colonize → deploy + colonize.
    core_ships: Query<
        (&crate::galaxy::AtSystem, &crate::faction::FactionOwner),
        With<crate::ship::CoreShip>,
    >,
    // #449 PR2b: per-MidAgent decision loop. Each `MidAgent` is
    // attached to a `Region`; the agent reasons over the systems in
    // its region only. With one region per empire today the loop
    // observes the same emit pattern as the legacy per-empire loop
    // (1 region = 1 mid agent = full empire scope), so existing NPC
    // integration tests stay green. Multi-region splits (PR2c+)
    // automatically activate cross-region isolation.
    mid_agents: Query<(Entity, &super::mid_agent::MidAgent)>,
    regions: Query<&crate::region::Region>,
    #[cfg(feature = "ai-log")] mut log: Option<ResMut<super::debug_log::AiLogConfig>>,
) {
    use crate::knowledge::SystemVisibilityTier;

    let now = clock.elapsed;
    if now <= last_tick.0 {
        return;
    }
    last_tick.0 = now;

    // #299 / #446: precompute per-empire "systems with our own Core"
    // before the empire loop. Used to filter `colonizable_systems` —
    // without this gate, NPCs re-emit `colonize_system` every tick for
    // targets that the settling handler will reject on Core sovereignty
    // grounds (Bug 4 in handoff doc).
    let mut core_systems_per_empire: std::collections::HashMap<
        Entity,
        std::collections::HashSet<Entity>,
    > = std::collections::HashMap::new();
    for (at, owner) in &core_ships {
        core_systems_per_empire
            .entry(owner.0)
            .or_default()
            .insert(at.0);
    }

    // Round 11 Bug A: precompute per-empire "in-flight survey /
    // colonize targets" from the light-speed outbox. The handler
    // insert path (`PendingAssignment` marker) only covers commands
    // that have already arrived; a command emitted on tick N with a
    // 30-hex light delay sits in `outbox.entries` until tick N+30,
    // and without this scan a `mid_cadence=2` `npc_decision_tick`
    // would happily re-fire onto the same target every other tick
    // for the entire delay window. The scan is a single pass over
    // outbox entries (typically small — only commands currently in
    // flight), so the cost is bounded and amortised across all
    // empires in the loop below.
    let survey_kind = cmd_ids::survey_system();
    let colonize_kind = cmd_ids::colonize_system();
    let mut outbox_survey_per_empire: std::collections::HashMap<
        Entity,
        std::collections::HashSet<Entity>,
    > = std::collections::HashMap::new();
    let mut outbox_colonize_per_empire: std::collections::HashMap<
        Entity,
        std::collections::HashSet<Entity>,
    > = std::collections::HashMap::new();
    // Both maps are mutated only inside the next block (single pass over
    // outbox.entries); the empire loop below reads via shared `&` only.
    if !outbox.entries.is_empty() {
        // Build issuer FactionId → empire Entity once (faction_id
        // encodes only `Entity::index()`, see `to_ai_faction`); then
        // each entry is an O(1) hashmap lookup instead of an O(empires)
        // scan.
        let mut faction_to_empire: std::collections::HashMap<macrocosmo_ai::FactionId, Entity> =
            std::collections::HashMap::new();
        for (entity, _, _, _) in &npcs {
            faction_to_empire.insert(to_ai_faction(entity), entity);
        }
        for entry in &outbox.entries {
            let cmd = &entry.command;
            let Some(&empire_entity) = faction_to_empire.get(&cmd.issuer) else {
                continue;
            };
            let target_set = if cmd.kind.as_str() == survey_kind.as_str() {
                Some(&mut outbox_survey_per_empire)
            } else if cmd.kind.as_str() == colonize_kind.as_str() {
                Some(&mut outbox_colonize_per_empire)
            } else {
                None
            };
            let Some(target_set) = target_set else {
                continue;
            };
            if let Some(macrocosmo_ai::CommandValue::System(s)) = cmd.params.get("target_system") {
                target_set
                    .entry(empire_entity)
                    .or_default()
                    .insert(from_ai_system(*s));
            }
        }
    }

    // #449 PR2b: per-MidAgent loop. We resolve each MidAgent →
    // Region → empire entity, then run the rule pipeline scoped to
    // the region's `member_systems`. With one region per empire (the
    // initial PR2a/PR2b state), `member_systems` is the empire's full
    // owned-system set so existing per-empire NPC behavior is
    // preserved bit-for-bit. PR2c+ (region split) automatically
    // activates cross-region isolation for free.
    //
    // Empire-level dedup precomputes (`outbox_*_per_empire`,
    // `core_systems_per_empire`) stay empire-keyed — they are
    // semantically per-faction and the simplest correct shape today.
    // Per-region narrowing is a future optimization once multi-region
    // splits land.
    for (mid_agent_entity, mid_agent) in &mid_agents {
        // Resolve region → empire. Skip silently if the back-references
        // are inconsistent (defensive: spawn pipeline guarantees a
        // valid Region; only a partial despawn or load could invalidate
        // it).
        let Ok(region) = regions.get(mid_agent.region) else {
            continue;
        };
        let empire = region.empire;
        let member_systems_slice = region.member_systems.as_slice();
        let member_systems_set: std::collections::HashSet<Entity> =
            member_systems_slice.iter().copied().collect();

        // Resolve empire-level data. Skip if the empire is not
        // `AiControlled` (e.g. a player empire whose MidAgent has
        // `auto_managed = false` — we still spawn the agent so PR3 UI
        // can reason about it, but the decision system stays silent).
        let Ok((entity, faction, knowledge, vis_map_opt)) = npcs.get(empire) else {
            continue;
        };
        // Player-empire / manual mode gate: MidAgent.auto_managed
        // controls whether NPC reasoning may emit commands for this
        // region. Today only the PlayerEmpire spawn path sets this to
        // false; #452 adds the per-region UI toggle.
        if !mid_agent.auto_managed {
            continue;
        }
        // `mid_agent_entity` is currently used only for logging /
        // future per-MidAgent state mutations; stance modulation is a
        // noop today (Rule pipeline ignores `Stance::default()`), so
        // we hold the agent by `&MidAgent` rather than `&mut`.
        let _ = mid_agent_entity;

        // Round 9 PR #2 Step 4: pre-collect this faction's in-flight
        // assignments so we can filter both ship and target candidates.
        // `pending_survey_targets` excludes systems already being
        // surveyed by one of our ships; `pending_assigned_ships`
        // excludes ships already carrying a marker (defense in depth —
        // by the time the handler runs, queue.is_empty() is also false,
        // but the marker covers the same-tick race).
        let mut pending_survey_targets: std::collections::HashSet<Entity> =
            std::collections::HashSet::new();
        let mut pending_assigned_ships: std::collections::HashSet<Entity> =
            std::collections::HashSet::new();
        for (ship_entity, pa) in &pending_assignments {
            if pa.faction != entity {
                continue;
            }
            pending_assigned_ships.insert(ship_entity);
            if pa.kind == AssignmentKind::Survey {
                if let AssignmentTarget::System(sys) = pa.target {
                    pending_survey_targets.insert(sys);
                }
            }
        }
        // Round 11 Bug A: union in outbox-resident in-flight commands
        // (handler hasn't inserted markers yet because the light-speed
        // window hasn't elapsed). Covers both decision-tick paths the
        // handler-side dedup misses.
        if let Some(set) = outbox_survey_per_empire.get(&entity) {
            pending_survey_targets.extend(set.iter().copied());
        }
        let pending_colonize_targets: std::collections::HashSet<Entity> =
            outbox_colonize_per_empire
                .get(&entity)
                .cloned()
                .unwrap_or_default();

        // Extract system intel. Hostile / colonizable signals still come
        // from the KnowledgeStore (those require detailed snapshots),
        // but `unsurveyed_systems` is derived from the galaxy-wide star
        // list minus whatever the empire has already surveyed —
        // otherwise freshly-spawned empires never find survey targets
        // because their KnowledgeStore is empty aside from the capital.
        //
        // PR2b: each list is intersected with `member_systems_set`
        // before reaching the adapter, so the Mid sees only systems
        // belonging to its region.
        let mut hostile_systems = Vec::new();
        let mut hostile_systems_set: std::collections::HashSet<Entity> =
            std::collections::HashSet::new();
        let mut colonizable_systems = Vec::new();
        let mut surveyed_ids: std::collections::HashSet<Entity> = std::collections::HashSet::new();
        // #299 / #446 short-term: limit colonization candidates to
        // systems where this empire already has a Core deployed. Without
        // an empty set this collapses to "no colonization possible" —
        // matching the settling handler's reject behavior, so the AI
        // stops looping on impossible orders. Once #446 lands and the AI
        // can issue `deploy_core`, this gate falls away naturally.
        let empty_core_set: std::collections::HashSet<Entity> = std::collections::HashSet::new();
        let owned_core_systems = core_systems_per_empire
            .get(&entity)
            .unwrap_or(&empty_core_set);
        for (_, k) in knowledge.iter() {
            if k.data.has_hostile && member_systems_set.contains(&k.system) {
                hostile_systems.push(k.system);
                hostile_systems_set.insert(k.system);
            }
            if k.data.surveyed {
                surveyed_ids.insert(k.system);
                // Bug B fix: skip systems known-hostile so the AI doesn't
                // ferry colonists into a meat grinder. Rule 1 (attack)
                // still consumes `hostile_systems` separately.
                if !k.data.colonized
                    && !k.data.has_hostile
                    && owned_core_systems.contains(&k.system)
                    // Round 11 Bug A: skip targets with a `colonize_system`
                    // already in the light-speed outbox — without this,
                    // the policy re-emits onto the same target every
                    // mid_cadence tick until the command lands.
                    && !pending_colonize_targets.contains(&k.system)
                    // PR2b: only colonize systems inside this Mid's region.
                    && member_systems_set.contains(&k.system)
                {
                    colonizable_systems.push(k.system);
                }
            }
        }
        // Every catalogued system (which, right now, means every system
        // in the galaxy thanks to `initialize_visibility_tiers`) is a
        // valid survey target if we haven't surveyed it yet. Fall back
        // to all stars when the empire has no visibility map — defensive
        // for test setups.
        let surveyed_positions: Vec<[f64; 3]> = star_positions
            .iter()
            .filter(|(e, _)| surveyed_ids.contains(e))
            .map(|(_, p)| p.as_array())
            .collect();

        // Resolve the empire's reference position for tiebreaks.
        let ruler_stationed_system: Option<Entity> = empire_rulers
            .get(entity)
            .ok()
            .and_then(|er| ruler_q.get(er.0).ok())
            .map(|(s, _)| s.system);
        let reference_pos: [f64; 3] = ruler_stationed_system
            .and_then(|sys| {
                star_positions
                    .iter()
                    .find(|(e, _)| *e == sys)
                    .map(|(_, p)| p.as_array())
            })
            .or_else(|| surveyed_positions.first().copied())
            .unwrap_or([0.0, 0.0, 0.0]);

        let candidates: Vec<(Entity, [f64; 3])> = star_positions
            .iter()
            .filter(|(e, _)| !surveyed_ids.contains(e))
            // Round 9 PR #2 Step 4: skip targets that already have a
            // surveyor in flight — prevents the "Vesk Scout-2 chases Vesk
            // Scout-1" double dispatch the handler-side dedup couldn't
            // catch from prior commits.
            .filter(|(e, _)| !pending_survey_targets.contains(e))
            // Bug B fix: skip systems we already know are hostile;
            // re-surveying a confirmed-hostile system loses scouts in a
            // tight loop. ROE-based engagement still flows through Rule 1.
            .filter(|(e, _)| !hostile_systems_set.contains(e))
            .filter(|(e, _)| {
                vis_map_opt
                    .map(|vm| vm.get(*e) >= SystemVisibilityTier::Catalogued)
                    .unwrap_or(true)
            })
            // PR2b: only survey systems inside this Mid's region.
            // Note: Rule 2's classical use case is "explore the
            // frontier of known space" — restricting to the region's
            // own member systems means survey targets must already
            // belong to this region. PR2c will introduce a separate
            // "expansion frontier" rule that proposes adding a system
            // to the region.
            .filter(|(e, _)| member_systems_set.contains(e))
            .map(|(e, p)| (e, p.as_array()))
            .collect();
        let unsurveyed_systems =
            rank_survey_targets(&candidates, &surveyed_positions, reference_pos);

        // Build ship inventory for this empire.
        // Round 9 PR #2 Step 4: ships carrying a `PendingAssignment` are
        // treated as non-idle even if their command queue hasn't yet been
        // populated by the handler (the marker is the AI-side "intent
        // already issued" signal). This closes the same-tick race where
        // `drain_ai_commands` writes a `SurveyRequested` event but
        // `handle_survey_requested` hasn't yet pushed the `MoveTo` /
        // `Survey` pair into the queue.
        //
        // PR2b: filter ships to those currently in this Mid's region
        // (`InSystem { system }` ∈ `member_systems`). Ships in transit
        // (`system: None`) are deliberately excluded — they belong to
        // whichever region they're heading toward (handled when they
        // arrive). Cross-region transfers will be modelled explicitly
        // by PR2c+.
        let ships: Vec<ShipInfo> = all_ships
            .iter()
            .filter(|(_, ship, _, _)| ship.owner == crate::ship::Owner::Empire(entity))
            .map(|(ship_entity, ship, state, queue)| {
                let system = match state {
                    crate::ship::ShipState::InSystem { system } => Some(*system),
                    _ => None,
                };
                let has_pending = pending_assigned_ships.contains(&ship_entity);
                let is_idle = system.is_some() && queue.commands.is_empty() && !has_pending;
                let can_survey = design_registry
                    .as_ref()
                    .is_some_and(|r| r.can_survey(&ship.design_id));
                let can_colonize = design_registry
                    .as_ref()
                    .is_some_and(|r| r.can_colonize(&ship.design_id));
                let is_combat = !can_survey && !can_colonize && !ship.is_immobile();
                ShipInfo {
                    entity: ship_entity,
                    design_id: ship.design_id.clone(),
                    system,
                    is_idle,
                    can_survey,
                    can_colonize,
                    is_combat,
                    ftl_range: ship.ftl_range,
                }
            })
            // PR2b: drop ships whose docked system is not in this
            // region's `member_systems`. In-transit ships
            // (`system: None`) never make it into the idle / combat /
            // colonizer / surveyor sets below regardless, so we filter
            // them out here too for clarity.
            .filter(|info| {
                info.system
                    .map(|s| member_systems_set.contains(&s))
                    .unwrap_or(false)
            })
            .collect();

        let is_researching = research_queues
            .get(entity)
            .is_ok_and(|rq| rq.current.is_some());

        // Extract Ruler info for this empire.
        let (ruler_entity, ruler_system, ruler_aboard) =
            if let Ok(empire_ruler) = empire_rulers.get(entity) {
                let ruler_e = empire_ruler.0;
                if let Ok((stationed, aboard)) = ruler_q.get(ruler_e) {
                    (Some(ruler_e), Some(stationed.system), aboard.is_some())
                } else {
                    (Some(ruler_e), None, false)
                }
            } else {
                (None, None, false)
            };

        let context = NpcContext {
            hostile_systems,
            unsurveyed_systems,
            colonizable_systems,
            ships,
            is_researching,
            ruler_entity,
            ruler_system,
            ruler_aboard,
        };

        // Route the per-MidAgent decision through the layered
        // MidStanceAgent (#448). Pre-compute idle_combat /
        // idle_colonizers / idle_surveyors with the same expressions
        // the agent's rules use (Rules 1 / 3 / 2) so the adapter can
        // hand them straight in without re-scanning the ship list.
        let idle_combat: Vec<Entity> = context
            .ships
            .iter()
            .filter(|s| s.is_idle && s.is_combat)
            .map(|s| s.entity)
            .collect();
        let idle_colonizers: Vec<Entity> = context
            .ships
            .iter()
            .filter(|s| s.is_idle && s.can_colonize)
            .map(|s| s.entity)
            .collect();
        let idle_surveyors: Vec<Entity> = context
            .ships
            .iter()
            .filter(|s| s.is_idle && s.can_survey)
            .map(|s| s.entity)
            .collect();
        let adapter = crate::ai::mid_adapter::BevyMidGameAdapter {
            faction: entity,
            context: &context,
            bus: &bus.0,
            idle_combat: &idle_combat,
            idle_colonizers: &idle_colonizers,
            idle_surveyors: &idle_surveyors,
            member_systems: member_systems_slice,
        };
        // Stance is read from the per-MidAgent state. Today every
        // rule ignores stance (`MidStanceAgent::decide` accepts but
        // does not consult it — see its module doc); a future PR
        // wires stance modulation into Rule priority weighting.
        // Keeping the read here means the spec is already in place.
        let proposals = super::mid_stance::MidStanceAgent::decide(
            &adapter,
            &mid_agent.state.stance,
            &faction.id,
            now,
        );
        let commands = crate::ai::mid_adapter::arbitrate(proposals);
        for cmd in commands {
            bus.0.emit_command(cmd);
        }

        #[cfg(feature = "ai-log")]
        if let Some(ref mut log) = log {
            super::debug_log::write_decision_log(log, now, &faction.id, &bus);
        }
    }
}
