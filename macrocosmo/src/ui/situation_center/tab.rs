//! Tab abstraction for the Empire Situation Center (#344 / ESC-1).
//!
//! A tab is any type that implements [`SituationTab`]. A tab whose body
//! is derived every frame from ECS state (Construction / Ship Ops /
//! Diplomatic Standing / Resource Trends in #346) implements
//! [`OngoingTab`] instead; the default [`SituationTab::render`] provided
//! via the blanket impl walks the `Vec<Event>` tree with
//! [`render_event_tree`].
//!
//! The traits are the stable public surface consumers rely on. Design
//! stability under the future `define_situation_tab` Lua API is covered
//! in `docs/plan-326-esc.md` §Lua boundary.

use std::any::Any;

use bevy::prelude::World;
use bevy_egui::egui;

use super::state::TabState;
use super::types::{Event, Severity};

/// Stable identifier for a registered tab. Uses `&'static str` so
/// registrations can live in a `const` without heap churn.
pub type TabId = &'static str;

/// Human-facing metadata for a tab. Returned once at registration time
/// and stored on the registry; renderers do not query `meta` every frame.
#[derive(Clone, Debug)]
pub struct TabMeta {
    /// Stable id for state lookup / routing.
    pub id: TabId,
    /// Label rendered on the tab strip.
    pub display_name: &'static str,
    /// Sort key — lower values render further left. Ties break by
    /// registration order. Recommended range: 0..1000 for core tabs.
    pub order: i32,
}

/// Per-frame badge for a tab. Surfaces a count + tint on the tab strip.
#[derive(Clone, Copy, Debug)]
pub struct TabBadge {
    /// Count displayed inside the badge circle. `0` suppresses the badge.
    pub count: u32,
    /// Colour / priority hint. Mapped to an egui colour by the renderer.
    pub severity: Severity,
}

impl TabBadge {
    pub fn new(count: u32, severity: Severity) -> Self {
        Self { count, severity }
    }
}

/// Anything rendered as a tab inside the Empire Situation Center.
///
/// Concrete tab types should be stateless — per-tab UI scratch state
/// (filter / scroll position) lives on the [`crate::ui::situation_center::SituationCenterState`]
/// keyed by `meta().id` so it survives tab switches and save/load.
pub trait SituationTab: Send + Sync + 'static {
    /// Stable metadata. Called once at `register_situation_tab` time.
    fn meta(&self) -> TabMeta;

    /// Per-frame badge. Returning `None` hides the badge.
    fn badge(&self, world: &World) -> Option<TabBadge>;

    /// Render the tab body. Default impl for [`OngoingTab`] walks the
    /// Event tree via [`render_event_tree`]; bespoke tabs (e.g.
    /// Notifications) override this directly.
    fn render(&self, ui: &mut egui::Ui, world: &World, state: &mut TabState);

    /// Escape hatch for downcasting from a `Box<dyn SituationTab>`.
    /// Used by tests and the Lua adapter placeholder.
    fn as_any(&self) -> &dyn Any;
}

/// Tab whose body is a `Vec<Event>` derived from ECS state each frame.
///
/// Implementors only need to provide `collect` — the framework supplies
/// a default `SituationTab::render` via the [`DefaultOngoingRender`]
/// blanket adapter (see the [`register_ongoing_tab_defaults`]-style
/// helpers in `registry.rs`).
pub trait OngoingTab: Send + Sync + 'static {
    /// Stable metadata. Same contract as [`SituationTab::meta`].
    fn meta(&self) -> TabMeta;

    /// Collect the Event tree for this frame. The returned `Vec` may
    /// have leaves, groups (children non-empty), or be empty.
    fn collect(&self, world: &World) -> Vec<Event>;

    /// Per-frame badge. Default roll-up counts top-level events; tabs
    /// that want severity-aware roll-ups should override.
    fn badge(&self, world: &World) -> Option<TabBadge> {
        let events = self.collect(world);
        if events.is_empty() {
            None
        } else {
            Some(TabBadge::new(events.len() as u32, Severity::Info))
        }
    }
}

/// Blanket adapter: every `OngoingTab` is automatically a `SituationTab`
/// via an `OngoingTabAdapter` wrapper constructed at registration time.
///
/// The wrapper is needed (rather than a direct blanket impl) because
/// Rust's coherence rules forbid "every `T: OngoingTab` is a
/// `SituationTab`" without risking collisions with a future concrete
/// `SituationTab` impl on the same type. Wrapping also cleanly isolates
/// the "collect + default render" adapter from the notifications-tab
/// path, which needs a custom `render`.
pub struct OngoingTabAdapter<T: OngoingTab>(pub T);

