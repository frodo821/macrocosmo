//! Notification banner system (#151).
//!
//! Provides a queue of in-flight banner notifications shown across the top of
//! the screen. Notifications come from two sources:
//!
//! 1. Automatic mapping from important `GameEvent`s
//! 2. The Lua `show_notification { ... }` API
//!
//! TTL is tracked in real seconds (not game time) so the banner UX is
//! consistent regardless of game speed and pause state. Long-running banners
//! ("high" priority) have no TTL and must be dismissed manually.

use bevy::prelude::*;

use crate::events::{GameEvent, GameEventKind};
use crate::knowledge::{EventId, NotifiedEventIds, PendingFactQueue, PerceivedFact};
use crate::scripting::ScriptEngine;
use crate::time_system::{GameClock, GameSpeed};

/// Severity / behavior class for a banner notification.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NotificationPriority {
    /// Routine information. Goes to the event log only — never produces a
    /// banner.
    Low,
    /// Notable but non-critical event. Shown as a banner that disappears
    /// after a few seconds.
    Medium,
    /// Critical event. Shown as a banner that stays until the player
    /// dismisses it. Also auto-pauses the game.
    High,
}

impl NotificationPriority {
    /// Returns `true` if this priority should be displayed as a banner
    /// (Medium and High). Low priority is log-only.
    pub fn shows_banner(&self) -> bool {
        matches!(self, NotificationPriority::Medium | NotificationPriority::High)
    }

    /// Real-time TTL for the banner, or `None` if the banner is sticky and
    /// must be dismissed manually.
    pub fn default_ttl_seconds(&self) -> Option<f32> {
        match self {
            NotificationPriority::Medium => Some(6.0),
            NotificationPriority::High => None,
            NotificationPriority::Low => Some(0.0),
        }
    }

    /// Whether this priority should auto-pause the game when shown.
    pub fn pauses_game(&self) -> bool {
        matches!(self, NotificationPriority::High)
    }

    /// Parse a Lua-side priority string. Defaults to `Medium` for unknown
    /// values to keep scripts forward-compatible.
    pub fn from_str(s: &str) -> Self {
        match s {
            "low" => NotificationPriority::Low,
            "high" => NotificationPriority::High,
            _ => NotificationPriority::Medium,
        }
    }
}

/// One in-flight banner notification.
#[derive(Clone, Debug)]
pub struct Notification {
    pub id: u64,
    pub title: String,
    pub description: String,
    /// Free-form icon identifier. Stored verbatim from Lua; not rendered yet
    /// because the icon registry (#143) is not implemented.
    #[allow(dead_code)]
    pub icon: Option<String>,
    pub priority: NotificationPriority,
    /// Optional star system the banner can jump to when clicked.
    pub target_system: Option<Entity>,
    /// Real-time seconds remaining before auto-dismiss. `None` = sticky.
    pub remaining_seconds: Option<f32>,
}

/// Resource holding all currently-displayed banner notifications.
///
/// Newest notifications are at the *front* of the queue (index 0) so the UI
/// renders them at the top of the stack.
#[derive(Resource, Default)]
pub struct NotificationQueue {
    pub items: Vec<Notification>,
    next_id: u64,
    pub max_items: usize,
}

impl NotificationQueue {
    /// Maximum number of notifications retained simultaneously. Beyond this
    /// the oldest entries are evicted from the bottom of the stack.
    pub const DEFAULT_MAX_ITEMS: usize = 8;

    pub fn new() -> Self {
        Self {
            items: Vec::new(),
            next_id: 1,
            max_items: Self::DEFAULT_MAX_ITEMS,
        }
    }

    /// Push a new notification. Low-priority notifications are silently
    /// dropped (they belong only to the event log). Returns the assigned id
    /// or `None` if it was dropped.
    pub fn push(
        &mut self,
        title: impl Into<String>,
        description: impl Into<String>,
        icon: Option<String>,
        priority: NotificationPriority,
        target_system: Option<Entity>,
    ) -> Option<u64> {
        if !priority.shows_banner() {
            return None;
        }

        let id = self.next_id;
        self.next_id = self.next_id.wrapping_add(1);

        let n = Notification {
            id,
            title: title.into(),
            description: description.into(),
            icon,
            priority,
            target_system,
            remaining_seconds: priority.default_ttl_seconds(),
        };

        // Newest at the front of the stack
        self.items.insert(0, n);

        // Evict oldest if we exceed the cap
        while self.items.len() > self.max_items {
            self.items.pop();
        }

        Some(id)
    }

