//! egui floating panel for the Empire Situation Center (#344 / ESC-1).
//!
//! Two systems:
//!
//! 1. [`toggle_situation_center`] — runs in `Update`; watches for the
//!    F3 keybind and flips [`SituationCenterState::open`]. This will
//!    be replaced by a keybinding registry lookup in #347; the
//!    hardcoded key is intentional until then.
//! 2. [`draw_situation_center_system`] — runs in `EguiPrimaryContextPass`
//!    as part of the chained UI pipeline. Renders the tab strip + the
//!    body of the active tab.
//!
//! Borrow note: trait objects from the [`SituationTabRegistry`] need
//! a `&World` for `badge` / `render`. To avoid borrow-checker conflicts
//! with `ResMut<SituationCenterState>` inside one system, the draw
//! system pulls both out of the world using `World::resource_scope`.
//! That API owns the `Res`/`ResMut` lifetime explicitly so the
//! registry can borrow the rest of the world for the duration of the
//! render closure.

use bevy::ecs::system::SystemState;
use bevy::prelude::*;
use bevy_egui::{EguiContexts, egui};

use super::notifications_tab::severity_tint;
use super::registry::SituationTabRegistry;
use super::state::SituationCenterState;
use super::tab::{TabBadge, TabId, TabMeta};
#[cfg(test)]
use super::types::Severity;

/// Hardcoded keybind. Tracked for replacement in #347 (In-game
/// keybinding manager).
pub const TOGGLE_KEY: KeyCode = KeyCode::F3;

/// Toggle the ESC panel when the player presses the configured key.
///
/// Registered in `Update` (not `EguiPrimaryContextPass`) so the toggle
/// fires exactly once per press regardless of frame pacing in the UI
/// schedule.
pub fn toggle_situation_center(
    keys: Res<ButtonInput<KeyCode>>,
    mut state: ResMut<SituationCenterState>,
) {
    if keys.just_pressed(TOGGLE_KEY) {
        state.open = !state.open;
    }
}

/// Draw the ESC floating window, tab strip, and active tab body.
///
/// Exclusive system — it needs `&World` for every registered tab's
/// `badge` / `render` as well as a `&mut SituationCenterState` for the
/// active-tab pointer and per-tab scratch state. The egui context is
/// pulled via a transient [`SystemState`], then cloned (egui's
/// `Context` is `Arc`-backed, so this is cheap) so the underlying
/// contexts borrow drops before we touch the world again.
///
/// Runs in `EguiPrimaryContextPass` chained into `UiPlugin`'s
/// pipeline after `draw_overlays_system` — it shares the "floating
/// window" conceptual slot with the research panel and ship designer.
pub fn draw_situation_center_system(world: &mut World) {
    // Cheap early-exit when no tabs are registered (e.g. on game start
    // before any plugin has added one). This keeps the ctx borrow from
    // running at all for headless / tests-with-egui scenarios.
    let registry_empty = world
        .get_resource::<SituationTabRegistry>()
        .is_none_or(|r| r.is_empty());
    let closed = world
        .get_resource::<SituationCenterState>()
        .is_none_or(|s| !s.open);
    if closed {
        return;
    }

    let ctx = {
        let mut system_state: SystemState<EguiContexts> = SystemState::new(world);
        let mut contexts = system_state.get_mut(world);
        match contexts.ctx_mut() {
            Ok(ctx) => ctx.clone(),
            Err(_) => return,
        }
    };

    // Ensure an active tab exists — pick the first registered tab if
    // none is set (first-open behaviour).
    world.resource_scope::<SituationCenterState, ()>(|world, mut state| {
        if state.active_tab.is_none() {
            if let Some(registry) = world.get_resource::<SituationTabRegistry>() {
                if let Some(first) = registry.iter().next() {
                    state.active_tab = Some(first.meta().id);
                }
            }
        }

        // Gather tab meta + badge up front so we don't hold a borrow on
        // the registry when rendering the active tab body (which needs
        // `&World` for `render`).
        let meta_and_badges: Vec<(TabMeta, Option<TabBadge>)> =
            if let Some(registry) = world.get_resource::<SituationTabRegistry>() {
                registry
                    .iter()
                    .map(|t| (t.meta(), t.badge(world)))
                    .collect()
            } else {
                Vec::new()
            };

        // Borrow split: egui `open` separately so the window X button
        // works without fighting the ResMut<SituationCenterState>.
        let mut open = state.open;

        egui::Window::new("Empire Situation Center")
            .id(egui::Id::new("empire_situation_center"))
            .open(&mut open)
            .resizable(true)
            .default_size([680.0, 480.0])
            .show(&ctx, |ui| {
                if registry_empty || meta_and_badges.is_empty() {
                    ui.label(egui::RichText::new("(no situation tabs registered)").weak());
                    return;
                }

                // --- Tab strip -----------------------------------------------
                ui.horizontal(|ui| {
                    for (meta, badge) in &meta_and_badges {
                        let is_active = state.active_tab == Some(meta.id);
                        let label = build_tab_label(meta, badge.as_ref());
                        if ui.selectable_label(is_active, label).clicked() && !is_active {
                            state.active_tab = Some(meta.id);
                        }
                    }
                });
                ui.separator();

                // --- Active tab body ----------------------------------------
                let active_id = match state.active_tab {
                    Some(id) => id,
                    None => return,
                };
                render_active_tab(ui, world, &mut state, active_id);
            });

        state.open = open;
    });
}

