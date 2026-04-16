//! Ship Operations tab (#346 / ESC-3 Commit 2).
//!
//! Group every live `Ship` by activity category (Travel / Survey /
//! Combat / Other). Category groups become root events; individual
//! ships hang as children with `EventSource::Ship(entity)`.
//!
//! Category mapping:
//! * **Travel** — `ShipState::SubLight` or `ShipState::InFTL`.
//! * **Survey** — `ShipState::Surveying` or `ShipState::Scouting`.
//! * **Combat** — any ship whose current system hosts a `Hostile`
//!   entity. `resolve_combat` is tick-based (no persistent "in-combat"
//!   flag), so co-location is the cleanest pure-read signal.
//! * **Other** — `Docked`, `Loitering`, `Settling`, `Refitting`.
//!
//! Badge surface: Combat count → `Severity::Warn` (ships engaged).

use std::collections::{HashMap, HashSet};

use bevy::prelude::*;

use crate::galaxy::{AtSystem, Hostile, StarSystem};
use crate::ship::{Ship, ShipState};
use crate::time_system::GameClock;

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

fn collect_ship_events(world: &World) -> Vec<Event> {
    let clock = world.resource::<GameClock>();
    let now = clock.elapsed;
    let hostiles = hostile_systems(world);

    let mut buckets: HashMap<Category, Vec<Event>> = HashMap::new();

    if let Some(mut q) = world.try_query::<(Entity, &Ship, &ShipState)>() {
        for (ship_entity, ship, state) in q.iter(world) {
            let current_system = ship_current_system(state);
            let (category, state_label, eta, started_at) =
                classify_ship(state, current_system, &hostiles, now);
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
    if let Some(mut q) = world.try_query::<(&Ship, &ShipState)>() {
        for (_ship, state) in q.iter(world) {
            let current_system = ship_current_system(state);
            let (category, _, _, _) = classify_ship(state, current_system, &hostiles, now);
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

fn ship_current_system(state: &ShipState) -> Option<Entity> {
    match state {
        ShipState::Docked { system } => Some(*system),
        ShipState::Settling { system, .. } => Some(*system),
        ShipState::Refitting { system, .. } => Some(*system),
        ShipState::Surveying { target_system, .. } => Some(*target_system),
        ShipState::Scouting { target_system, .. } => Some(*target_system),
        ShipState::SubLight { target_system, .. } => *target_system,
        ShipState::InFTL {
            destination_system, ..
        } => Some(*destination_system),
        ShipState::Loitering { .. } => None,
    }
}

fn classify_ship(
    state: &ShipState,
    current_system: Option<Entity>,
    hostiles: &HashSet<Entity>,
    now: i64,
) -> (Category, String, Option<i64>, i64) {
    // Combat override: if the ship is currently resident in a system
    // containing a hostile entity AND it's not in transit, classify as
    // Combat regardless of the other state bits. A ship in InFTL to a
    // hostile-occupied system is still "travelling" — it's not yet
    // engaged.
    let in_transit = matches!(state, ShipState::InFTL { .. } | ShipState::SubLight { .. });
    if !in_transit
        && let Some(sys) = current_system
        && hostiles.contains(&sys)
    {
        return (Category::Combat, "engaging hostiles".into(), None, now);
    }

    match state {
        ShipState::SubLight {
            departed_at,
            arrival_at,
            ..
        } => (
            Category::Travel,
            "sublight transit".into(),
            Some(*arrival_at),
            *departed_at,
        ),
        ShipState::InFTL {
            departed_at,
            arrival_at,
            ..
        } => (
            Category::Travel,
            "in FTL".into(),
            Some(*arrival_at),
            *departed_at,
        ),
        ShipState::Surveying {
            started_at,
            completes_at,
            ..
        } => (
            Category::Survey,
            "surveying".into(),
            Some(*completes_at),
            *started_at,
        ),
        ShipState::Scouting {
            started_at,
            completes_at,
            ..
        } => (
            Category::Survey,
            "scouting".into(),
            Some(*completes_at),
            *started_at,
        ),
        ShipState::Settling {
            started_at,
            completes_at,
            ..
        } => (
            Category::Other,
            "settling colony".into(),
            Some(*completes_at),
            *started_at,
        ),
        ShipState::Refitting {
            started_at,
            completes_at,
            ..
        } => (
            Category::Other,
            "refitting".into(),
            Some(*completes_at),
            *started_at,
        ),
        ShipState::Docked { .. } => (Category::Other, "docked".into(), None, now),
        ShipState::Loitering { .. } => (Category::Other, "loitering".into(), None, now),
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
                    player_aboard: false,
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
        spawn_ship(&mut world, "Alpha", ShipState::Docked { system });

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
        spawn_ship(&mut world, "Guardian", ShipState::Docked { system });

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