    /// Remove a notification by id (used when the player dismisses one).
    pub fn dismiss(&mut self, id: u64) -> bool {
        let before = self.items.len();
        self.items.retain(|n| n.id != id);
        before != self.items.len()
    }

    /// Tick all notifications by `delta_seconds` and remove any whose TTL
    /// reached zero. Sticky (None TTL) entries are untouched.
    pub fn tick(&mut self, delta_seconds: f32) {
        for item in self.items.iter_mut() {
            if let Some(ref mut t) = item.remaining_seconds {
                *t -= delta_seconds;
            }
        }
        self.items
            .retain(|item| item.remaining_seconds.is_none_or(|t| t > 0.0));
    }
}

impl Default for Notification {
    fn default() -> Self {
        Self {
            id: 0,
            title: String::new(),
            description: String::new(),
            icon: None,
            priority: NotificationPriority::Medium,
            target_system: None,
            remaining_seconds: None,
        }
    }
}

/// Per-frame system that decays notification TTLs and removes expired ones.
///
/// Real-time (`Time`) is used deliberately so the banner UX is independent
/// of game speed / pause. Only TTL is decremented during pause — the
/// banners themselves remain dismissible.
pub fn tick_notifications(time: Res<Time>, mut queue: ResMut<NotificationQueue>) {
    let dt = time.delta_secs();
    if dt <= 0.0 {
        return;
    }
    queue.tick(dt);
}

/// Map a `GameEventKind` to a notification priority. `None` means the event
/// kind should not produce an automatic banner (it is still recorded in the
/// `EventLog` by the existing `collect_events` system).
pub fn priority_for_event_kind(kind: &GameEventKind) -> Option<NotificationPriority> {
    match kind {
        GameEventKind::SurveyComplete => Some(NotificationPriority::Medium),
        GameEventKind::SurveyDiscovery => Some(NotificationPriority::High),
        GameEventKind::ColonyEstablished => Some(NotificationPriority::High),
        GameEventKind::CombatVictory => Some(NotificationPriority::High),
        GameEventKind::CombatDefeat => Some(NotificationPriority::High),
        GameEventKind::HostileDetected => Some(NotificationPriority::High),
        GameEventKind::PlayerRespawn => Some(NotificationPriority::High),
        GameEventKind::ResourceAlert => Some(NotificationPriority::Medium),
        // Other events are routine and do not deserve a banner.
        _ => None,
    }
}

/// Short human-readable title for an automatic event banner. Description
/// uses the `GameEvent.description` field directly.
fn title_for_event_kind(kind: &GameEventKind) -> &'static str {
    match kind {
        GameEventKind::SurveyComplete => "Survey Complete",
        GameEventKind::SurveyDiscovery => "Discovery",
        GameEventKind::ColonyEstablished => "Colony Established",
        GameEventKind::CombatVictory => "Combat Victory",
        GameEventKind::CombatDefeat => "Combat Defeat",
        GameEventKind::HostileDetected => "Hostile Detected",
        GameEventKind::PlayerRespawn => "Player Respawn",
        GameEventKind::ResourceAlert => "Resource Alert",
        GameEventKind::ShipArrived => "Ship Arrived",
        GameEventKind::ShipBuilt => "Ship Built",
        GameEventKind::BuildingDemolished => "Building Demolished",
        GameEventKind::ShipScrapped => "Ship Scrapped",
        GameEventKind::ColonyFailed => "Colony Failed",
        GameEventKind::AnomalyDiscovered => "Anomaly Discovered",
    }
}

/// #233: Whitelist of `GameEventKind` variants still routed through the
/// legacy `GameEvent → NotificationQueue` path. All other world-facing kinds
/// now flow through `PendingFactQueue` (light-speed / relay-delayed).
///
/// The whitelist is intentionally narrow: only events whose information
/// *cannot* be delayed without breaking the gameplay contract. The player
/// respawn is an engine-level fact (not a remote observation) and the
/// resource alert is a capital-aggregated warning with no light-speed origin.
pub fn is_legacy_whitelisted(kind: &GameEventKind) -> bool {
    matches!(
        kind,
        GameEventKind::PlayerRespawn | GameEventKind::ResourceAlert,
    )
}

