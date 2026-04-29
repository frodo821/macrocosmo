//! Ship Operations tab (#346 / ESC-3 Commit 2).
//!
//! Group every live `Ship` by activity category (Travel / Survey /
//! Combat / Other). Category groups become root events; individual
//! ships hang as children with `EventSource::Ship(entity)`.
//!
//! Category mapping is light-coherent (#491 PR-4): the classifier reads
//! the viewing empire's `KnowledgeStore` projection (own ships) or
//! snapshot (foreign ships), never the realtime ECS [`ShipState`]
//! directly. The `ship_view` helper performs the projection /
//! snapshot collapse and returns a [`ShipView`] carrying a
//! [`ShipSnapshotState`]; this tab classifies on that.
//!
//! Category mapping (over [`ShipSnapshotState`]):
//! * **Travel** — `InTransitSubLight` or `InTransitFTL`. The two
//!   variants render distinct labels (`"sublight transit"` vs
//!   `"in FTL"`) so the player can see whether the ship is
//!   interceptable; FTL ships are not.
//! * **Survey** — `Surveying`. (Realtime `Scouting` collapses to
//!   `Surveying` at snapshot granularity per #217 — the player UI
//!   does not distinguish them at this layer.)
//! * **Combat** — any ship whose current system hosts a `Hostile`
//!   entity AND that is not currently in transit. `resolve_combat` is
//!   tick-based (no persistent "in-combat" flag), so co-location is
//!   the cleanest pure-read signal.
//! * **Other** — `InSystem` (docked), `Loitering`, `Settling`,
//!   `Refitting`. `Destroyed` and `Missing` are filtered out so the
//!   tab does not surface ships the player can no longer command.
//!
//! Badge surface: Combat count → `Severity::Warn` (ships engaged).

use std::collections::{HashMap, HashSet};

use bevy::prelude::*;

use crate::galaxy::{AtSystem, Hostile, StarSystem};
use crate::knowledge::{KnowledgeStore, ShipSnapshotState};
use crate::observer::ObserverMode;
use crate::player::PlayerEmpire;
use crate::ship::{Ship, ShipState};
use crate::time_system::GameClock;
use crate::ui::ship_view::{ShipView, ShipViewTiming, ship_view_with_timing};

use super::tab::{OngoingTab, TabBadge, TabMeta};
use super::types::{Event, EventKind, EventSource, Severity};

/// ESC-3 Commit 2: Ship Operations.
pub struct ShipOperationsTab;

impl ShipOperationsTab {
    pub const ID: &'static str = "ship_operations";
    pub const ORDER: i32 = 200;
}

impl OngoingTab for ShipOperationsTab {
    fn meta(&self) -> TabMeta {
        TabMeta {
            id: Self::ID,
            display_name: "Ship Operations",
            order: Self::ORDER,
        }
    }

    fn collect(&self, world: &World) -> Vec<Event> {
        collect_ship_events(world)
    }

    fn badge(&self, world: &World) -> Option<TabBadge> {
        let summary = summarise_ships(world);
        let total = summary.travel + summary.survey + summary.combat + summary.other;
        if total == 0 {
            return None;
        }
        let severity = if summary.combat > 0 {
            Severity::Warn
        } else {
            Severity::Info
        };
        Some(TabBadge::new(total as u32, severity))
    }
}

#[derive(Default, Debug, PartialEq, Eq)]
struct ShipSummary {
    travel: usize,
    survey: usize,
    combat: usize,
    other: usize,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
enum Category {
    Travel,
    Survey,
    Combat,
    Other,
}

impl Category {
    fn label(self) -> &'static str {
        match self {
            Category::Travel => "Traveling",
            Category::Survey => "Surveying / Scouting",
            Category::Combat => "In Combat",
            Category::Other => "Docked / Idle",
        }
    }

    /// Sort key: groups appear left-to-right in this order.
    fn order(self) -> u8 {
        match self {
            Category::Combat => 0,
            Category::Travel => 1,
            Category::Survey => 2,
            Category::Other => 3,
        }
    }

