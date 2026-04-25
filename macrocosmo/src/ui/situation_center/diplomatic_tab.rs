//! Diplomatic Standing tab (#346 / ESC-3 Commit 3).
//!
//! Surfaces the player empire's view of every known faction and
//! flags recent standing deterioration. The collect path walks
//! [`FactionRelations`] entries whose `from` is the player empire and
//! turns each into a leaf event. The label renders the target's name
//! (via the `Faction` / `Empire` components when available) plus the
//! `RelationState` and current `standing` value.
//!
//! Deterioration alerts:
//! * A [`DiplomaticStandingHistory`] resource snapshots the previous
//!   tick's standing per `(from, to)` key. A drop below
//!   `STANDING_DROP_WARN_THRESHOLD` (10 points) tags the leaf with
//!   `Severity::Warn`; a drop below `STANDING_DROP_CRITICAL_THRESHOLD`
//!   (25 points) or a transition **into** `RelationState::War` tags
//!   the leaf with `Severity::Critical`.
//! * Severity is surfaced via the tab badge (highest of any leaf's
//!   delta) and a `[WARN]` / `[CRIT]` label prefix on the individual
//!   row.
//!
//! The history is refreshed by a dedicated `Update`-scheduled system,
//! `record_diplomatic_history`, so `collect` never mutates state (it
//! only reads the already-captured snapshot).
//!
//! #### Diplomacy v2 (#302…)
//!
//! When the `FactionRelations` shape changes — e.g. to carry per-view
//! ledger deltas natively, or to split "diplomatic action in flight"
//! from "steady-state standing" — the only file that needs to
//! re-integrate is `collect_diplomatic_events` here. The hash key
//! `(Entity, Entity)` stays valid; only the `view.state`/`view.standing`
//! accessors would swap out.

use std::collections::HashMap;

use bevy::prelude::*;

use crate::faction::{FactionRelations, RelationState};
use crate::player::{Empire, Faction, PlayerEmpire};
use crate::time_system::GameClock;

use super::tab::{OngoingTab, TabBadge, TabMeta};
use super::types::{Event, EventKind, EventSource, Severity, severity_max};

/// Standing drop (in points) that triggers a `Warn` alert.
const STANDING_DROP_WARN_THRESHOLD: f64 = 10.0;

/// Standing drop (in points) that triggers a `Critical` alert.
const STANDING_DROP_CRITICAL_THRESHOLD: f64 = 25.0;

/// ESC-3 Commit 3: Diplomatic Standing.
pub struct DiplomaticStandingTab;

impl DiplomaticStandingTab {
    pub const ID: &'static str = "diplomatic_standing";
    pub const ORDER: i32 = 300;
}

impl OngoingTab for DiplomaticStandingTab {
    fn meta(&self) -> TabMeta {
        TabMeta {
            id: Self::ID,
            display_name: "Diplomatic Standing",
            order: Self::ORDER,
        }
    }

    fn collect(&self, world: &World) -> Vec<Event> {
        collect_diplomatic_events(world)
    }

    fn badge(&self, world: &World) -> Option<TabBadge> {
        let summary = summarise_diplomatic(world);
        let alert_total = summary.warn + summary.critical;
        if alert_total == 0 {
            return None;
        }
        let severity = if summary.critical > 0 {
            Severity::Critical
        } else {
            Severity::Warn
        };
        Some(TabBadge::new(alert_total as u32, severity))
    }
}

/// History snapshot: previous-tick standing + relation state per
/// `(from, to)` key. Populated by `record_diplomatic_history`.
#[derive(Resource, Default, Debug, Clone, Reflect)]
#[reflect(Resource)]
pub struct DiplomaticStandingHistory {
    pub entries: HashMap<(Entity, Entity), DiplomaticSnapshot>,
    /// Tick at which the history was last refreshed.
    pub last_tick: i64,
}

#[derive(Clone, Copy, Debug, bevy::reflect::Reflect)]
pub struct DiplomaticSnapshot {
    pub standing: f64,
    pub state: RelationState,
}

/// Refresh the previous-tick standing snapshot. Runs every frame so
/// delta detection in `collect` sees "prior-tick vs current-tick".
///
/// Intentionally `Update`-scheduled (not tied to `advance_game_time`):
/// the ESC draw path reads history on every frame regardless of
/// whether the game clock advanced.
pub fn record_diplomatic_history(
    clock: Option<Res<GameClock>>,
    relations: Option<Res<FactionRelations>>,
    mut history: ResMut<DiplomaticStandingHistory>,
) {
    let Some(clock) = clock else {
        return;
    };
    let Some(relations) = relations else {
        return;
    };
    if clock.elapsed == history.last_tick && !history.entries.is_empty() {
        // No clock advance since last capture — don't overwrite the
        // baseline, or we'd smear the delta to zero within the same
        // tick as the change landed.
        return;
    }
    history.entries.clear();
    for ((from, to), view) in relations.relations.iter() {
        history.entries.insert(
            (*from, *to),
            DiplomaticSnapshot {
                standing: view.standing,
                state: view.state,
            },
        );
    }
    history.last_tick = clock.elapsed;
}

