//! Common shape types for the Empire Situation Center (#344 / ESC-1).
//!
//! All ESC tab entries are expressed as either an [`Event`] (ongoing
//! situation, derived each frame from ECS state) or a [`Notification`]
//! (past fact, queued and ack-able). Both carry a uniform tree shape —
//! `children` empty ⇒ leaf, non-empty ⇒ collapsible group — so tab
//! renderers never branch on enum variants.
//!
//! The `EventKind` enum is intentionally *closed* for v1 (see
//! `docs/plan-326-esc.md` §Lua boundary). The `Custom(String)` / open-variant
//! extension lands with the `define_situation_tab` Lua API in a later issue.
//!
//! Identifiers for build orders are carried as [`BuildOrderId`] which is
//! a `u64` alias matching the monotonic counter on
//! [`crate::colony::building_queue::BuildQueue`]. The alias is re-exported
//! here so downstream tab implementations can depend on `ui::situation_center`
//! alone.

use bevy::prelude::Entity;

/// Integer hexadies (matches `GameClock.elapsed`). Re-exported here so tab
/// implementations using `Event` / `Notification` don't need to import
/// `time_system`.
pub type GameTime = i64;

/// Stable id assigned at `BuildQueue::push_order` time. Matches the
/// underlying `u64` type used by
/// [`crate::colony::building_queue::BuildQueue::next_order_id`].
pub type BuildOrderId = u64;

/// Stable id for an ESC [`Event`]. Tab `collect` implementations choose
/// a stable-across-frames scheme (e.g. `Entity.to_bits()`, build order id,
/// hash of source) so the renderer can diff scroll state between frames.
pub type EventId = u64;

/// Stable id for an ESC [`Notification`]. Assigned by the notification
/// queue on push; #345 (ESC-2) will wire the real allocator.
pub type NotificationId = u64;

/// Severity of a [`Notification`] and colour key for tab badges.
///
/// This is ESC-local — it intentionally does **not** replace the existing
/// `crate::notifications::NotificationPriority` which drives the banner
/// stack (Low/Medium/High with TTL / pause semantics). ESC Notifications
/// are post-hoc, ack-gated, and have no TTL, so the shape differs.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Severity {
    #[default]
    Info,
    Warn,
    Critical,
}

/// Coarse kind of an [`Event`] (closed v1 enum, per #326 "設計確定事項").
///
/// `Other` is the catch-all for tabs that don't fit a specific category —
/// a full `Custom(String)` / trait-object extension is deferred to the
/// Lua tab API (see `docs/plan-326-esc.md`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum EventKind {
    Construction,
    Combat,
    Diplomatic,
    Survey,
    Travel,
    Resource,
    Other,
}

/// Origin entity for an [`Event`].
///
/// Same shape as [`NotificationSource`] — kept as a separate type so
/// individual tabs / bridges can attach tighter invariants later (e.g.
/// "only Colony / System is valid for Construction events") without
/// breaking Notification-side consumers.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default)]
pub enum EventSource {
    #[default]
    None,
    Empire(Entity),
    System(Entity),
    Colony(Entity),
    Ship(Entity),
    Fleet(Entity),
    Faction(Entity),
    BuildOrder(BuildOrderId),
}

/// Origin entity for a [`Notification`]. See [`EventSource`] for rationale
/// behind the twin types.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default)]
pub enum NotificationSource {
    #[default]
    None,
    Empire(Entity),
    System(Entity),
    Colony(Entity),
    Ship(Entity),
    Fleet(Entity),
    Faction(Entity),
    BuildOrder(BuildOrderId),
}

/// An ongoing situation surfaced by an [`crate::ui::situation_center::OngoingTab`].
///
/// The struct is deliberately uniform — a leaf entry (no children) and a
/// group entry (aggregate with children) share the same shape so the
/// default renderer walks a single tree recursion. Tabs that need
/// richer presentation can still override `SituationTab::render`.
#[derive(Clone, Debug)]
pub struct Event {
    pub id: EventId,
    pub source: EventSource,
    pub started_at: GameTime,
    pub kind: EventKind,
    pub label: String,
    /// Optional 0.0..=1.0 progress fraction. `None` ⇒ indeterminate.
    pub progress: Option<f32>,
    /// Optional ETA (absolute hexadies). `None` ⇒ unknown / open-ended.
    pub eta: Option<GameTime>,
    pub children: Vec<Event>,
}

impl Event {
    /// Recursively count this event + every descendant.
    pub fn tree_len(&self) -> usize {
        1 + self.children.iter().map(Event::tree_len).sum::<usize>()
    }

    /// Depth of the longest descendant chain (leaf = 0).
    pub fn max_depth(&self) -> usize {
        self.children
            .iter()
            .map(|c| c.max_depth() + 1)
            .max()
            .unwrap_or(0)
    }
}