/// System that mirrors **whitelisted** `GameEvent`s into the notification
/// queue (#233). Non-whitelisted world events are routed through the
/// `PendingFactQueue` pipeline instead — this system intentionally ignores
/// them to avoid double-notifications in the dual-write transition window.
///
/// Runs alongside (not instead of) `collect_events` so the event log is
/// preserved for every `GameEvent`.
pub fn auto_notify_from_events(
    mut reader: MessageReader<GameEvent>,
    mut queue: ResMut<NotificationQueue>,
    mut notified: ResMut<NotifiedEventIds>,
) {
    for event in reader.read() {
        if !is_legacy_whitelisted(&event.kind) {
            continue;
        }
        // #249: Dedupe — if a paired KnowledgeFact already surfaced a banner
        // for this id (or will later), skip this one. `EventId::default()`
        // (== 0) is the pre-migration "no id" sentinel; treat it as never
        // previously notified so legacy code paths keep working.
        if event.id != EventId::default() && !notified.mark(event.id) {
            continue;
        }
        if let Some(priority) = priority_for_event_kind(&event.kind) {
            queue.push(
                title_for_event_kind(&event.kind).to_string(),
                event.description.clone(),
                None,
                priority,
                event.related_system,
            );
        }
    }
}

/// #233: Drain facts whose arrival time has been reached and push them into
/// the notification queue. Runs after `advance_game_time` so fresh facts
/// written the same tick can arrive at `observed_at == clock.elapsed`.
///
/// This is the systems-1 counterpart of `auto_notify_from_events`. Together
/// the two cover the full notification surface area while keeping the
/// light-speed contract intact for remote observations.
pub fn notify_from_knowledge_facts(
    clock: Res<GameClock>,
    mut queue: ResMut<PendingFactQueue>,
    mut notifications: ResMut<NotificationQueue>,
    mut notified: ResMut<NotifiedEventIds>,
    mut speed: ResMut<GameSpeed>,
) {
    let ready = queue.drain_ready(clock.elapsed);
    for PerceivedFact { fact, .. } in ready {
        // #249: Dedupe by EventId. If a banner already fired for this id
        // (either from `auto_notify_from_events` or a sibling fact with the
        // same id), drop this push silently. The fact is still drained — we
        // just don't surface a second banner.
        if let Some(eid) = fact.event_id() {
            if !notified.mark(eid) {
                continue;
            }
        }
        let priority = fact.priority();
        let title = fact.title().to_string();
        let description = fact.description();
        let related = fact.related_system();
        let id = notifications.push(title, description, None, priority, related);
        if id.is_some() && priority.pauses_game() {
            speed.pause();
        }
    }
}

/// Drain `_pending_notifications` from Lua and push each entry into the
/// `NotificationQueue`. High-priority entries also auto-pause the game (so
/// scripted critical alerts behave like the engine ones).
pub fn drain_pending_notifications(
    engine: Res<ScriptEngine>,
    mut queue: ResMut<NotificationQueue>,
    mut speed: ResMut<GameSpeed>,
) {
    let lua = engine.lua();
    let Ok(table) = lua.globals().get::<mlua::Table>("_pending_notifications") else {
        return;
    };
    let Ok(len) = table.len() else {
        return;
    };
    if len == 0 {
        return;
    }

    for i in 1..=len {
        let Ok(entry) = table.get::<mlua::Table>(i) else {
            continue;
        };
        let title: String = entry.get("title").unwrap_or_default();
        let description: String = entry.get("description").unwrap_or_default();
        let icon: Option<String> = entry.get("icon").ok();
        let priority_str: String = entry
            .get("priority")
            .unwrap_or_else(|_| "medium".to_string());
        let priority = NotificationPriority::from_str(&priority_str);

        // target_system is encoded as the raw Entity bits (u64). Lua scripts
        // currently only have indirect handles, so this is best-effort: we
        // accept either a number (Entity::to_bits) or omit it.
        let target_system: Option<Entity> = entry
            .get::<u64>("target_system")
            .ok()
            .map(Entity::from_bits);

        let id = queue.push(title, description, icon, priority, target_system);
        if id.is_some() && priority.pauses_game() {
            speed.pause();
        }
    }

    // Clear the pending queue
    if let Ok(new_table) = lua.create_table() {
        let _ = lua.globals().set("_pending_notifications", new_table);
    }
}

/// Plugin wiring up the notification queue, the per-frame TTL ticker, and
/// the auto-mapping from `GameEvent` to banners.
pub struct NotificationsPlugin;

