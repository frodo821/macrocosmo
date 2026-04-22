//! ESC Notifications tab + queue (#344 ESC-1 stub → #345 ESC-2 real).
//!
//! This module hosts:
//!
//! 1. [`EscNotificationQueue`] — the ack-able history queue backing the
//!    Notifications tab. Each top-level entry carries a tree of
//!    [`Notification`] children; `ack` cascades parent → children. Pushes
//!    can optionally carry a `KnowledgeFact` [`EventId`] so the same
//!    `#249 NotifiedEventIds` tri-state map the banner queue uses also
//!    dedupes ESC entries. (#345 commit 1)
//! 2. [`NotificationsTab`] — the [`SituationTab`] that renders the queue.
//!    Custom render path (not `OngoingTab`) because each row needs an
//!    ack button + a severity / ack-state filter UI. (#345 commit 2)
//!
//! The queue is deliberately **separate** from the pre-existing banner
//! `crate::notifications::NotificationQueue` — the banner queue drives
//! TTL-based pop-overs with Low/Medium/High priority and pause semantics,
//! while ESC notifications are post-hoc, ack-gated, and tree-structured.
//! Merging the two is out of scope for the ESC epic; see
//! `docs/plan-326-esc.md` §"Existing NotificationQueue coexistence".
//!
//! # Push path
//!
//! Production pushes arrive from Lua via
//! `scripts/notifications/default_bridge.lua`
//! (an `on("*@observed", fn)` wildcard subscriber) calling the
//! `push_notification { event_id, severity, message, source }` Lua API.
//! The Rust-side drain system (`drain_pending_esc_notifications` in
//! `crate::scripting::esc_notifications`) parses those entries and
//! calls [`EscNotificationQueue::push`].
//!
//! [`EscNotificationQueue::push`] is also reachable from Rust for tests
//! and future direct-write consumers; it performs id allocation + the
//! `NotifiedEventIds` dedup handshake in a single call.
//!
//! # Ack routing (render → system)
//!
//! `SituationTab::render` only receives `&World`, so the tab cannot
//! mutate the queue during render. The render path pushes ack requests
//! into a process-wide `Mutex<Vec<PendingAck>>` buffer; the
//! [`apply_pending_acks_system`] Bevy system drains the buffer in
//! `Update` and applies each ack to the queue. Tests can drive this
//! directly via [`enqueue_pending_ack`] and
//! [`drain_pending_acks_for_tests`].

use std::any::Any;
use std::sync::Mutex;

use bevy::prelude::*;
use bevy_egui::egui;

use super::state::TabState;
use super::tab::{SituationTab, TabBadge, TabMeta};
use super::types::{Notification, NotificationId, Severity, severity_max};
use crate::knowledge::{EventId, NotifiedEventIds};

/// Outcome of [`EscNotificationQueue::push`] — a new entry was appended,
/// or the push was suppressed because the event_id already fired.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PushOutcome {
    /// The notification was appended with the returned id.
    Pushed(NotificationId),
    /// The notification was silently dropped because its `event_id` had
    /// already fired (`#249 NotifiedEventIds::try_notify` returned
    /// `false`).
    DedupedByEventId,
}

/// ESC ack-able notification history queue.
///
/// Top-level entries are stored **newest-first** (index 0 is the most
/// recently pushed). Children nest via [`Notification::children`].
///
/// ## Id allocation
///
/// Every successful push assigns a fresh [`NotificationId`] from a
/// monotonically increasing counter (starts at 1). Ids are never reused
/// even after an ack — so the tab strip badge, filter selection, and
/// any future persistence layer can safely use them as stable keys.
///
/// ## Dedup
///
/// Pushes that carry an `event_id` are routed through
/// [`NotifiedEventIds::try_notify`] before allocation. The first push
/// for a given id wins; subsequent pushes return
/// [`PushOutcome::DedupedByEventId`] with no mutation. Pushes without an
/// `event_id` always succeed.
///
/// ## Ack
///
/// [`EscNotificationQueue::ack`] cascades to children via
/// [`Notification::ack_cascade`] so the tree-level "ack all" semantics
/// stay consistent with the ESC-1 `ack_cascade` unit tests. Ack only
/// touches the entry and its descendants — siblings are untouched.
#[derive(Resource, Debug, Default)]
pub struct EscNotificationQueue {
    /// Newest-first stack of top-level notifications.
    pub items: Vec<Notification>,
    /// Monotonic id counter for new pushes. Never decreases.
    next_id: NotificationId,
}