/// A past fact that the player may want to acknowledge.
///
/// Wiring (queue + push API + light-speed bridge) is #345 scope. ESC-1
/// only defines the shape so the `NotificationsTab` placeholder compiles
/// and tabs can reason about the final structure.
#[derive(Clone, Debug)]
pub struct Notification {
    pub id: NotificationId,
    pub source: NotificationSource,
    pub timestamp: GameTime,
    pub severity: Severity,
    pub message: String,
    pub acked: bool,
    pub children: Vec<Notification>,
}

impl Notification {
    /// Recursively count this notification + every descendant.
    pub fn tree_len(&self) -> usize {
        1 + self
            .children
            .iter()
            .map(Notification::tree_len)
            .sum::<usize>()
    }

    /// Cascade-ack: mark this entry *and* every descendant as `acked`.
    /// Mirrors the ESC-2 "parent ack → children ack" rule.
    pub fn ack_cascade(&mut self) {
        self.acked = true;
        for c in &mut self.children {
            c.ack_cascade();
        }
    }

    /// Count of unacked entries in this subtree (self + descendants).
    pub fn unacked_count(&self) -> usize {
        let self_count = if self.acked { 0 } else { 1 };
        self_count
            + self
                .children
                .iter()
                .map(Notification::unacked_count)
                .sum::<usize>()
    }

    /// Highest severity across self + unacked descendants. Returns
    /// `None` if every entry in the subtree is already acked.
    pub fn highest_unacked_severity(&self) -> Option<Severity> {
        let mut best: Option<Severity> = if self.acked {
            None
        } else {
            Some(self.severity)
        };
        for c in &self.children {
            if let Some(child_sev) = c.highest_unacked_severity() {
                best = Some(match best {
                    None => child_sev,
                    Some(cur) => severity_max(cur, child_sev),
                });
            }
        }
        best
    }
}

/// Order `Severity` so `Critical > Warn > Info`. Used by badge colour
/// roll-up in the notifications tab.
pub fn severity_max(a: Severity, b: Severity) -> Severity {
    fn rank(s: Severity) -> u8 {
        match s {
            Severity::Info => 0,
            Severity::Warn => 1,
            Severity::Critical => 2,
        }
    }
    if rank(a) >= rank(b) { a } else { b }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn leaf(id: EventId, label: &str) -> Event {
        Event {
            id,
            source: EventSource::None,
            started_at: 0,
            kind: EventKind::Other,
            label: label.into(),
            progress: None,
            eta: None,
            children: Vec::new(),
        }
    }

    fn notif(id: NotificationId, sev: Severity, acked: bool) -> Notification {
        Notification {
            id,
            source: NotificationSource::None,
            timestamp: 0,
            severity: sev,
            message: format!("n{}", id),
            acked,
            children: Vec::new(),
        }
    }

    #[test]
    fn event_tree_len_and_depth_traverse_all() {
        // root
        //   a
        //     a1
        //   b
        let mut root = leaf(0, "root");
        let mut a = leaf(1, "a");
        a.children.push(leaf(2, "a1"));
        root.children.push(a);
        root.children.push(leaf(3, "b"));

        assert_eq!(root.tree_len(), 4);
        assert_eq!(root.max_depth(), 2);
        // Pure leaf reports depth 0.
        assert_eq!(leaf(9, "x").max_depth(), 0);
    }

    #[test]
    fn notification_ack_cascade_marks_every_descendant() {
        let mut n = notif(1, Severity::Warn, false);
        let mut child = notif(2, Severity::Critical, false);
        child.children.push(notif(3, Severity::Info, false));
        n.children.push(child);
        n.children.push(notif(4, Severity::Info, false));

        assert_eq!(n.unacked_count(), 4);
        n.ack_cascade();
        assert_eq!(n.unacked_count(), 0);
        assert!(n.children.iter().all(|c| c.acked));
    }

    #[test]
    fn highest_unacked_severity_ignores_acked_entries() {
        // root (Info, acked)
        //   child (Critical, acked) — masked
        //   sibling (Warn, unacked)
        let mut root = notif(1, Severity::Info, true);
        root.children.push(notif(2, Severity::Critical, true));
        root.children.push(notif(3, Severity::Warn, false));

        assert_eq!(root.highest_unacked_severity(), Some(Severity::Warn));

        // Fully acked tree reports None.
        let mut all_acked = notif(10, Severity::Critical, true);
        all_acked.children.push(notif(11, Severity::Critical, true));
        assert_eq!(all_acked.highest_unacked_severity(), None);
    }

    #[test]
    fn severity_max_orders_critical_greater_than_warn_greater_than_info() {
        assert_eq!(severity_max(Severity::Info, Severity::Warn), Severity::Warn);
        assert_eq!(
            severity_max(Severity::Warn, Severity::Critical),
            Severity::Critical
        );
        assert_eq!(severity_max(Severity::Info, Severity::Info), Severity::Info);
    }
}
