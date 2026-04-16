//! Resource Trends tab (#346 / ESC-3 Commit 4).
//!
//! Surfaces per-resource trend history across the player empire.
//! Unlike the other three ongoing tabs, the body includes per-resource
//! sparkline plots that the default Event-tree renderer cannot draw —
//! this tab therefore implements [`SituationTab`] directly and
//! overrides `render`.
//!
//! Data flow:
//! 1. A dedicated [`record_resource_trends`] system (scheduled in
//!    `Update`) aggregates the player empire's resource totals from
//!    every `ResourceStockpile` into a [`ResourceTrendHistory`]
//!    resource. The resource keeps a bounded ring of recent samples
//!    keyed by `ResourceKind`.
//! 2. [`collect_resource_events`] reads that buffer and emits five
//!    leaf events (one per resource kind). Each leaf carries current
//!    value in its label, and a severity prefix when a recent slope
//!    drops by more than `RESOURCE_DROP_ALERT_FRACTION`.
//! 3. `render` walks the Events and, per leaf, draws a tiny
//!    sparkline beneath the label using egui's `plot` primitives.
//!
//! Clicking through to top-bar resource detail is out of scope for
//! this commit (integration lives alongside the top bar — see §plan
//! deviation notes in the PR body). The tree leaves carry
//! `EventSource::Empire(player)` so a future click-handler can route
//! the navigation without a schema change.

use std::any::Any;
use std::collections::{HashMap, VecDeque};

use bevy::prelude::*;
use bevy_egui::egui;

use crate::colony::ResourceStockpile;
use crate::galaxy::StarSystem;
use crate::player::PlayerEmpire;
use crate::time_system::GameClock;

use super::state::TabState;
use super::tab::{SituationTab, TabBadge, TabMeta};
use super::types::{Event, EventKind, EventSource, Severity};

/// Keep 60 most-recent samples (1 year at 1-hexady sampling, or 1
/// minute of real time at 1 sample/s game pace). Tuned to keep the
/// sparkline legible on the default tab width.
pub const RESOURCE_TREND_HISTORY_LEN: usize = 60;

/// Fractional drop (current / peak-over-window) below which the
/// resource is flagged as declining. `0.25` ⇒ alert if current is
/// less than 75% of the recent peak.
pub const RESOURCE_DROP_ALERT_FRACTION: f64 = 0.25;

/// Resources tracked by the trends buffer.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ResourceKind {
    Minerals,
    Energy,
    Research,
    Food,
    Authority,
}

impl ResourceKind {
    pub const ALL: [ResourceKind; 5] = [
        ResourceKind::Minerals,
        ResourceKind::Energy,
        ResourceKind::Research,
        ResourceKind::Food,
        ResourceKind::Authority,
    ];

    fn label(self) -> &'static str {
        match self {
            ResourceKind::Minerals => "Minerals",
            ResourceKind::Energy => "Energy",
            ResourceKind::Research => "Research",
            ResourceKind::Food => "Food",
            ResourceKind::Authority => "Authority",
        }
    }
}

/// One sample = tick + value for each tracked resource. Stored as a
/// tagged struct rather than a HashMap per sample to keep the plot
/// path allocation-free during `render`.
#[derive(Clone, Copy, Debug)]
pub struct ResourceSample {
    pub tick: i64,
    pub minerals: f64,
    pub energy: f64,
    pub research: f64,
    pub food: f64,
    pub authority: f64,
}

impl ResourceSample {
    fn value(&self, kind: ResourceKind) -> f64 {
        match kind {
            ResourceKind::Minerals => self.minerals,
            ResourceKind::Energy => self.energy,
            ResourceKind::Research => self.research,
            ResourceKind::Food => self.food,
            ResourceKind::Authority => self.authority,
        }
    }
}

/// Bounded ring buffer of empire-wide resource totals.
#[derive(Resource, Default, Debug, Clone)]
pub struct ResourceTrendHistory {
    pub samples: VecDeque<ResourceSample>,
    pub last_tick: i64,
}

impl ResourceTrendHistory {
    fn push(&mut self, sample: ResourceSample) {
        self.samples.push_back(sample);
        while self.samples.len() > RESOURCE_TREND_HISTORY_LEN {
            self.samples.pop_front();
        }
        self.last_tick = sample.tick;
    }

    /// Current value across all tracked resources (most recent sample).
    fn current(&self) -> HashMap<ResourceKind, f64> {
        let mut out = HashMap::new();
        if let Some(sample) = self.samples.back() {
            for kind in ResourceKind::ALL {
                out.insert(kind, sample.value(kind));
            }
        }
        out
    }