impl EscNotificationQueue {
    /// Append a new notification. Optionally claim an `event_id` for
    /// dedup via [`NotifiedEventIds`].
    ///
    /// Returns [`PushOutcome::Pushed`] with the allocated id on success.
    /// Returns [`PushOutcome::DedupedByEventId`] when `event_id` is
    /// `Some(id)` and the id has already been claimed (either by an
    /// earlier ESC push or by the banner `auto_notify_from_events` path,
    /// since the map is shared).
    ///
    /// The caller is responsible for building the [`Notification`] with
    /// its `source`, `timestamp`, `severity`, `message`, and any
    /// pre-built `children`. The queue overwrites `id` to guarantee
    /// monotonicity — callers should not rely on whatever value they
    /// pass in.
    pub fn push(
        &mut self,
        mut notification: Notification,
        event_id: Option<EventId>,
        notified_ids: Option<&mut NotifiedEventIds>,
    ) -> PushOutcome {
        if let (Some(eid), Some(notified)) = (event_id, notified_ids)
            && !notified.try_notify(eid)
        {
            return PushOutcome::DedupedByEventId;
        }
        self.next_id = self.next_id.saturating_add(1);
        let id = self.next_id;
        notification.id = id;
        // Newest-first: prepend so the tab renders the most recent
        // entry at the top without the renderer having to reverse-walk.
        self.items.insert(0, notification);
        PushOutcome::Pushed(id)
    }

    /// Ack the notification with `id` and cascade to all descendants.
    /// Returns `true` if a matching entry was found (even if it was
    /// already acked). Returns `false` if no entry with that id exists.
    pub fn ack(&mut self, id: NotificationId) -> bool {
        for top in self.items.iter_mut() {
            if let Some(found) = find_mut_in_subtree(top, id) {
                found.ack_cascade();
                return true;
            }
        }
        false
    }

    /// Cascade-ack every top-level entry in the queue. Returns the
    /// number of top-level entries that were touched (equal to
    /// `items.len()`).
    pub fn ack_all(&mut self) -> usize {
        let n = self.items.len();
        for top in self.items.iter_mut() {
            top.ack_cascade();
        }
        n
    }

    /// Total count of unacked entries across every tree in the queue.
    pub fn total_unacked(&self) -> usize {
        self.items.iter().map(Notification::unacked_count).sum()
    }

    /// Highest severity across all unacked entries, or `None` if
    /// nothing is pending.
    pub fn highest_unacked_severity(&self) -> Option<Severity> {
        self.items
            .iter()
            .filter_map(Notification::highest_unacked_severity)
            .reduce(severity_max)
    }

    /// Most recently allocated id (zero before the first push). Mostly
    /// useful for tests / diagnostics.
    pub fn last_id(&self) -> NotificationId {
        self.next_id
    }
}

/// Recursive helper that finds a mutable reference to the notification
/// with `id` inside a notification tree.
fn find_mut_in_subtree(root: &mut Notification, id: NotificationId) -> Option<&mut Notification> {
    if root.id == id {
        return Some(root);
    }
    for child in root.children.iter_mut() {
        if let Some(found) = find_mut_in_subtree(child, id) {
            return Some(found);
        }
    }
    None
}

/// Ack request emitted by the tab renderer and drained by
/// [`apply_pending_acks_system`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PendingAck {
    Single(NotificationId),
    All,
}

// Shared render-path buffer. `SituationTab::render` only sees `&World`,
// so we cannot route ack requests through a `ResMut` there. The render
// path pushes into this process-wide mutex-protected buffer, and
// `apply_pending_acks_system` drains it in `Update`. A `Mutex` (rather
// than a thread-local) is required because the egui draw schedule and
// the Bevy runner systems may run on different worker threads — a TLS
// would be invisible to the drain system.
//
// Producer: `render_notification_leaf` / the "ack all" button branch
// / the public [`enqueue_pending_ack`] helper (used by tests).
// Consumer: [`apply_pending_acks_system`] (Bevy `Update`).
static PENDING_ACK_BUFFER: Mutex<Vec<PendingAck>> = Mutex::new(Vec::new());

/// Test helper + alternate entry: append an ack intent directly. The
/// tab renderer calls this too; exposed so tests can drive the drain
/// system without wiring egui.
pub fn enqueue_pending_ack(action: PendingAck) {
    match PENDING_ACK_BUFFER.lock() {
        Ok(mut buf) => buf.push(action),
        Err(poison) => poison.into_inner().push(action),
    }
}