impl Plugin for NotificationsPlugin {
    fn build(&self, app: &mut App) {
        app.insert_resource(NotificationQueue::new())
            .init_resource::<PendingFactQueue>()
            .add_systems(Update, (tick_notifications, auto_notify_from_events))
            .add_systems(
                Update,
                (notify_from_knowledge_facts, drain_pending_notifications)
                    .after(crate::time_system::advance_game_time),
            );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn priority_from_str_defaults_to_medium() {
        assert_eq!(
            NotificationPriority::from_str("medium"),
            NotificationPriority::Medium
        );
        assert_eq!(
            NotificationPriority::from_str("low"),
            NotificationPriority::Low
        );
        assert_eq!(
            NotificationPriority::from_str("high"),
            NotificationPriority::High
        );
        // Unknown defaults to medium for forward-compat
        assert_eq!(
            NotificationPriority::from_str("garbage"),
            NotificationPriority::Medium
        );
    }

    #[test]
    fn priority_shows_banner_rules() {
        assert!(!NotificationPriority::Low.shows_banner());
        assert!(NotificationPriority::Medium.shows_banner());
        assert!(NotificationPriority::High.shows_banner());
    }

    #[test]
    fn priority_pauses_only_high() {
        assert!(!NotificationPriority::Low.pauses_game());
        assert!(!NotificationPriority::Medium.pauses_game());
        assert!(NotificationPriority::High.pauses_game());
    }

    #[test]
    fn push_low_priority_is_dropped() {
        let mut q = NotificationQueue::new();
        let id = q.push("a", "b", None, NotificationPriority::Low, None);
        assert!(id.is_none());
        assert!(q.items.is_empty());
    }

    #[test]
    fn push_medium_has_finite_ttl() {
        let mut q = NotificationQueue::new();
        let id = q
            .push("a", "b", None, NotificationPriority::Medium, None)
            .unwrap();
        assert_eq!(q.items.len(), 1);
        assert_eq!(q.items[0].id, id);
        assert!(q.items[0].remaining_seconds.is_some());
        assert!(q.items[0].remaining_seconds.unwrap() > 0.0);
    }

    #[test]
    fn push_high_is_sticky() {
        let mut q = NotificationQueue::new();
        q.push("a", "b", None, NotificationPriority::High, None);
        assert!(q.items[0].remaining_seconds.is_none());
    }

    #[test]
    fn newest_notification_is_at_front() {
        let mut q = NotificationQueue::new();
        q.push("first", "", None, NotificationPriority::Medium, None);
        q.push("second", "", None, NotificationPriority::Medium, None);
        q.push("third", "", None, NotificationPriority::Medium, None);
        assert_eq!(q.items[0].title, "third");
        assert_eq!(q.items[1].title, "second");
        assert_eq!(q.items[2].title, "first");
    }

    #[test]
    fn queue_evicts_oldest_when_full() {
        let mut q = NotificationQueue::new();
        q.max_items = 3;
        for i in 0..5 {
            q.push(
                format!("t{}", i),
                "",
                None,
                NotificationPriority::Medium,
                None,
            );
        }
        assert_eq!(q.items.len(), 3);
        // Most recent ("t4") at the front; oldest retained is "t2".
        assert_eq!(q.items[0].title, "t4");
        assert_eq!(q.items[2].title, "t2");
    }

    #[test]
    fn dismiss_removes_by_id() {
        let mut q = NotificationQueue::new();
        let id1 = q
            .push("a", "", None, NotificationPriority::Medium, None)
            .unwrap();
        let _id2 = q
            .push("b", "", None, NotificationPriority::High, None)
            .unwrap();
        assert!(q.dismiss(id1));
        assert_eq!(q.items.len(), 1);
        assert_eq!(q.items[0].title, "b");
        // Dismissing again returns false
        assert!(!q.dismiss(id1));
    }

    #[test]
    fn tick_decrements_medium_and_expires() {
        let mut q = NotificationQueue::new();
        q.push("a", "", None, NotificationPriority::Medium, None);
        let initial = q.items[0].remaining_seconds.unwrap();
        q.tick(1.0);
        let after = q.items[0].remaining_seconds.unwrap();
        assert!((initial - after - 1.0).abs() < 1e-4);
        // Force expiry
        q.tick(initial);
        assert!(q.items.is_empty());
    }

    #[test]
    fn tick_does_not_expire_high_priority() {
        let mut q = NotificationQueue::new();
        q.push("sticky", "", None, NotificationPriority::High, None);
        q.tick(1_000_000.0);
        assert_eq!(q.items.len(), 1, "high priority must not auto-expire");
    }

    #[test]
    fn priority_mapping_for_known_events() {
        assert_eq!(
            priority_for_event_kind(&GameEventKind::SurveyComplete),
            Some(NotificationPriority::Medium)
        );
        assert_eq!(
            priority_for_event_kind(&GameEventKind::SurveyDiscovery),
            Some(NotificationPriority::High)
        );
        assert_eq!(
            priority_for_event_kind(&GameEventKind::ColonyEstablished),
            Some(NotificationPriority::High)
        );
        assert_eq!(
            priority_for_event_kind(&GameEventKind::CombatVictory),
            Some(NotificationPriority::High)
        );
        assert_eq!(
            priority_for_event_kind(&GameEventKind::CombatDefeat),
            Some(NotificationPriority::High)
        );
        assert_eq!(
            priority_for_event_kind(&GameEventKind::HostileDetected),
            Some(NotificationPriority::High)
        );
        assert_eq!(
            priority_for_event_kind(&GameEventKind::PlayerRespawn),
            Some(NotificationPriority::High)
        );
        assert_eq!(
            priority_for_event_kind(&GameEventKind::ResourceAlert),
            Some(NotificationPriority::Medium)
        );
    }

    #[test]
    fn priority_mapping_for_routine_events_is_none() {
        assert_eq!(priority_for_event_kind(&GameEventKind::ShipArrived), None);
        assert_eq!(priority_for_event_kind(&GameEventKind::ShipBuilt), None);
        assert_eq!(
            priority_for_event_kind(&GameEventKind::BuildingDemolished),
            None
        );
        assert_eq!(priority_for_event_kind(&GameEventKind::ShipScrapped), None);
    }

    /// Integration: GameEvents flow into the NotificationQueue through the
    /// auto_notify_from_events system without affecting the event log
    /// pipeline.
    ///
    /// #233: Only the whitelisted kinds (PlayerRespawn, ResourceAlert) still
    /// surface through the legacy `GameEvent → notification` mapping; other
    /// world events are routed through `PendingFactQueue`.
    #[test]
    fn auto_notify_pushes_for_whitelisted_events() {
        let mut app = App::new();
        app.add_message::<GameEvent>();
        app.insert_resource(NotificationQueue::new());
        app.init_resource::<NotifiedEventIds>();
        app.add_systems(Update, auto_notify_from_events);

        // PlayerRespawn → whitelisted; still banners.
        app.world_mut().write_message(GameEvent {
            id: EventId::default(),
            timestamp: 0,
            kind: GameEventKind::PlayerRespawn,
            description: "Flagship destroyed".into(),
            related_system: None,
        });
        // #233: SurveyDiscovery is no longer whitelisted — it must be routed
        // through the fact queue, so the notification queue should stay empty
        // for this path.
        app.world_mut().write_message(GameEvent {
            id: EventId::default(),
            timestamp: 0,
            kind: GameEventKind::SurveyDiscovery,
            description: "Found ruins".into(),
            related_system: None,
        });
        // Routine event — must NOT show banner
        app.world_mut().write_message(GameEvent {
            id: EventId::default(),
            timestamp: 0,
            kind: GameEventKind::ShipBuilt,
            description: "Built corvette".into(),
            related_system: None,
        });

        app.update();

        let queue = app.world().resource::<NotificationQueue>();
        assert_eq!(queue.items.len(), 1);
        assert_eq!(queue.items[0].title, "Player Respawn");
        assert_eq!(queue.items[0].priority, NotificationPriority::High);
    }

    #[test]
    fn legacy_whitelist_covers_player_respawn_and_resource_alert() {
        assert!(is_legacy_whitelisted(&GameEventKind::PlayerRespawn));
        assert!(is_legacy_whitelisted(&GameEventKind::ResourceAlert));
    }

    #[test]
    fn legacy_whitelist_excludes_world_events() {
        assert!(!is_legacy_whitelisted(&GameEventKind::SurveyComplete));
        assert!(!is_legacy_whitelisted(&GameEventKind::SurveyDiscovery));
        assert!(!is_legacy_whitelisted(&GameEventKind::ColonyEstablished));
        assert!(!is_legacy_whitelisted(&GameEventKind::CombatVictory));
        assert!(!is_legacy_whitelisted(&GameEventKind::CombatDefeat));
        assert!(!is_legacy_whitelisted(&GameEventKind::HostileDetected));
        assert!(!is_legacy_whitelisted(&GameEventKind::AnomalyDiscovered));
        assert!(!is_legacy_whitelisted(&GameEventKind::ColonyFailed));
    }
}