    /// Peak observed for `kind` across the window.
    fn peak(&self, kind: ResourceKind) -> f64 {
        self.samples
            .iter()
            .map(|s| s.value(kind))
            .fold(0.0_f64, f64::max)
    }
}

/// Aggregate every system's stockpile once per frame, pushing a new
/// sample into `ResourceTrendHistory`. Intentionally gated on
/// clock advance so a paused game doesn't flood the buffer with
/// duplicates.
pub fn record_resource_trends(
    clock: Option<Res<GameClock>>,
    mut history: ResMut<ResourceTrendHistory>,
    stockpiles: Query<&ResourceStockpile, With<StarSystem>>,
    _player: Query<Entity, With<PlayerEmpire>>,
) {
    let Some(clock) = clock else {
        return;
    };
    // Same-tick dedupe: the `Update` schedule fires multiple times per
    // in-game tick when the game is paused; we only want one sample
    // per distinct clock value.
    if !history.samples.is_empty() && clock.elapsed == history.last_tick {
        return;
    }
    // NB: totals currently span every system's stockpile regardless of
    // ownership. Light-speed correctness (per `KnowledgeStore`) lives
    // in the top bar; the ESC trends view is a local, real-time roll
    // up matching the on-screen resource readout. When a proper per-
    // empire aggregator lands (#268-ish), this lookup swaps for a
    // `KnowledgeStore` walk.
    let mut sample = ResourceSample {
        tick: clock.elapsed,
        minerals: 0.0,
        energy: 0.0,
        research: 0.0,
        food: 0.0,
        authority: 0.0,
    };
    for sp in &stockpiles {
        sample.minerals += sp.minerals.raw() as f64;
        sample.energy += sp.energy.raw() as f64;
        sample.research += sp.research.raw() as f64;
        sample.food += sp.food.raw() as f64;
        sample.authority += sp.authority.raw() as f64;
    }
    history.push(sample);
}

/// ESC-3 Commit 4: Resource Trends.
pub struct ResourceTrendsTab;

impl ResourceTrendsTab {
    pub const ID: &'static str = "resource_trends";
    pub const ORDER: i32 = 400;
}

impl SituationTab for ResourceTrendsTab {
    fn meta(&self) -> TabMeta {
        TabMeta {
            id: Self::ID,
            display_name: "Resource Trends",
            order: Self::ORDER,
        }
    }

    fn badge(&self, world: &World) -> Option<TabBadge> {
        let alerts = count_resource_alerts(world);
        if alerts == 0 {
            return None;
        }
        Some(TabBadge::new(alerts as u32, Severity::Warn))
    }