/// Drain every ack intent emitted by the tab renderer since the last
/// call. Public for tests only.
pub fn drain_pending_acks_for_tests() -> Vec<PendingAck> {
    match PENDING_ACK_BUFFER.lock() {
        Ok(mut buf) => std::mem::take(&mut *buf),
        Err(poison) => std::mem::take(&mut *poison.into_inner()),
    }
}

/// Bevy system that drains the render-path ack intents and applies them
/// to the queue. Runs in `Update` after the UI pass so the queue sees
/// each ack exactly once per frame.
pub fn apply_pending_acks_system(mut queue: ResMut<EscNotificationQueue>) {
    let drained: Vec<PendingAck> = match PENDING_ACK_BUFFER.lock() {
        Ok(mut buf) => std::mem::take(&mut *buf),
        Err(poison) => std::mem::take(&mut *poison.into_inner()),
    };
    for action in drained {
        match action {
            PendingAck::Single(id) => {
                queue.ack(id);
            }
            PendingAck::All => {
                queue.ack_all();
            }
        }
    }
}

/// ESC tab that surfaces the [`EscNotificationQueue`].
///
/// Implements [`SituationTab`] directly rather than [`super::tab::OngoingTab`]
/// because its render path needs per-row ack buttons + a filter UI — the
/// Event-tree default renderer doesn't fit. Filter state (severity floor,
/// show-acked flag) lives on [`TabState`] so it persists across tab
/// switches.
pub struct NotificationsTab;

impl NotificationsTab {
    pub const ID: &'static str = "notifications";
}

impl SituationTab for NotificationsTab {
    fn meta(&self) -> TabMeta {
        TabMeta {
            id: Self::ID,
            display_name: "Notifications",
            // Notifications sit on the right edge of the tab strip by
            // convention — ongoing tabs (Construction / Ship Ops /
            // Diplomatic / Resource) occupy 100..400 in ESC-3.
            order: 900,
        }
    }

    fn badge(&self, world: &World) -> Option<TabBadge> {
        let queue = world.get_resource::<EscNotificationQueue>()?;
        let count = queue.total_unacked();
        if count == 0 {
            return None;
        }
        let severity = queue.highest_unacked_severity().unwrap_or(Severity::Info);
        Some(TabBadge::new(count as u32, severity))
    }