/// Render the active tab's body. Pulled out of the window closure so
/// the borrow dance with `SituationTabRegistry` is localised.
fn render_active_tab(
    ui: &mut egui::Ui,
    world: &World,
    state: &mut SituationCenterState,
    active_id: TabId,
) {
    let Some(registry) = world.get_resource::<SituationTabRegistry>() else {
        ui.label(egui::RichText::new("(registry missing)").weak());
        return;
    };
    let Some(tab) = registry.get(active_id) else {
        ui.label(egui::RichText::new(format!("(unknown tab: {})", active_id)).weak());
        return;
    };
    let tab_state = state.tab_state_mut(active_id);
    tab.render(ui, world, tab_state);
}

fn build_tab_label(meta: &TabMeta, badge: Option<&TabBadge>) -> egui::WidgetText {
    match badge {
        None => egui::RichText::new(meta.display_name).into(),
        Some(b) if b.count == 0 => egui::RichText::new(meta.display_name).into(),
        Some(b) => {
            let color = severity_tint(b.severity);
            // Concatenate base text + coloured badge suffix into a
            // LayoutJob so both layout passes see a single widget.
            let mut job = egui::text::LayoutJob::default();
            job.append(
                meta.display_name,
                0.0,
                egui::TextFormat {
                    color: egui::Color32::WHITE,
                    ..Default::default()
                },
            );
            job.append(
                &format!(" ({})", b.count),
                4.0,
                egui::TextFormat {
                    color,
                    ..Default::default()
                },
            );
            job.into()
        }
    }
}

/// Mild escape hatch for tests that want to inspect the mapping from
/// a badge to its render colour without going through egui plumbing.
#[cfg(test)]
pub(super) fn badge_color_for_tests(severity: Severity) -> egui::Color32 {
    severity_tint(severity)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::situation_center::registry::AppSituationExt;
    use crate::ui::situation_center::state::{SituationCenterState, TabState};
    use crate::ui::situation_center::tab::{SituationTab, TabMeta};
    use crate::ui::situation_center::types::Severity;
    use std::any::Any;

    struct TestTab;

    impl SituationTab for TestTab {
        fn meta(&self) -> TabMeta {
            TabMeta {
                id: "test",
                display_name: "Test",
                order: 100,
            }
        }
        fn badge(&self, _w: &World) -> Option<TabBadge> {
            None
        }
        fn render(&self, _ui: &mut egui::Ui, _w: &World, _s: &mut TabState) {}
        fn as_any(&self) -> &dyn Any {
            self
        }
    }

    /// Pressing F3 when `Update` runs must flip the panel state.
    #[test]
    fn toggle_flips_open_flag() {
        let mut app = App::new();
        app.insert_resource(ButtonInput::<KeyCode>::default());
        app.insert_resource(SituationCenterState::default());
        app.add_systems(Update, toggle_situation_center);
        app.register_situation_tab(TestTab);

        // Frame 1: press F3.
        app.world_mut()
            .resource_mut::<ButtonInput<KeyCode>>()
            .press(TOGGLE_KEY);
        app.update();
        assert!(app.world().resource::<SituationCenterState>().open);

        // Frame 2: release, no change.
        {
            let mut keys = app.world_mut().resource_mut::<ButtonInput<KeyCode>>();
            keys.release(TOGGLE_KEY);
            keys.clear_just_pressed(TOGGLE_KEY);
        }
        app.update();
        assert!(app.world().resource::<SituationCenterState>().open);

        // Frame 3: re-press → flips back.
        app.world_mut()
            .resource_mut::<ButtonInput<KeyCode>>()
            .press(TOGGLE_KEY);
        app.update();
        assert!(!app.world().resource::<SituationCenterState>().open);
    }

    #[test]
    fn badge_color_matches_severity_tint() {
        assert_eq!(
            badge_color_for_tests(Severity::Critical),
            severity_tint(Severity::Critical)
        );
        assert_eq!(
            badge_color_for_tests(Severity::Warn),
            severity_tint(Severity::Warn)
        );
    }
}