    fn render(&self, ui: &mut egui::Ui, world: &World, _state: &mut TabState) {
        let events = collect_resource_events(world);
        let history = world.get_resource::<ResourceTrendHistory>();
        if events.is_empty() {
            ui.label(egui::RichText::new("(no resource samples yet)").weak());
            return;
        }
        for event in &events {
            ui.horizontal(|ui| {
                ui.label(&event.label);
            });
            if let Some(history) = history {
                // Render the sparkline inline below the label row.
                let kind = kind_from_event(event);
                if let Some(kind) = kind {
                    draw_sparkline(ui, history, kind);
                }
            }
            ui.add_space(6.0);
        }
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

fn kind_from_event(event: &Event) -> Option<ResourceKind> {
    // The leaf label starts with the resource name; match by prefix.
    // This keeps `EventKind::Resource` as a closed v1 enum (per
    // plan-326-esc.md) without a `Custom(String)` variant.
    for k in ResourceKind::ALL {
        if event.label.contains(k.label()) {
            return Some(k);
        }
    }
    None
}

fn draw_sparkline(ui: &mut egui::Ui, history: &ResourceTrendHistory, kind: ResourceKind) {
    let values: Vec<f64> = history.samples.iter().map(|s| s.value(kind)).collect();
    let width = 180.0f32;
    let height = 24.0f32;
    let (rect, _) = ui.allocate_exact_size(egui::vec2(width, height), egui::Sense::hover());
    let painter = ui.painter_at(rect);
    if values.is_empty() {
        painter.text(
            rect.center(),
            egui::Align2::CENTER_CENTER,
            "(no samples)",
            egui::FontId::monospace(10.0),
            egui::Color32::DARK_GRAY,
        );
        return;
    }
    let min = values.iter().copied().fold(f64::INFINITY, f64::min);
    let max = values.iter().copied().fold(f64::NEG_INFINITY, f64::max);
    let span = (max - min).max(1.0);
    let n = values.len();
    if n < 2 {
        // Single sample ⇒ draw a dot at the centre.
        let x = rect.center().x;
        let y = rect.center().y;
        painter.circle_filled(egui::pos2(x, y), 1.5, egui::Color32::LIGHT_BLUE);
        return;
    }
    let step = rect.width() / (n as f32 - 1.0);
    let mut prev: Option<egui::Pos2> = None;
    for (i, v) in values.iter().enumerate() {
        let t = ((v - min) / span) as f32;
        // Higher resource value ⇒ higher pixel (egui y grows down, so
        // invert).
        let y = rect.bottom() - t * rect.height();
        let x = rect.left() + i as f32 * step;
        let p = egui::pos2(x, y);
        if let Some(prev) = prev {
            painter.line_segment([prev, p], egui::Stroke::new(1.0, egui::Color32::LIGHT_BLUE));
        }
        prev = Some(p);
    }
}

fn collect_resource_events(world: &World) -> Vec<Event> {
    let now = world.resource::<GameClock>().elapsed;
    let Some(history) = world.get_resource::<ResourceTrendHistory>() else {
        return Vec::new();
    };
    if history.samples.is_empty() {
        return Vec::new();
    }
    let current = history.current();
    let player = world
        .try_query_filtered::<Entity, With<PlayerEmpire>>()
        .and_then(|mut q| q.iter(world).next());

    ResourceKind::ALL
        .iter()
        .map(|kind| {
            let value = current.get(kind).copied().unwrap_or(0.0);
            let peak = history.peak(*kind);
            let alert = is_declining(value, peak);
            let base_label = format!("{}: {:.0}", kind.label(), value);
            let label = if alert {
                format!("[WARN] {}", base_label)
            } else {
                base_label
            };
            Event {
                id: hash_resource_kind(*kind),
                source: player.map(EventSource::Empire).unwrap_or(EventSource::None),
                started_at: now,
                kind: EventKind::Resource,
                label,
                progress: None,
                eta: None,
                children: Vec::new(),
            }
        })
        .collect()
}

fn is_declining(current: f64, peak: f64) -> bool {
    if peak <= 0.0 {
        return false;
    }
    let ratio = current / peak;
    (1.0 - ratio) >= RESOURCE_DROP_ALERT_FRACTION
}

fn count_resource_alerts(world: &World) -> usize {
    let Some(history) = world.get_resource::<ResourceTrendHistory>() else {
        return 0;
    };
    if history.samples.is_empty() {
        return 0;
    }
    let current = history.current();
    ResourceKind::ALL
        .iter()
        .filter(|kind| {
            let value = current.get(kind).copied().unwrap_or(0.0);
            let peak = history.peak(**kind);
            is_declining(value, peak)
        })
        .count()
}

fn hash_resource_kind(kind: ResourceKind) -> u64 {
    // Disjoint from Entity.to_bits() by high-bit tag.
    0xCA5E_0000_0000_0000 | (kind as u64)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::amount::Amt;
    use crate::colony::ResourceStockpile;
    use crate::components::Position;
    use crate::galaxy::StarSystem;

    fn spawn_system_with_stockpile(world: &mut World, sp: ResourceStockpile) -> Entity {
        world
            .spawn((
                StarSystem {
                    name: "Sol".into(),
                    surveyed: true,
                    is_capital: true,
                    star_type: "yellow_dwarf".into(),
                },
                Position::from([0.0, 0.0, 0.0]),
                sp,
            ))
            .id()
    }

    #[test]
    fn empty_history_emits_no_events() {
        let mut world = World::new();
        world.insert_resource(GameClock::new(0));
        world.insert_resource(ResourceTrendHistory::default());
        let tab = ResourceTrendsTab;
        let events = collect_resource_events(&world);
        assert!(events.is_empty());
        assert!(tab.badge(&world).is_none());
    }

    #[test]
    fn single_sample_produces_five_leaves() {
        let mut world = World::new();
        world.insert_resource(GameClock::new(0));
        let mut history = ResourceTrendHistory::default();
        history.push(ResourceSample {
            tick: 0,
            minerals: 100.0,
            energy: 50.0,
            research: 10.0,
            food: 30.0,
            authority: 5.0,
        });
        world.insert_resource(history);
        let events = collect_resource_events(&world);
        assert_eq!(events.len(), 5);
        assert!(events[0].label.contains("Minerals"));
        assert!(events[0].label.contains("100"));
        assert!(events.iter().all(|e| e.kind == EventKind::Resource));
    }

    #[test]
    fn sharp_drop_flags_warn_alert() {
        let mut world = World::new();
        world.insert_resource(GameClock::new(2));
        let mut history = ResourceTrendHistory::default();
        // Peak at 1000, then crash to 100.
        history.push(ResourceSample {
            tick: 0,
            minerals: 1000.0,
            energy: 0.0,
            research: 0.0,
            food: 0.0,
            authority: 0.0,
        });
        history.push(ResourceSample {
            tick: 1,
            minerals: 500.0,
            energy: 0.0,
            research: 0.0,
            food: 0.0,
            authority: 0.0,
        });
        history.push(ResourceSample {
            tick: 2,
            minerals: 100.0,
            energy: 0.0,
            research: 0.0,
            food: 0.0,
            authority: 0.0,
        });
        world.insert_resource(history);

        let events = collect_resource_events(&world);
        let mineral_leaf = events
            .iter()
            .find(|e| e.label.contains("Minerals"))
            .unwrap();
        assert!(
            mineral_leaf.label.contains("[WARN]"),
            "expected warn prefix, got `{}`",
            mineral_leaf.label
        );

        let tab = ResourceTrendsTab;
        let badge = tab.badge(&world).unwrap();
        assert_eq!(badge.severity, Severity::Warn);
        assert_eq!(badge.count, 1);
    }

    #[test]
    fn recorder_aggregates_all_stockpiles() {
        let mut world = World::new();
        world.insert_resource(GameClock::new(1));
        world.insert_resource(ResourceTrendHistory::default());
        spawn_system_with_stockpile(
            &mut world,
            ResourceStockpile {
                minerals: Amt::units(10),
                energy: Amt::units(20),
                research: Amt::ZERO,
                food: Amt::units(5),
                authority: Amt::ZERO,
            },
        );
        spawn_system_with_stockpile(
            &mut world,
            ResourceStockpile {
                minerals: Amt::units(15),
                energy: Amt::ZERO,
                research: Amt::units(3),
                food: Amt::ZERO,
                authority: Amt::ZERO,
            },
        );

        // Drive the recorder by calling it once via a schedule.
        let mut schedule = Schedule::default();
        schedule.add_systems(record_resource_trends);
        schedule.run(&mut world);

        let history = world.resource::<ResourceTrendHistory>();
        assert_eq!(history.samples.len(), 1);
        let s = history.samples.back().unwrap();
        // `Amt::units(n)` stores `n * SCALE` internally (SCALE = 1000).
        // Totals: 25 * 1000 minerals, 20 * 1000 energy.
        assert!((s.minerals - 25_000.0).abs() < 1e-6);
        assert!((s.energy - 20_000.0).abs() < 1e-6);
    }

    #[test]
    fn recorder_dedupes_same_tick_samples() {
        let mut world = World::new();
        world.insert_resource(GameClock::new(0));
        world.insert_resource(ResourceTrendHistory::default());
        spawn_system_with_stockpile(
            &mut world,
            ResourceStockpile {
                minerals: Amt::units(5),
                energy: Amt::ZERO,
                research: Amt::ZERO,
                food: Amt::ZERO,
                authority: Amt::ZERO,
            },
        );

        let mut schedule = Schedule::default();
        schedule.add_systems(record_resource_trends);
        schedule.run(&mut world);
        schedule.run(&mut world);
        // Second run is the same tick ⇒ no new sample.
        assert_eq!(world.resource::<ResourceTrendHistory>().samples.len(), 1);

        // Advance the clock and re-run.
        world.resource_mut::<GameClock>().elapsed = 1;
        schedule.run(&mut world);
        assert_eq!(world.resource::<ResourceTrendHistory>().samples.len(), 2);
    }

    #[test]
    fn render_does_not_panic_on_empty_or_populated_history() {
        // Exercise the render path without a real Bevy App / egui
        // context by spinning up a detached egui::Context.
        let mut world = World::new();
        world.insert_resource(GameClock::new(0));
        world.insert_resource(ResourceTrendHistory::default());
        let tab = ResourceTrendsTab;
        let ctx = egui::Context::default();
        let mut state = TabState::default();
        ctx.run(Default::default(), |ctx| {
            egui::Area::new(egui::Id::new("rt_test_area")).show(ctx, |ui| {
                tab.render(ui, &world, &mut state);
            });
        });

        // Populated history path.
        let mut hist = world.resource_mut::<ResourceTrendHistory>();
        hist.push(ResourceSample {
            tick: 0,
            minerals: 1.0,
            energy: 2.0,
            research: 3.0,
            food: 4.0,
            authority: 5.0,
        });
        hist.push(ResourceSample {
            tick: 1,
            minerals: 2.0,
            energy: 3.0,
            research: 4.0,
            food: 5.0,
            authority: 6.0,
        });
        drop(hist);

        let ctx = egui::Context::default();
        ctx.run(Default::default(), |ctx| {
            egui::Area::new(egui::Id::new("rt_test_area_2")).show(ctx, |ui| {
                tab.render(ui, &world, &mut state);
            });
        });
    }
}