    fn render(&self, ui: &mut egui::Ui, world: &World, state: &mut TabState) {
        let Some(queue) = world.get_resource::<EscNotificationQueue>() else {
            ui.label(egui::RichText::new("(EscNotificationQueue resource missing)").weak());
            return;
        };

        render_filter_toolbar(ui, state);
        ui.separator();

        if queue.items.is_empty() {
            ui.label(egui::RichText::new("(no notifications)").weak());
            return;
        }

        ui.horizontal(|ui| {
            if ui.button("Ack all").clicked() {
                enqueue_pending_ack(PendingAck::All);
            }
            ui.label(egui::RichText::new(format!("{} unacked", queue.total_unacked())).weak());
        });
        ui.separator();

        egui::ScrollArea::vertical()
            .auto_shrink([false, true])
            .max_height(360.0)
            .show(ui, |ui| {
                for notif in &queue.items {
                    if !passes_filter(notif, state) {
                        continue;
                    }
                    render_notification_row(ui, notif);
                }
            });
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

/// Render the filter toolbar (severity floor + show-acked toggle). Both
/// edits land on the shared [`TabState`]; the queue itself is not
/// touched here.
fn render_filter_toolbar(ui: &mut egui::Ui, state: &mut TabState) {
    ui.horizontal(|ui| {
        ui.label("Filter:");
        egui::ComboBox::from_id_salt("esc_notif_severity_filter")
            .selected_text(match state.severity_floor {
                None => "All",
                Some(Severity::Info) => "Info+",
                Some(Severity::Warn) => "Warn+",
                Some(Severity::Critical) => "Critical only",
            })
            .show_ui(ui, |ui| {
                ui.selectable_value(&mut state.severity_floor, None, "All");
                ui.selectable_value(&mut state.severity_floor, Some(Severity::Info), "Info+");
                ui.selectable_value(&mut state.severity_floor, Some(Severity::Warn), "Warn+");
                ui.selectable_value(
                    &mut state.severity_floor,
                    Some(Severity::Critical),
                    "Critical only",
                );
            });

        // Show-acked toggle stored as a sentinel in `TabState::filter`
        // so the shared state struct stays shape-compatible with other
        // tabs (no dedicated `hide_acked` field).
        let mut hide_acked = state.filter == HIDE_ACKED_SENTINEL;
        if ui.checkbox(&mut hide_acked, "Hide acked").changed() {
            state.filter = if hide_acked {
                HIDE_ACKED_SENTINEL.into()
            } else {
                String::new()
            };
        }
    });
}

/// Internal marker written to `TabState::filter` when the player toggles
/// "Hide acked" in the notifications tab. Stored as a sentinel string so
/// the shared `TabState` struct stays shape-compatible with other tabs
/// (no dedicated `hide_acked` field).
const HIDE_ACKED_SENTINEL: &str = "__esc_hide_acked__";

/// Predicate that gates each top-level notification by the tab state.
/// Tree descendants inherit the decision of their root — individual
/// children are not re-filtered. This keeps the "parent ack cascades
/// children" UX coherent: the root is the unit of filtering.
fn passes_filter(notif: &Notification, state: &TabState) -> bool {
    if let Some(floor) = state.severity_floor {
        // Use the highest unacked severity when present so a fully-acked
        // high-severity tree doesn't dominate the filter. Fall through
        // to `notif.severity` when the subtree is fully acked so the
        // entry is still reachable in the default view.
        let sev = notif.highest_unacked_severity().unwrap_or(notif.severity);
        if severity_rank(sev) < severity_rank(floor) {
            return false;
        }
    }

    if state.filter == HIDE_ACKED_SENTINEL && notif.unacked_count() == 0 {
        return false;
    }

    true
}

fn severity_rank(s: Severity) -> u8 {
    match s {
        Severity::Info => 0,
        Severity::Warn => 1,
        Severity::Critical => 2,
    }
}

/// Render a single top-level notification row (possibly with children)
/// plus an ack button. Queues ack intents via [`enqueue_pending_ack`].
fn render_notification_row(ui: &mut egui::Ui, notif: &Notification) {
    if notif.children.is_empty() {
        render_notification_leaf(ui, notif);
    } else {
        let header = format!(
            "{} ({} unacked of {})",
            notif.message,
            notif.unacked_count(),
            notif.tree_len(),
        );
        egui::CollapsingHeader::new(header)
            .id_salt(("esc_notif", notif.id))
            .default_open(true)
            .show(ui, |ui| {
                render_notification_leaf(ui, notif);
                ui.indent(("esc_notif_children", notif.id), |ui| {
                    for child in &notif.children {
                        render_notification_row(ui, child);
                    }
                });
            });
    }
}

fn render_notification_leaf(ui: &mut egui::Ui, notif: &Notification) {
    ui.horizontal(|ui| {
        let tint = severity_tint(notif.severity);
        ui.label(
            egui::RichText::new(severity_label(notif.severity))
                .color(tint)
                .strong(),
        );
        let text = if notif.acked {
            egui::RichText::new(&notif.message).weak()
        } else {
            egui::RichText::new(&notif.message)
        };
        ui.label(text);
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if !notif.acked && ui.small_button("ack").clicked() {
                enqueue_pending_ack(PendingAck::Single(notif.id));
            }
            ui.label(
                egui::RichText::new(format!("t={}", notif.timestamp))
                    .weak()
                    .small(),
            );
        });
    });
}

fn severity_label(severity: Severity) -> &'static str {
    match severity {
        Severity::Info => "INFO",
        Severity::Warn => "WARN",
        Severity::Critical => "CRIT",
    }
}