    /// Event kind surfaced on each leaf.
    fn kind(self) -> EventKind {
        match self {
            Category::Travel => EventKind::Travel,
            Category::Survey => EventKind::Survey,
            Category::Combat => EventKind::Combat,
            Category::Other => EventKind::Other,
        }
    }
}

/// Set of star-system entities currently hosting a `Hostile` entity.
/// Combat detection reduces to "does the ship's current system live in
/// this set".
fn hostile_systems(world: &World) -> HashSet<Entity> {
    let mut out = HashSet::new();
    if let Some(mut q) = world.try_query::<(&AtSystem, &Hostile)>() {
        for (at, _hostile) in q.iter(world) {
            out.insert(at.0);
        }
    }
    out
}

/// Resolve the player empire entity (= viewing empire). Returns `None`
/// if no `PlayerEmpire` exists yet (early Startup before the spawn
/// system runs); callers fall back to the realtime ECS path.
fn resolve_player_empire(world: &World) -> Option<Entity> {
    let mut q = world.try_query::<(Entity, &PlayerEmpire)>()?;
    q.iter(world).next().map(|(e, _)| e)
}

/// #491 PR-4 follow-up: source-resolved viewing context. The
/// situation_center collects events from the player empire's
/// perspective by default; in observer mode we want ground truth
/// (= realtime ECS), so we pass `None` for both the `KnowledgeStore`
/// and the viewing empire — `ship_view_with_timing` then falls
/// through to `realtime_state_to_snapshot`.
struct ViewingContext<'a> {
    knowledge: Option<&'a KnowledgeStore>,
    empire: Option<Entity>,
}

fn resolve_viewing_context(world: &World) -> ViewingContext<'_> {
    let observer_mode_enabled = world
        .get_resource::<ObserverMode>()
        .map(|o| o.enabled)
        .unwrap_or(false);
    if observer_mode_enabled {
        // Observer mode: omniscient view = realtime ECS = ground truth.
        // Mirrors the gate pattern in outline-tree (#487), ship-panel
        // (#491 PR-2), and map tooltip (#491 PR-6) observer paths.
        return ViewingContext {
            knowledge: None,
            empire: None,
        };
    }
    let empire = resolve_player_empire(world);
    let knowledge = empire.and_then(|e| world.entity(e).get::<KnowledgeStore>());
    ViewingContext { knowledge, empire }
}

fn collect_ship_events(world: &World) -> Vec<Event> {
    let clock = world.resource::<GameClock>();
    let now = clock.elapsed;
    let hostiles = hostile_systems(world);

    let ctx = resolve_viewing_context(world);

    let mut buckets: HashMap<Category, Vec<Event>> = HashMap::new();

    if let Some(mut q) = world.try_query::<(Entity, &Ship, &ShipState)>() {
        for (ship_entity, ship, state) in q.iter(world) {
            // #389: Exclude immobile ships (stations) from ship operations
            if ship.is_immobile() {
                continue;
            }

            // #491 PR-4: route the ship through the viewing empire's
            // KnowledgeStore. Own = projection, foreign = snapshot,
            // no-store = realtime fallback.
            //
            // #491 PR-4 follow-up: replaced the local
            // `(ship_view + ship_view_timing)` two-step with the
            // hoisted `ship_view_with_timing` helper so the ladder
            // cannot drift from the canonical implementation in
            // `crate::knowledge::ship_view`.
            let Some((view, timing)) =
                ship_view_with_timing(ship_entity, ship, state, ctx.knowledge, ctx.empire)
            else {
                // No projection / snapshot for this ship — skip rather
                // than render stale realtime ECS state. Mirrors the
                // outline-tree contract (#487).
                continue;
            };

            let Some((category, state_label, eta, started_at)) =
                classify_view(&view, &timing, &hostiles, now)
            else {
                // Destroyed / Missing — filter out of the tab.
                continue;
            };

            let label = format!("{} — {}", ship.name, state_label);
            buckets.entry(category).or_default().push(Event {
                id: ship_entity.to_bits(),
                source: EventSource::Ship(ship_entity),
                started_at,
                kind: category.kind(),
                label,
                progress: eta.and_then(|arr| travel_progress(started_at, arr, now)),
                eta,
                children: Vec::new(),
            });
        }
    }

    // Empty world ⇒ empty tree. Callers expect `None` badge in that
    // case; the default `OngoingTab::badge` roll-up would emit one
    // Info-severity entry otherwise.
    let mut events: Vec<Event> = buckets
        .into_iter()
        .filter(|(_, v)| !v.is_empty())
        .map(|(category, mut children)| {
            // Sort children by label for deterministic ordering.
            children.sort_by(|a, b| a.label.cmp(&b.label));
            let child_count = children.len();
            Event {
                id: hash_category(category),
                source: EventSource::None,
                started_at: now,
                kind: category.kind(),
                label: format!("{} ({})", category.label(), child_count),
                progress: None,
                eta: None,
                children,
            }
        })
        .collect();
    events.sort_by_key(|e| {
        // Round-trip the category order from the first child's kind —
        // but we've embedded it implicitly via the header label. Use
        // the category order from the first matching variant instead.
        guess_category_from_kind(e.kind).order()
    });
    events
}