impl<T: OngoingTab> SituationTab for OngoingTabAdapter<T> {
    fn meta(&self) -> TabMeta {
        self.0.meta()
    }

    fn badge(&self, world: &World) -> Option<TabBadge> {
        self.0.badge(world)
    }

    fn render(&self, ui: &mut egui::Ui, world: &World, _state: &mut TabState) {
        let events = self.0.collect(world);
        render_event_tree(ui, &events);
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

/// Default renderer for a slice of [`Event`] trees.
///
/// * A leaf (`children` empty) is rendered as a single line with label +
///   optional progress bar + optional ETA.
/// * A group (`children` non-empty) is wrapped in a `CollapsingHeader`
///   and recursed into.
pub fn render_event_tree(ui: &mut egui::Ui, events: &[Event]) {
    if events.is_empty() {
        ui.label(egui::RichText::new("(nothing ongoing)").weak());
        return;
    }
    for event in events {
        render_event_entry(ui, event);
    }
}

fn render_event_entry(ui: &mut egui::Ui, event: &Event) {
    if event.children.is_empty() {
        render_event_leaf(ui, event);
    } else {
        let header = build_event_header(event);
        egui::CollapsingHeader::new(header)
            .id_salt(("esc_event", event.id))
            .default_open(true)
            .show(ui, |ui| {
                for child in &event.children {
                    render_event_entry(ui, child);
                }
            });
    }
}

fn render_event_leaf(ui: &mut egui::Ui, event: &Event) {
    ui.horizontal(|ui| {
        ui.label(&event.label);
        if let Some(progress) = event.progress {
            let clamped = progress.clamp(0.0, 1.0);
            ui.add(
                egui::ProgressBar::new(clamped)
                    .desired_width(120.0)
                    .show_percentage(),
            );
        }
        if let Some(eta) = event.eta {
            ui.label(egui::RichText::new(format!("ETA {}", eta)).weak().small());
        }
    });
}

fn build_event_header(event: &Event) -> String {
    let child_count = event.children.len();
    if child_count == 0 {
        event.label.clone()
    } else {
        format!("{} ({})", event.label, child_count)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::situation_center::state::TabState;
    use crate::ui::situation_center::types::{Event as EscEvent, EventKind, EventSource};
    use bevy::prelude::*;

    /// A minimal `OngoingTab` used only to exercise framework plumbing —
    /// real tabs land in #346.
    struct DummyOngoingTab;

    impl OngoingTab for DummyOngoingTab {
        fn meta(&self) -> TabMeta {
            TabMeta {
                id: "dummy",
                display_name: "Dummy",
                order: 0,
            }
        }

        fn collect(&self, _world: &World) -> Vec<EscEvent> {
            vec![EscEvent {
                id: 42,
                source: EventSource::None,
                started_at: 0,
                kind: EventKind::Other,
                label: "leaf".into(),
                progress: Some(0.25),
                eta: Some(10),
                children: vec![],
            }]
        }
    }

    #[test]
    fn ongoing_tab_adapter_delegates_to_inner() {
        let mut world = World::new();
        let adapter = OngoingTabAdapter(DummyOngoingTab);

        assert_eq!(adapter.meta().id, "dummy");
        let badge = adapter.badge(&world).expect("dummy emits one event");
        assert_eq!(badge.count, 1);
        assert_eq!(badge.severity, Severity::Info);

        // Exercise the render path through a detached egui context so
        // the default Event-tree renderer is covered without a real
        // Bevy app. `render` must not panic on a well-formed tree.
        let ctx = egui::Context::default();
        let mut state = TabState::default();
        ctx.run(Default::default(), |ctx| {
            egui::Area::new(egui::Id::new("esc_test_area")).show(ctx, |ui| {
                adapter.render(ui, &mut world, &mut state);
            });
        });
    }

    #[test]
    fn default_render_empty_slice_shows_placeholder() {
        // Just make sure the empty-state path does not panic.
        let ctx = egui::Context::default();
        ctx.run(Default::default(), |ctx| {
            egui::Area::new(egui::Id::new("esc_empty")).show(ctx, |ui| {
                render_event_tree(ui, &[]);
            });
        });
    }
}