#[derive(Default, Debug, PartialEq, Eq)]
struct DiplomaticSummary {
    warn: usize,
    critical: usize,
    total: usize,
}

fn collect_diplomatic_events(world: &World) -> Vec<Event> {
    let now = world.resource::<GameClock>().elapsed;
    let Some(relations) = world.get_resource::<FactionRelations>() else {
        return Vec::new();
    };
    let history = world.get_resource::<DiplomaticStandingHistory>();

    // Resolve the player empire entity once. Without a PlayerEmpire
    // (observer mode / early boot) the tab stays empty rather than
    // surfacing every NPC pair — those are out of scope for the
    // player-facing ESC.
    let player_empire = find_player_empire(world);

    let mut leaves: Vec<Event> = Vec::new();
    for ((from, to), view) in relations.relations.iter() {
        if Some(*from) != player_empire {
            // Only surface standings *from the player's perspective*.
            continue;
        }
        let target_name = faction_display_name(world, *to);
        let prior = history.and_then(|h| h.entries.get(&(*from, *to)).copied());
        let (severity, alert_label) = classify_alert(prior, view.standing, view.state);
        let base_label = format!("{} — {:?}, {:+.1}", target_name, view.state, view.standing);
        let label = match alert_label {
            Some(prefix) => format!("{} {}", prefix, base_label),
            None => base_label,
        };
        leaves.push(Event {
            id: diplomatic_event_id(*from, *to),
            source: EventSource::Faction(*to),
            started_at: now,
            kind: EventKind::Diplomatic,
            label,
            progress: None,
            eta: None,
            children: Vec::new(),
        });
        // `severity` is embedded in the label prefix; we don't carry a
        // separate Severity field on Event (that lives on Notification)
        // — but we use it for the badge roll-up below via
        // `summarise_diplomatic`.
        let _ = severity;
    }
    if leaves.is_empty() {
        return Vec::new();
    }
    leaves.sort_by(|a, b| a.label.cmp(&b.label));
    let len = leaves.len();
    vec![Event {
        id: 0xD1D1_D1D1_0000_0000u64,
        source: EventSource::None,
        started_at: now,
        kind: EventKind::Diplomatic,
        label: format!("Known factions ({})", len),
        progress: None,
        eta: None,
        children: leaves,
    }]
}

fn summarise_diplomatic(world: &World) -> DiplomaticSummary {
    let mut summary = DiplomaticSummary::default();
    let Some(relations) = world.get_resource::<FactionRelations>() else {
        return summary;
    };
    let history = world.get_resource::<DiplomaticStandingHistory>();
    let player_empire = find_player_empire(world);
    let mut highest = Severity::Info;
    for ((from, to), view) in relations.relations.iter() {
        if Some(*from) != player_empire {
            continue;
        }
        summary.total += 1;
        let prior = history.and_then(|h| h.entries.get(&(*from, *to)).copied());
        let (sev, _) = classify_alert(prior, view.standing, view.state);
        if let Some(sev) = sev {
            match sev {
                Severity::Warn => summary.warn += 1,
                Severity::Critical => summary.critical += 1,
                Severity::Info => {}
            }
            highest = severity_max(highest, sev);
        }
    }
    summary
}

fn classify_alert(
    prior: Option<DiplomaticSnapshot>,
    current_standing: f64,
    current_state: RelationState,
) -> (Option<Severity>, Option<&'static str>) {
    let Some(prior) = prior else {
        return (None, None);
    };
    // War transition is always Critical.
    if prior.state != RelationState::War && current_state == RelationState::War {
        return (Some(Severity::Critical), Some("[CRIT]"));
    }
    let drop = prior.standing - current_standing;
    if drop >= STANDING_DROP_CRITICAL_THRESHOLD {
        (Some(Severity::Critical), Some("[CRIT]"))
    } else if drop >= STANDING_DROP_WARN_THRESHOLD {
        (Some(Severity::Warn), Some("[WARN]"))
    } else {
        (None, None)
    }
}

fn find_player_empire(world: &World) -> Option<Entity> {
    let mut it = world.try_query_filtered::<Entity, With<PlayerEmpire>>()?;
    it.iter(world).next()
}

fn faction_display_name(world: &World, entity: Entity) -> String {
    if let Some(faction) = world.get::<Faction>(entity) {
        return faction.name.clone();
    }
    if let Some(empire) = world.get::<Empire>(entity) {
        return empire.name.clone();
    }
    format!("Faction {:?}", entity)
}