fn summarise_ships(world: &World) -> ShipSummary {
    let hostiles = hostile_systems(world);
    let mut summary = ShipSummary::default();
    let clock = world.resource::<GameClock>();
    let now = clock.elapsed;

    let ctx = resolve_viewing_context(world);

    if let Some(mut q) = world.try_query::<(Entity, &Ship, &ShipState)>() {
        for (ship_entity, ship, state) in q.iter(world) {
            // #389: Exclude immobile ships (stations) from summary
            if ship.is_immobile() {
                continue;
            }

            // #491 PR-4 follow-up: same hoisted-helper rewire as
            // `collect_ship_events` so the badge count and the tree
            // contents cannot drift.
            let Some((view, timing)) =
                ship_view_with_timing(ship_entity, ship, state, ctx.knowledge, ctx.empire)
            else {
                continue;
            };

            let Some((category, _, _, _)) = classify_view(&view, &timing, &hostiles, now) else {
                continue;
            };

            match category {
                Category::Travel => summary.travel += 1,
                Category::Survey => summary.survey += 1,
                Category::Combat => summary.combat += 1,
                Category::Other => summary.other += 1,
            }
        }
    }
    summary
}

/// #491 PR-4: Classify a [`ShipView`] (= projection / snapshot
/// derived) into a UI category, plus the labels and timing payload.
///
/// Returns `None` for `Destroyed` / `Missing` — the player has no
/// command surface for those ships, so they are filtered out of the
/// Ship Operations tab. Callers `continue` on `None`.
fn classify_view(
    view: &ShipView,
    timing: &ShipViewTiming,
    hostiles: &HashSet<Entity>,
    now: i64,
) -> Option<(Category, String, Option<i64>, i64)> {
    // Combat override: if the ship is currently resident in a system
    // containing a hostile entity AND it's not in transit, classify
    // as Combat regardless of the other state bits. A ship in
    // InTransitFTL / InTransitSubLight to a hostile-occupied system
    // is still "travelling" — it's not yet engaged.
    let in_transit = view.state.is_in_transit();
    if !in_transit
        && let Some(sys) = view.system
        && hostiles.contains(&sys)
    {
        return Some((Category::Combat, "engaging hostiles".into(), None, now));
    }

    match &view.state {
        ShipSnapshotState::InTransitSubLight => Some((
            Category::Travel,
            "sublight transit".into(),
            timing.expected_tick,
            timing.origin_tick,
        )),
        ShipSnapshotState::InTransitFTL => Some((
            Category::Travel,
            "in FTL".into(),
            timing.expected_tick,
            timing.origin_tick,
        )),
        ShipSnapshotState::Surveying => Some((
            Category::Survey,
            "surveying".into(),
            timing.expected_tick,
            timing.origin_tick,
        )),
        ShipSnapshotState::Settling => Some((
            Category::Other,
            "settling colony".into(),
            timing.expected_tick,
            timing.origin_tick,
        )),
        ShipSnapshotState::Refitting => Some((
            Category::Other,
            "refitting".into(),
            timing.expected_tick,
            timing.origin_tick,
        )),
        ShipSnapshotState::InSystem => Some((Category::Other, "docked".into(), None, now)),
        ShipSnapshotState::Loitering { .. } => {
            Some((Category::Other, "loitering".into(), None, now))
        }
        // Destroyed / Missing: filtered out of the tab. The player
        // cannot act on these ships — `Destroyed` is a terminal
        // state, `Missing` (#409) is "presumed lost". Letting them
        // through would inflate the badge count for non-actionable
        // entries.
        ShipSnapshotState::Destroyed | ShipSnapshotState::Missing => None,
    }
}

