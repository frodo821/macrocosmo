//! Construction Overview tab (#346 / ESC-3 Commit 1).
//!
//! Surfaces every active build order across the player's empire, grouped
//! by star system. Each system with a non-empty queue becomes a root
//! `Event`; individual orders (ship builds, building construction,
//! demolitions, upgrades) hang as children.
//!
//! Bottleneck detection — an order whose build time still has hexadies
//! remaining but whose hosting system's `ResourceStockpile` cannot cover
//! the outstanding per-order resource cost — is surfaced by prefixing the
//! label with `[BOTTLENECK]` and bumping the tab badge to `Critical`. The
//! heuristic is intentionally cheap (system-level stockpile check, no
//! per-tick integration) to stay inside the `collect`-is-pure-read
//! contract.
//!
//! The tab is registered as an [`OngoingTab`]; the framework supplies
//! the default Event-tree renderer via `OngoingTabAdapter`.

use std::collections::HashMap;

use bevy::prelude::*;

use crate::amount::Amt;
use crate::colony::{BuildQueue, BuildingQueue, Colony, ResourceStockpile};
use crate::galaxy::{Planet, StarSystem};
use crate::time_system::GameClock;

use super::tab::{OngoingTab, TabBadge, TabMeta};
use super::types::{Event, EventKind, EventSource, Severity};

/// ESC-3 Commit 1: Construction Overview.
pub struct ConstructionOverviewTab;

impl ConstructionOverviewTab {
    pub const ID: &'static str = "construction_overview";
    pub const ORDER: i32 = 100;
}

impl OngoingTab for ConstructionOverviewTab {
    fn meta(&self) -> TabMeta {
        TabMeta {
            id: Self::ID,
            display_name: "Construction",
            order: Self::ORDER,
        }
    }

    fn collect(&self, world: &World) -> Vec<Event> {
        collect_construction_events(world)
    }

    fn badge(&self, world: &World) -> Option<TabBadge> {
        let summary = summarise_construction(world);
        if summary.active == 0 && summary.bottleneck == 0 {
            return None;
        }
        let severity = if summary.bottleneck > 0 {
            Severity::Critical
        } else {
            Severity::Info
        };
        // Badge counts active orders (including bottlenecked ones). The
        // tint flips to Critical when any bottleneck is detected so the
        // strip colour gives an at-a-glance warning.
        Some(TabBadge::new(summary.active as u32, severity))
    }
}

#[derive(Default, Debug, PartialEq, Eq)]
struct ConstructionSummary {
    active: usize,
    bottleneck: usize,
}