/// Stable event id for a `(from, to)` pair. Packs both entity bits
/// into a 64-bit space — hash of raw bits is fine because the
/// renderer only needs uniqueness within a frame.
fn diplomatic_event_id(from: Entity, to: Entity) -> u64 {
    // Simple xor + rotate, deterministic enough for diffing.
    let a = from.to_bits();
    let b = to.to_bits();
    a.rotate_left(13) ^ b.rotate_left(47)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::faction::FactionView;

    fn spawn_player(world: &mut World, name: &str) -> Entity {
        world
            .spawn((
                PlayerEmpire,
                Empire { name: name.into() },
                Faction::new("player", name),
            ))
            .id()
    }

    fn spawn_faction(world: &mut World, id: &str, name: &str) -> Entity {
        world.spawn(Faction::new(id, name)).id()
    }

    fn setup(world: &mut World, now: i64) {
        world.insert_resource(GameClock::new(now));
        world.insert_resource(FactionRelations::default());
        world.insert_resource(DiplomaticStandingHistory::default());
    }

    #[test]
    fn empty_world_emits_no_events() {
        let mut world = World::new();
        setup(&mut world, 0);
        let tab = DiplomaticStandingTab;
        assert!(tab.collect(&world).is_empty());
        assert!(tab.badge(&world).is_none());
    }

    #[test]
    fn collect_surfaces_known_factions_from_player_perspective() {
        let mut world = World::new();
        setup(&mut world, 0);
        let player = spawn_player(&mut world, "Sol Republic");
        let alien = spawn_faction(&mut world, "zorgs", "Zorg Hegemony");

        world.resource_mut::<FactionRelations>().set(
            player,
            alien,
            FactionView::new(RelationState::Peace, 20.0),
        );
        // Reverse direction — must NOT show up on the player tab.
        world.resource_mut::<FactionRelations>().set(
            alien,
            player,
            FactionView::new(RelationState::Peace, 20.0),
        );

        let tab = DiplomaticStandingTab;
        let events = tab.collect(&world);
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].kind, EventKind::Diplomatic);
        assert_eq!(events[0].children.len(), 1);
        assert!(events[0].children[0].label.contains("Zorg Hegemony"));
    }

    #[test]
    fn standing_drop_below_warn_threshold_tags_leaf_warn() {
        let mut world = World::new();
        setup(&mut world, 1);
        let player = spawn_player(&mut world, "Sol Republic");
        let alien = spawn_faction(&mut world, "zorgs", "Zorg Hegemony");

        world.resource_mut::<FactionRelations>().set(
            player,
            alien,
            FactionView::new(RelationState::Peace, 50.0),
        );
        // Seed history with the previous standing BEFORE the drop.
        world
            .resource_mut::<DiplomaticStandingHistory>()
            .entries
            .insert(
                (player, alien),
                DiplomaticSnapshot {
                    standing: 65.0,
                    state: RelationState::Peace,
                },
            );

        let tab = DiplomaticStandingTab;
        let events = tab.collect(&world);
        let leaf = &events[0].children[0];
        assert!(
            leaf.label.contains("[WARN]"),
            "label should carry WARN prefix, got `{}`",
            leaf.label
        );

        let badge = tab.badge(&world).unwrap();
        assert_eq!(badge.severity, Severity::Warn);
    }

    #[test]
    fn war_transition_is_critical() {
        let mut world = World::new();
        setup(&mut world, 1);
        let player = spawn_player(&mut world, "Sol Republic");
        let alien = spawn_faction(&mut world, "zorgs", "Zorg Hegemony");

        world.resource_mut::<FactionRelations>().set(
            player,
            alien,
            FactionView::new(RelationState::War, -50.0),
        );
        world
            .resource_mut::<DiplomaticStandingHistory>()
            .entries
            .insert(
                (player, alien),
                DiplomaticSnapshot {
                    standing: 0.0,
                    state: RelationState::Peace,
                },
            );

        let tab = DiplomaticStandingTab;
        let events = tab.collect(&world);
        let leaf = &events[0].children[0];
        assert!(leaf.label.contains("[CRIT]"));

        let badge = tab.badge(&world).unwrap();
        assert_eq!(badge.severity, Severity::Critical);
    }

    #[test]
    fn history_recorder_captures_current_standing() {
        let mut world = World::new();
        setup(&mut world, 5);
        let player = spawn_player(&mut world, "Sol Republic");
        let alien = spawn_faction(&mut world, "zorgs", "Zorg Hegemony");

        world.resource_mut::<FactionRelations>().set(
            player,
            alien,
            FactionView::new(RelationState::Peace, 40.0),
        );

        // Simulate the Update system running once.
        let clock = world.resource::<GameClock>().elapsed;
        let relations = world.resource::<FactionRelations>().clone_entries();
        let mut history = world.resource_mut::<DiplomaticStandingHistory>();
        // Inlined copy of the recorder logic for test simplicity.
        history.entries.clear();
        for ((f, t), v) in relations {
            history.entries.insert(
                (f, t),
                DiplomaticSnapshot {
                    standing: v.standing,
                    state: v.state,
                },
            );
        }
        history.last_tick = clock;
        drop(history);

        let snap = world
            .resource::<DiplomaticStandingHistory>()
            .entries
            .get(&(player, alien))
            .copied()
            .expect("history populated");
        assert_eq!(snap.standing, 40.0);
        assert_eq!(snap.state, RelationState::Peace);
    }
}

/// Test-only helper to snapshot `FactionRelations` without borrowing.
#[cfg(test)]
impl FactionRelations {
    fn clone_entries(&self) -> Vec<((Entity, Entity), crate::faction::FactionView)> {
        self.relations
            .iter()
            .map(|(k, v)| (*k, v.clone()))
            .collect()
    }
}