/// Map [`Severity`] to an egui colour. Also used by the tab strip
/// badge renderer so colours stay consistent.
pub fn severity_tint(severity: Severity) -> egui::Color32 {
    match severity {
        Severity::Info => egui::Color32::from_rgb(150, 200, 230),
        Severity::Warn => egui::Color32::from_rgb(230, 200, 90),
        Severity::Critical => egui::Color32::from_rgb(220, 80, 80),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::situation_center::types::NotificationSource;

    fn notif(sev: Severity, acked: bool, message: &str) -> Notification {
        Notification {
            id: 0,
            source: NotificationSource::None,
            timestamp: 0,
            severity: sev,
            message: message.into(),
            acked,
            children: Vec::new(),
        }
    }

    #[test]
    fn push_assigns_monotonic_ids() {
        let mut q = EscNotificationQueue::default();
        let a = q.push(notif(Severity::Info, false, "a"), None, None);
        let b = q.push(notif(Severity::Info, false, "b"), None, None);
        let c = q.push(notif(Severity::Info, false, "c"), None, None);
        match (a, b, c) {
            (PushOutcome::Pushed(ia), PushOutcome::Pushed(ib), PushOutcome::Pushed(ic)) => {
                assert!(ia < ib && ib < ic);
            }
            _ => panic!("pushes should succeed: {a:?} {b:?} {c:?}"),
        }
        assert_eq!(q.items.len(), 3);
        // Newest-first invariant.
        assert_eq!(q.items[0].message, "c");
        assert_eq!(q.items[2].message, "a");
    }

    #[test]
    fn push_deduplicates_by_event_id() {
        let mut q = EscNotificationQueue::default();
        let mut notified = NotifiedEventIds::default();
        let eid = EventId(42);
        notified.register(eid);

        let first = q.push(
            notif(Severity::Warn, false, "first"),
            Some(eid),
            Some(&mut notified),
        );
        let second = q.push(
            notif(Severity::Warn, false, "second"),
            Some(eid),
            Some(&mut notified),
        );

        assert!(matches!(first, PushOutcome::Pushed(_)));
        assert_eq!(second, PushOutcome::DedupedByEventId);
        assert_eq!(q.items.len(), 1);
        assert_eq!(q.items[0].message, "first");
    }

    #[test]
    fn push_without_event_id_never_dedupes() {
        let mut q = EscNotificationQueue::default();
        let mut notified = NotifiedEventIds::default();
        for _ in 0..3 {
            let outcome = q.push(notif(Severity::Info, false, "x"), None, Some(&mut notified));
            assert!(matches!(outcome, PushOutcome::Pushed(_)));
        }
        assert_eq!(q.items.len(), 3);
    }

    #[test]
    fn ack_cascades_to_children() {
        let mut q = EscNotificationQueue::default();
        let mut root = notif(Severity::Warn, false, "root");
        root.children.push(notif(Severity::Info, false, "child1"));
        root.children
            .push(notif(Severity::Critical, false, "child2"));
        let id = match q.push(root, None, None) {
            PushOutcome::Pushed(id) => id,
            _ => panic!(),
        };

        assert_eq!(q.total_unacked(), 3);
        assert!(q.ack(id));
        assert_eq!(q.total_unacked(), 0);
        assert!(q.items[0].acked);
        assert!(q.items[0].children.iter().all(|c| c.acked));
    }

    #[test]
    fn ack_only_affects_matching_subtree() {
        let mut q = EscNotificationQueue::default();
        let mut a = notif(Severity::Warn, false, "a");
        a.children.push(notif(Severity::Info, false, "a1"));
        let id_a = match q.push(a, None, None) {
            PushOutcome::Pushed(id) => id,
            _ => panic!(),
        };
        let _id_b = q.push(notif(Severity::Critical, false, "b"), None, None);

        assert!(q.ack(id_a));
        assert_eq!(q.total_unacked(), 1);
        let b = q
            .items
            .iter()
            .find(|n| n.message == "b")
            .expect("b present");
        assert!(!b.acked);
    }

    #[test]
    fn ack_missing_id_returns_false() {
        let mut q = EscNotificationQueue::default();
        q.push(notif(Severity::Info, false, "a"), None, None);
        assert!(!q.ack(999_999));
    }

    #[test]
    fn ack_all_cascades_everything() {
        let mut q = EscNotificationQueue::default();
        q.push(notif(Severity::Info, false, "a"), None, None);
        q.push(notif(Severity::Warn, false, "b"), None, None);
        q.push(notif(Severity::Critical, false, "c"), None, None);
        let n = q.ack_all();
        assert_eq!(n, 3);
        assert_eq!(q.total_unacked(), 0);
    }

    #[test]
    fn ack_is_idempotent() {
        let mut q = EscNotificationQueue::default();
        let id = match q.push(notif(Severity::Warn, false, "a"), None, None) {
            PushOutcome::Pushed(id) => id,
            _ => panic!(),
        };
        assert!(q.ack(id));
        assert!(q.ack(id));
        assert_eq!(q.total_unacked(), 0);
    }

    #[test]
    fn empty_queue_emits_no_badge() {
        let mut world = World::new();
        world.insert_resource(EscNotificationQueue::default());
        let tab = NotificationsTab;
        assert!(tab.badge(&world).is_none());
    }

    #[test]
    fn badge_counts_unacked_entries_and_reports_highest_severity() {
        let mut world = World::new();
        let mut queue = EscNotificationQueue::default();
        queue.push(notif(Severity::Info, true, "acked info"), None, None);
        queue.push(notif(Severity::Warn, false, "warn"), None, None);
        queue.push(notif(Severity::Critical, false, "crit"), None, None);
        queue.push(notif(Severity::Critical, true, "acked crit"), None, None);
        world.insert_resource(queue);

        let tab = NotificationsTab;
        let badge = tab.badge(&world).expect("unacked entries produce a badge");
        assert_eq!(badge.count, 2);
        assert_eq!(badge.severity, Severity::Critical);
    }

    #[test]
    fn missing_resource_is_treated_as_empty() {
        let world = World::new();
        let tab = NotificationsTab;
        assert!(tab.badge(&world).is_none());
    }

    #[test]
    fn filter_severity_floor_hides_lower() {
        let mut state = TabState::default();
        state.severity_floor = Some(Severity::Warn);

        let info = notif(Severity::Info, false, "info");
        let warn = notif(Severity::Warn, false, "warn");
        let crit = notif(Severity::Critical, false, "crit");

        assert!(!passes_filter(&info, &state));
        assert!(passes_filter(&warn, &state));
        assert!(passes_filter(&crit, &state));
    }

    #[test]
    fn filter_hide_acked_drops_fully_acked_tree() {
        let mut state = TabState::default();
        state.filter = HIDE_ACKED_SENTINEL.into();

        let mut acked = notif(Severity::Warn, true, "acked");
        acked.children.push(notif(Severity::Info, true, "c"));
        let unacked = notif(Severity::Info, false, "unacked");

        assert!(!passes_filter(&acked, &state));
        assert!(passes_filter(&unacked, &state));
    }

    #[test]
    fn push_outcome_bumps_last_id() {
        let mut q = EscNotificationQueue::default();
        assert_eq!(q.last_id(), 0);
        q.push(notif(Severity::Info, false, "a"), None, None);
        assert_eq!(q.last_id(), 1);
        q.push(notif(Severity::Info, false, "b"), None, None);
        assert_eq!(q.last_id(), 2);
    }

    /// Unit-level test for [`apply_pending_acks_system`] that calls
    /// it directly rather than going through `App::update()`. Avoids
    /// flakiness from the Bevy worker pool interleaving with the
    /// global [`PENDING_ACK_BUFFER`] across concurrent tests in the
    /// same process. The end-to-end (App::update) path is covered by
    /// `tests/esc_notification_pipeline.rs` with explicit serialisation.
    #[test]
    fn apply_pending_acks_system_drains_buffer_and_acks_queue() {
        let _guard = acquire_test_ack_serial();
        // Clear leftover buffer state from any prior test run.
        let _ = drain_pending_acks_for_tests();

        let mut queue = EscNotificationQueue::default();
        let id1 = match queue.push(notif(Severity::Warn, false, "a"), None, None) {
            PushOutcome::Pushed(id) => id,
            _ => unreachable!(),
        };
        let _id2 = queue.push(notif(Severity::Critical, false, "b"), None, None);

        enqueue_pending_ack(PendingAck::Single(id1));

        // Exercise the system function directly via a short-lived World
        // so there's no worker-pool interleaving with other tests'
        // queue resources.
        let mut world = World::new();
        world.insert_resource(queue);
        let mut system = bevy::ecs::system::IntoSystem::into_system(apply_pending_acks_system);
        system.initialize(&mut world);
        system.run((), &mut world);

        let q = world.resource::<EscNotificationQueue>();
        let acked_ids: Vec<_> = q.items.iter().filter(|n| n.acked).map(|n| n.id).collect();
        assert_eq!(acked_ids, vec![id1]);

        enqueue_pending_ack(PendingAck::All);
        system.run((), &mut world);
        let q = world.resource::<EscNotificationQueue>();
        assert_eq!(q.total_unacked(), 0);
    }

    /// Global mutex used by tests that touch [`PENDING_ACK_BUFFER`] so
    /// they don't race each other. `cargo test` runs tests concurrently
    /// by default; wrapping each buffer-touching test in
    /// `acquire_test_ack_serial` keeps their effects isolated.
    fn acquire_test_ack_serial() -> std::sync::MutexGuard<'static, ()> {
        static TEST_ACK_SERIAL: std::sync::Mutex<()> = std::sync::Mutex::new(());
        TEST_ACK_SERIAL
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
    }
}