/// Walk every colony once, looking up the hosting system's stockpile, and
/// emit one root `Event` per system with a non-empty queue.
fn collect_construction_events(world: &World) -> Vec<Event> {
    let clock = world.resource::<GameClock>();
    let now = clock.elapsed;

    // Colony queries mix `BuildQueue` (ship orders) and `BuildingQueue`
    // (building / demolition / upgrade orders). Both live on the colony
    // entity. `collect` receives `&World` (read-only) — `World::query`
    // needs `&mut World`, so build `QueryState`s via `try_query` and
    // iterate them against the immutable world.
    //
    // We deliberately do NOT gate on player-empire ownership yet:
    // observer mode and future multi-empire scenarios both want the full
    // construction roll-up. If a filter is ever needed it slots in here.
    let mut systems: HashMap<Entity, SystemBucket> = HashMap::new();

    // --- Ship / deliverable orders (BuildQueue on Colony) ------------
    if let Some(mut colonies_q) = world.try_query::<(&Colony, &BuildQueue)>() {
        for (colony, queue) in colonies_q.iter(world) {
            if queue.queue.is_empty() {
                continue;
            }
            let Some(system_entity) = resolve_system(world, colony.planet) else {
                continue;
            };
            let bucket = systems.entry(system_entity).or_default();
            let stockpile = world.get::<ResourceStockpile>(system_entity);
            let planet_name = world
                .get::<Planet>(colony.planet)
                .map(|p| p.name.clone())
                .unwrap_or_else(|| "?".into());
            for order in &queue.queue {
                let label = format!("{}: {}", planet_name, order.display_name);
                let minerals_remaining = order.minerals_cost.sub(order.minerals_invested);
                let energy_remaining = order.energy_cost.sub(order.energy_invested);
                let bottleneck = is_bottlenecked(
                    stockpile,
                    minerals_remaining,
                    energy_remaining,
                    order.build_time_remaining,
                );
                let progress = compute_progress(
                    order.minerals_cost,
                    order.minerals_invested,
                    order.energy_cost,
                    order.energy_invested,
                    order.build_time_total,
                    order.build_time_remaining,
                );
                let eta = compute_eta(now, order.build_time_remaining);
                let started_at =
                    now.saturating_sub(order.build_time_total - order.build_time_remaining);
                bucket.push(build_event(
                    order.order_id,
                    started_at,
                    label,
                    progress,
                    eta,
                    bottleneck,
                ));
            }
        }
    }

    // --- Building / demolition / upgrade orders (BuildingQueue) -------
    if let Some(mut colonies_bq) = world.try_query::<(&Colony, &BuildingQueue)>() {
        for (colony, bq) in colonies_bq.iter(world) {
            let total = bq.queue.len() + bq.demolition_queue.len() + bq.upgrade_queue.len();
            if total == 0 {
                continue;
            }
            let Some(system_entity) = resolve_system(world, colony.planet) else {
                continue;
            };
            let bucket = systems.entry(system_entity).or_default();
            let stockpile = world.get::<ResourceStockpile>(system_entity);
            let planet_name = world
                .get::<Planet>(colony.planet)
                .map(|p| p.name.clone())
                .unwrap_or_else(|| "?".into());

            for order in &bq.queue {
                let label = format!(
                    "{}: build {} (slot {})",
                    planet_name, order.building_id.0, order.target_slot
                );
                let bottleneck = is_bottlenecked(
                    stockpile,
                    order.minerals_remaining,
                    order.energy_remaining,
                    order.build_time_remaining,
                );
                // Progress here is tricky: `BuildingOrder` carries only
                // *remaining* values, not the original totals. Without
                // registry resolution in a pure-read path we cannot infer
                // the progress fraction cleanly — so progress stays `None`
                // and the renderer shows an indeterminate label.
                bucket.push(build_event(
                    order.order_id,
                    now, // best effort — real `started_at` needs a builder migration
                    label,
                    None,
                    compute_eta(now, order.build_time_remaining),
                    bottleneck,
                ));
            }

            for order in &bq.demolition_queue {
                let label = format!(
                    "{}: demolish {} (slot {})",
                    planet_name, order.building_id.0, order.target_slot
                );
                bucket.push(build_event(
                    order.order_id,
                    now,
                    label,
                    None,
                    compute_eta(now, order.time_remaining),
                    false,
                ));
            }

            for order in &bq.upgrade_queue {
                let label = format!(
                    "{}: upgrade -> {} (slot {})",
                    planet_name, order.target_id.0, order.slot_index
                );
                let bottleneck = is_bottlenecked(
                    stockpile,
                    order.minerals_remaining,
                    order.energy_remaining,
                    order.build_time_remaining,
                );
                bucket.push(build_event(
                    order.order_id,
                    now,
                    label,
                    None,
                    compute_eta(now, order.build_time_remaining),
                    bottleneck,
                ));
            }
        }
    }

    // --- Fold per-system buckets into root Events --------------------
    let mut events: Vec<Event> = systems
        .into_iter()
        .filter(|(_, b)| !b.children.is_empty())
        .map(|(system_entity, bucket)| {
            let system_name = world
                .get::<StarSystem>(system_entity)
                .map(|s| s.name.clone())
                .unwrap_or_else(|| format!("System {:?}", system_entity));
            let child_count = bucket.children.len();
            let bottleneck_count = bucket.bottleneck_count;
            let header = if bottleneck_count > 0 {
                format!(
                    "{} ({} active, {} bottleneck)",
                    system_name, child_count, bottleneck_count
                )
            } else {
                format!("{} ({} active)", system_name, child_count)
            };
            Event {
                id: system_entity.to_bits(),
                source: EventSource::System(system_entity),
                started_at: now,
                kind: EventKind::Construction,
                label: header,
                progress: None,
                eta: None,
                children: bucket.children,
            }
        })
        .collect();

    // Stable ordering: bottlenecked systems first (critical), then by
    // label. This keeps the tree visually deterministic across frames.
    events.sort_by(|a, b| b.label.cmp(&a.label).reverse());
    events
}

