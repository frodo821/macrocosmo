//! ESC-internal Notifications tab + queue stub (#344 / ESC-1).
//!
//! The *queue* and its push / ack / dedupe / light-speed bridging is
//! #345 (ESC-2) scope. ESC-1 only lands:
//!
//! 1. The [`EscNotificationQueue`] resource as an empty stub so the
//!    framework compiles and other systems can depend on it.
//! 2. The [`NotificationsTab`] concrete type, implementing
//!    [`SituationTab`] directly (not `OngoingTab`) because its render
//!    path needs to surface an "ack" button per row — the Event-tree
//!    default renderer doesn't fit.
//! 3. A minimal tree renderer that walks `Vec<Notification>` so the
//!    tab shows *something* when entries are present.
//!
//! The queue struct is deliberately **separate** from the pre-existing
//! banner `NotificationQueue` in `crate::notifications` — the banner
//! queue drives TTL-based pop-overs with Low/Medium/High priority and
//! pause semantics, while ESC notifications are post-hoc, ack-gated,
//! and tree-structured. Merging the two is out of scope for ESC-1; see
//! `docs/plan-326-esc.md` §"Existing NotificationQueue coexistence".

use std::any::Any;

use bevy::prelude::*;
use bevy_egui::egui;

use super::state::TabState;
use super::tab::{SituationTab, TabBadge, TabMeta};
use super::types::{Notification, Severity, severity_max};

/// Stub queue for ESC notifications. ESC-2 replaces this with the real
/// push / ack / cascade-ack implementation; the minimal shape here is
/// just what the tab renderer needs (iterate + cascade-ack by id).
#[derive(Resource, Default, Debug)]
pub struct EscNotificationQueue {
    /// Newest first. ESC-2 will seal the push API and add dedupe
    /// against `NotifiedEventIds`.
    pub items: Vec<Notification>,
}

impl EscNotificationQueue {
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
}

/// ESC tab that surfaces the [`EscNotificationQueue`].
///
/// Implements [`SituationTab`] directly rather than [`super::tab::OngoingTab`]
/// because its render path needs per-row ack buttons (and eventually
/// filter UI in ESC-2) — the Event-tree default renderer doesn't fit.
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

    fn render(&self, ui: &mut egui::Ui, world: &World, _state: &mut TabState) {
        let Some(queue) = world.get_resource::<EscNotificationQueue>() else {
            ui.label(egui::RichText::new("(EscNotificationQueue resource missing)").weak());
            return;
        };

        if queue.items.is_empty() {
            ui.label(egui::RichText::new("(no notifications)").weak());
            return;
        }

        for notif in &queue.items {
            render_notification(ui, notif);
        }
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

fn render_notification(ui: &mut egui::Ui, notif: &Notification) {
    if notif.children.is_empty() {
        render_notification_leaf(ui, notif);
    } else {
        let header = format!("{} ({} unacked)", notif.message, notif.unacked_count(),);
        egui::CollapsingHeader::new(header)
            .id_salt(("esc_notif", notif.id))
            .default_open(true)
            .show(ui, |ui| {
                for c in &notif.children {
                    render_notification(ui, c);
                }
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

    fn notif(id: u64, sev: Severity, acked: bool) -> Notification {
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
        queue.items.push(notif(1, Severity::Info, true));
        queue.items.push(notif(2, Severity::Warn, false));
        queue.items.push(notif(3, Severity::Critical, false));
        // Extra acked Critical must not bump the severity.
        queue.items.push(notif(4, Severity::Critical, true));
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
}