/// Stable-ish ids for category headers so scroll state survives frames.
fn hash_category(category: Category) -> u64 {
    // High-bit tag keeps category ids disjoint from ship-entity ids.
    0xC17E_0000_0000_0000 | (category as u64)
}

fn guess_category_from_kind(kind: EventKind) -> Category {
    match kind {
        EventKind::Travel => Category::Travel,
        EventKind::Survey => Category::Survey,
        EventKind::Combat => Category::Combat,
        _ => Category::Other,
    }
}

fn travel_progress(started_at: i64, arrival_at: i64, now: i64) -> Option<f32> {
    if arrival_at <= started_at {
        return None;
    }
    let total = (arrival_at - started_at) as f32;
    let done = ((now - started_at) as f32).clamp(0.0, total);
    Some((done / total).clamp(0.0, 1.0))
}

/// Attach system names to travel leaves once the buckets are built.
/// Optional helper — kept separate so `collect` remains straightforward.
#[allow(dead_code)]
fn system_name(world: &World, system: Entity) -> String {
    world
        .get::<StarSystem>(system)
        .map(|s| s.name.clone())
        .unwrap_or_else(|| format!("System {:?}", system))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::components::Position;
    use crate::galaxy::{Hostile, HostileHitpoints, HostileStats, StarSystem};
    use crate::ship::{EquippedModule, Owner, Ship, ShipState};

    fn spawn_system(world: &mut World, name: &str) -> Entity {
        world
            .spawn((
                StarSystem {
                    name: name.into(),
                    surveyed: true,
                    is_capital: false,
                    star_type: "yellow_dwarf".into(),
                },
                Position::from([0.0, 0.0, 0.0]),
            ))
            .id()
    }

    fn spawn_ship(world: &mut World, name: &str, state: ShipState) -> Entity {
        world
            .spawn((
                Ship {
                    name: name.into(),
                    design_id: "corvette".into(),
                    hull_id: "corvette".into(),
                    modules: Vec::<EquippedModule>::new(),
                    owner: Owner::Neutral,
                    sublight_speed: 1.0,
                    ftl_range: 0.0,
                    ruler_aboard: false,
                    home_port: Entity::PLACEHOLDER,
                    design_revision: 0,
                    fleet: None,
                },
                state,
            ))
            .id()
    }

    fn setup_clock(world: &mut World, now: i64) {
        world.insert_resource(GameClock::new(now));
    }

    #[test]
    fn empty_world_emits_no_events() {
        let mut world = World::new();
        setup_clock(&mut world, 0);
        let tab = ShipOperationsTab;
        let events = tab.collect(&world);
        assert!(events.is_empty());
        assert!(tab.badge(&world).is_none());
    }

    #[test]
    fn docked_ship_surfaces_under_other_category() {
        let mut world = World::new();
        setup_clock(&mut world, 0);
        let system = spawn_system(&mut world, "Sol");
        spawn_ship(&mut world, "Alpha", ShipState::InSystem { system });

        let tab = ShipOperationsTab;
        let events = tab.collect(&world);
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].kind, EventKind::Other);
        assert_eq!(events[0].children.len(), 1);
        assert!(events[0].children[0].label.contains("Alpha"));
        assert!(events[0].children[0].label.contains("docked"));
    }

    #[test]
    fn traveling_ship_goes_into_travel_group_with_eta_and_progress() {
        let mut world = World::new();
        setup_clock(&mut world, 50);
        let origin = spawn_system(&mut world, "Sol");
        let dest = spawn_system(&mut world, "Proxima");
        spawn_ship(
            &mut world,
            "Explorer",
            ShipState::InFTL {
                origin_system: origin,
                destination_system: dest,
                departed_at: 10,
                arrival_at: 110,
            },
        );

        let tab = ShipOperationsTab;
        let events = tab.collect(&world);
        let travel = events
            .iter()
            .find(|e| e.kind == EventKind::Travel)
            .expect("travel bucket exists");
        let leaf = &travel.children[0];
        assert_eq!(leaf.eta, Some(110));
        let p = leaf.progress.expect("progress computed");
        // 50 is 40% of the way from 10 to 110.
        assert!((p - 0.4).abs() < 0.001, "unexpected progress {}", p);
        // #491 PR-4: InTransitFTL must surface as "in FTL" — distinct
        // from sublight transit.
        assert!(leaf.label.contains("in FTL"));
    }

    #[test]
    fn surveying_ship_goes_into_survey_group() {
        let mut world = World::new();
        setup_clock(&mut world, 0);
        let target = spawn_system(&mut world, "Sirius");
        spawn_ship(
            &mut world,
            "Surveyor",
            ShipState::Surveying {
                target_system: target,
                started_at: 0,
                completes_at: 20,
            },
        );

        let tab = ShipOperationsTab;
        let events = tab.collect(&world);
        let survey = events
            .iter()
            .find(|e| e.kind == EventKind::Survey)
            .expect("survey bucket exists");
        assert_eq!(survey.children.len(), 1);
        assert_eq!(survey.children[0].eta, Some(20));
    }

    #[test]
    fn combat_group_triggers_on_hostile_colocation_and_bumps_badge() {
        let mut world = World::new();
        setup_clock(&mut world, 0);
        let system = spawn_system(&mut world, "Frontier");
        // Plant a hostile in that system.
        world.spawn((
            crate::galaxy::AtSystem(system),
            Hostile,
            HostileHitpoints {
                hp: 10.0,
                max_hp: 10.0,
            },
            HostileStats {
                strength: 1.0,
                evasion: 0.1,
            },
        ));
        spawn_ship(&mut world, "Guardian", ShipState::InSystem { system });

        let tab = ShipOperationsTab;
        let events = tab.collect(&world);
        let combat = events
            .iter()
            .find(|e| e.kind == EventKind::Combat)
            .expect("combat bucket exists");
        assert_eq!(combat.children.len(), 1);

        let badge = tab.badge(&world).expect("combat badge");
        assert_eq!(badge.severity, Severity::Warn);
        assert_eq!(badge.count, 1);
    }

    #[test]
    fn in_flight_ship_is_not_flagged_as_combat_even_if_destination_is_hostile() {
        let mut world = World::new();
        setup_clock(&mut world, 0);
        let origin = spawn_system(&mut world, "Sol");
        let destination = spawn_system(&mut world, "Frontier");
        world.spawn((
            crate::galaxy::AtSystem(destination),
            Hostile,
            HostileHitpoints {
                hp: 10.0,
                max_hp: 10.0,
            },
            HostileStats {
                strength: 1.0,
                evasion: 0.1,
            },
        ));
        spawn_ship(
            &mut world,
            "Transit",
            ShipState::InFTL {
                origin_system: origin,
                destination_system: destination,
                departed_at: 0,
                arrival_at: 100,
            },
        );

        let tab = ShipOperationsTab;
        let events = tab.collect(&world);
        assert!(
            events.iter().all(|e| e.kind != EventKind::Combat),
            "InFTL ships must not be flagged as Combat even when destination hosts a hostile"
        );
    }
}