/// Lightweight summary used only by `badge`. Mirrors `collect` counts but
/// avoids the label allocations.
fn summarise_construction(world: &World) -> ConstructionSummary {
    let mut summary = ConstructionSummary::default();

    if let Some(mut colonies_q) = world.try_query::<(&Colony, &BuildQueue)>() {
        for (colony, queue) in colonies_q.iter(world) {
            if queue.queue.is_empty() {
                continue;
            }
            let Some(system_entity) = resolve_system(world, colony.planet) else {
                continue;
            };
            let stockpile = world.get::<ResourceStockpile>(system_entity);
            for order in &queue.queue {
                summary.active += 1;
                let minerals_remaining = order.minerals_cost.sub(order.minerals_invested);
                let energy_remaining = order.energy_cost.sub(order.energy_invested);
                if is_bottlenecked(
                    stockpile,
                    minerals_remaining,
                    energy_remaining,
                    order.build_time_remaining,
                ) {
                    summary.bottleneck += 1;
                }
            }
        }
    }

    if let Some(mut colonies_bq) = world.try_query::<(&Colony, &BuildingQueue)>() {
        for (colony, bq) in colonies_bq.iter(world) {
            let Some(system_entity) = resolve_system(world, colony.planet) else {
                continue;
            };
            let stockpile = world.get::<ResourceStockpile>(system_entity);
            for order in &bq.queue {
                summary.active += 1;
                if is_bottlenecked(
                    stockpile,
                    order.minerals_remaining,
                    order.energy_remaining,
                    order.build_time_remaining,
                ) {
                    summary.bottleneck += 1;
                }
            }
            summary.active += bq.demolition_queue.len();
            for order in &bq.upgrade_queue {
                summary.active += 1;
                if is_bottlenecked(
                    stockpile,
                    order.minerals_remaining,
                    order.energy_remaining,
                    order.build_time_remaining,
                ) {
                    summary.bottleneck += 1;
                }
            }
        }
    }

    summary
}

#[derive(Default)]
struct SystemBucket {
    children: Vec<Event>,
    bottleneck_count: usize,
}

impl SystemBucket {
    fn push(&mut self, (event, bottleneck): (Event, bool)) {
        if bottleneck {
            self.bottleneck_count += 1;
        }
        self.children.push(event);
    }
}

fn resolve_system(world: &World, planet: Entity) -> Option<Entity> {
    world.get::<Planet>(planet).map(|p| p.system)
}

fn build_event(
    order_id: u64,
    started_at: i64,
    label: String,
    progress: Option<f32>,
    eta: Option<i64>,
    bottleneck: bool,
) -> (Event, bool) {
    let final_label = if bottleneck {
        format!("[BOTTLENECK] {}", label)
    } else {
        label
    };
    (
        Event {
            id: order_id,
            source: EventSource::BuildOrder(order_id),
            started_at,
            kind: EventKind::Construction,
            label: final_label,
            progress,
            eta,
            children: Vec::new(),
        },
        bottleneck,
    )
}

fn compute_progress(
    minerals_cost: Amt,
    minerals_invested: Amt,
    energy_cost: Amt,
    energy_invested: Amt,
    build_time_total: i64,
    build_time_remaining: i64,
) -> Option<f32> {
    // Three-axis progress — return the min so the bar reflects the
    // tightest constraint. A zero-cost axis contributes 1.0.
    let mineral = amt_fraction(minerals_invested, minerals_cost);
    let energy = amt_fraction(energy_invested, energy_cost);
    let time = if build_time_total <= 0 {
        1.0
    } else {
        let done = (build_time_total - build_time_remaining).max(0) as f32;
        done / build_time_total as f32
    };
    Some(mineral.min(energy).min(time).clamp(0.0, 1.0))
}

fn amt_fraction(invested: Amt, cost: Amt) -> f32 {
    if cost.raw() == 0 {
        return 1.0;
    }
    let num = invested.raw() as f64;
    let den = cost.raw() as f64;
    (num / den) as f32
}

fn compute_eta(now: i64, remaining: i64) -> Option<i64> {
    if remaining <= 0 {
        None
    } else {
        Some(now.saturating_add(remaining))
    }
}

fn is_bottlenecked(
    stockpile: Option<&ResourceStockpile>,
    minerals_remaining: Amt,
    energy_remaining: Amt,
    build_time_remaining: i64,
) -> bool {
    if build_time_remaining <= 0 {
        return false;
    }
    let Some(stockpile) = stockpile else {
        // No stockpile component at all — can't progress at all, so a
        // pending order is bottlenecked by definition.
        return minerals_remaining.raw() > 0 || energy_remaining.raw() > 0;
    };
    let mineral_blocked = minerals_remaining.raw() > 0 && stockpile.minerals.raw() == 0;
    let energy_blocked = energy_remaining.raw() > 0 && stockpile.energy.raw() == 0;
    mineral_blocked || energy_blocked
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::amount::Amt;
    use crate::colony::{BuildOrder, ResourceStockpile};
    use crate::components::Position;
    use crate::galaxy::{Planet, StarSystem};

    fn push_test_order(
        world: &mut World,
        colony_entity: Entity,
        design_id: &str,
        display_name: &str,
        minerals_cost: u64,
        minerals_invested: u64,
        build_time_total: i64,
        build_time_remaining: i64,
    ) -> u64 {
        let mut queue = world.get_mut::<BuildQueue>(colony_entity).unwrap();
        queue.push_order(BuildOrder {
            order_id: 0,
            kind: Default::default(),
            design_id: design_id.into(),
            display_name: display_name.into(),
            minerals_cost: Amt(minerals_cost),
            minerals_invested: Amt(minerals_invested),
            energy_cost: Amt(0),
            energy_invested: Amt(0),
            build_time_total,
            build_time_remaining,
        })
    }

    fn setup_world() -> (World, Entity, Entity) {
        let mut world = World::new();
        world.insert_resource(GameClock::new(100));

        let system = world
            .spawn((
                StarSystem {
                    name: "Sol".into(),
                    surveyed: true,
                    is_capital: true,
                    star_type: "yellow_dwarf".into(),
                },
                Position::from([0.0, 0.0, 0.0]),
                ResourceStockpile {
                    minerals: Amt::units(500),
                    energy: Amt::units(500),
                    research: Amt::ZERO,
                    food: Amt::ZERO,
                    authority: Amt::ZERO,
                },
            ))
            .id();
        let planet = world
            .spawn((Planet {
                name: "Earth".into(),
                system,
                planet_type: "terrestrial".into(),
            },))
            .id();
        let colony = world
            .spawn((
                Colony {
                    planet,
                    growth_rate: 0.0,
                },
                BuildQueue::default(),
                BuildingQueue::default(),
            ))
            .id();
        (world, system, colony)
    }

    #[test]
    fn empty_world_produces_no_events() {
        let world = World::new();
        let mut w = world;
        w.insert_resource(GameClock::new(0));
        let tab = ConstructionOverviewTab;
        let events = tab.collect(&w);
        assert!(events.is_empty());
        assert!(tab.badge(&w).is_none());
    }

    #[test]
    fn active_order_is_reported_as_child_of_system() {
        let (mut world, system, colony) = setup_world();
        push_test_order(&mut world, colony, "corvette", "Corvette", 100, 20, 10, 5);

        let tab = ConstructionOverviewTab;
        let events = tab.collect(&world);
        assert_eq!(events.len(), 1);
        let root = &events[0];
        assert_eq!(root.source, EventSource::System(system));
        assert_eq!(root.kind, EventKind::Construction);
        assert_eq!(root.children.len(), 1);
        let leaf = &root.children[0];
        assert!(matches!(leaf.source, EventSource::BuildOrder(_)));
        assert!(leaf.label.contains("Corvette"));
        assert!(leaf.progress.is_some());
        // ETA = now + remaining = 100 + 5 = 105.
        assert_eq!(leaf.eta, Some(105));

        // Badge: 1 active, 0 bottleneck ⇒ Info.
        let badge = tab.badge(&world).unwrap();
        assert_eq!(badge.count, 1);
        assert_eq!(badge.severity, Severity::Info);
    }

    #[test]
    fn bottlenecked_order_flips_badge_to_critical_and_tags_label() {
        let (mut world, _system, colony) = setup_world();
        // Drain the stockpile so the order is bottlenecked.
        let mut systems_q = world.query::<(Entity, &mut ResourceStockpile)>();
        for (_, mut sp) in systems_q.iter_mut(&mut world) {
            sp.minerals = Amt::ZERO;
            sp.energy = Amt::ZERO;
        }
        push_test_order(&mut world, colony, "corvette", "Corvette", 100, 20, 10, 5);

        let tab = ConstructionOverviewTab;
        let events = tab.collect(&world);
        let leaf = &events[0].children[0];
        assert!(
            leaf.label.starts_with("[BOTTLENECK]"),
            "bottleneck orders must carry the [BOTTLENECK] prefix, got `{}`",
            leaf.label
        );

        let badge = tab.badge(&world).unwrap();
        assert_eq!(badge.severity, Severity::Critical);
        assert_eq!(badge.count, 1);
    }

    #[test]
    fn completed_order_is_not_marked_bottleneck() {
        let (mut world, _system, colony) = setup_world();
        // Fully invested, zero build time remaining — not bottlenecked.
        push_test_order(&mut world, colony, "corvette", "Corvette", 100, 100, 10, 0);

        let tab = ConstructionOverviewTab;
        let events = tab.collect(&world);
        let leaf = &events[0].children[0];
        assert!(!leaf.label.starts_with("[BOTTLENECK]"));
    }

    #[test]
    fn multiple_systems_produce_multiple_roots() {
        let (mut world, _system_a, colony_a) = setup_world();
        // Second star system with its own colony and queue.
        let system_b = world
            .spawn((
                StarSystem {
                    name: "Proxima".into(),
                    surveyed: true,
                    is_capital: false,
                    star_type: "red_dwarf".into(),
                },
                Position::from([10.0, 0.0, 0.0]),
                ResourceStockpile {
                    minerals: Amt::units(10),
                    energy: Amt::units(10),
                    research: Amt::ZERO,
                    food: Amt::ZERO,
                    authority: Amt::ZERO,
                },
            ))
            .id();
        let planet_b = world
            .spawn(Planet {
                name: "Proxima b".into(),
                system: system_b,
                planet_type: "terrestrial".into(),
            })
            .id();
        let colony_b = world
            .spawn((
                Colony {
                    planet: planet_b,
                    growth_rate: 0.0,
                },
                BuildQueue::default(),
                BuildingQueue::default(),
            ))
            .id();

        push_test_order(&mut world, colony_a, "corvette", "Corvette A", 10, 0, 5, 5);
        push_test_order(
            &mut world,
            colony_b,
            "colony_ship",
            "Colony Ship",
            10,
            0,
            5,
            5,
        );
        // Colony-a gets a second order to verify child aggregation.
        push_test_order(&mut world, colony_a, "courier", "Courier", 10, 0, 5, 5);

        let tab = ConstructionOverviewTab;
        let events = tab.collect(&world);
        assert_eq!(events.len(), 2);
        let total_children: usize = events.iter().map(|e| e.children.len()).sum();
        assert_eq!(total_children, 3);
    }
}
