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

/// Default keybind for the ESC toggle. Re-exported for tests and any
/// remaining code that wants to know the *default* binding (e.g. tooltip
/// hints). Live binding lookup goes through
/// [`crate::input::KeybindingRegistry`] under
/// [`crate::input::actions::UI_TOGGLE_SITUATION_CENTER`] (#347).
pub const DEFAULT_TOGGLE_KEY: KeyCode = KeyCode::F3;

/// Toggle the ESC panel when the player presses the configured key.
///
/// Registered in `Update` (not `EguiPrimaryContextPass`) so the toggle
/// fires exactly once per press regardless of frame pacing in the UI
/// schedule.
///
/// #347: Looks the binding up via [`crate::input::KeybindingRegistry`].
/// The registry is `Option<Res<…>>` so headless tests that don't install
/// `KeybindingPlugin` fall back to the [`DEFAULT_TOGGLE_KEY`] check.
pub fn toggle_situation_center(
    keys: Res<ButtonInput<KeyCode>>,
    keybindings: Option<Res<crate::input::KeybindingRegistry>>,
    mut state: ResMut<SituationCenterState>,
) {
    let pressed = match keybindings.as_deref() {
        Some(kb) => kb.is_just_pressed(crate::input::actions::UI_TOGGLE_SITUATION_CENTER, &keys),
        None => keys.just_pressed(DEFAULT_TOGGLE_KEY),
    };
    if pressed {
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

        let screen = ctx.screen_rect();
        let top_offset = 28.0;
        let sheet_height = (screen.height() - top_offset).max(360.0);
        let sheet_width = (screen.width() * 0.42)
            .clamp(560.0, 760.0)
            .min(screen.width().max(320.0));
        let mut close_requested = false;

        egui::Area::new(egui::Id::new("empire_situation_center_sheet"))
            .order(egui::Order::Foreground)
            .fixed_pos(egui::pos2(screen.left(), screen.top() + top_offset))
            .show(&ctx, |ui| {
                egui::Frame::NONE
                    .fill(egui::Color32::from_rgb(18, 18, 20))
                    .inner_margin(12.0)
                    .show(ui, |ui| {
                        let content_size = egui::vec2(sheet_width, sheet_height);
                        ui.set_min_size(content_size);
                        ui.set_clip_rect(ui.max_rect());

                        ui.allocate_ui_with_layout(
                            content_size,
                            egui::Layout::top_down(egui::Align::Min),
                            |ui| {
                                ui.set_min_size(content_size);
                                ui.set_max_size(content_size);

                                ui.allocate_ui_with_layout(
                                    egui::vec2(sheet_width, 36.0),
                                    egui::Layout::left_to_right(egui::Align::Center),
                                    |ui| {
                                        ui.heading("Empire Situation Center");
                                        ui.with_layout(
                                            egui::Layout::right_to_left(egui::Align::Center),
                                            |ui| {
                                                if ui.button("\u{00D7}").clicked() {
                                                    close_requested = true;
                                                }
                                            },
                                        );
                                    },
                                );
                                ui.separator();

                                if registry_empty || meta_and_badges.is_empty() {
                                    ui.label(
                                        egui::RichText::new("(no situation tabs registered)")
                                            .weak(),
                                    );
                                    return;
                                }

                                let body_height = sheet_height - 52.0;
                                let tab_width = 176.0;
                                let content_width = sheet_width - tab_width - 18.0;
                                ui.allocate_ui_with_layout(
                                    egui::vec2(sheet_width, body_height),
                                    egui::Layout::left_to_right(egui::Align::Min),
                                    |ui| {
                                        ui.allocate_ui_with_layout(
                                            egui::vec2(tab_width, body_height),
                                            egui::Layout::top_down(egui::Align::Min),
                                            |ui| {
                                                ui.set_width(tab_width);
                                                for (meta, badge) in &meta_and_badges {
                                                    let is_active = state.active_tab == Some(meta.id);
                                                    let label = build_tab_label(meta, badge.as_ref());
                                                    let tab_resp = ui.add_sized(
                                                        [tab_width, 28.0],
                                                        egui::Button::new(label).selected(is_active),
                                                    );
                                                    #[cfg(feature = "remote")]
                                                    if let Some(mut reg) = world
                                                        .get_resource_mut::<crate::ui::UiElementRegistry>()
                                                    {
                                                        crate::ui::register_ui_element(
                                                            &mut reg,
                                                            &format!("esc.tab.{}", meta.display_name),
                                                            meta.display_name,
                                                            tab_resp.rect,
                                                        );
                                                    }
                                                    if tab_resp.clicked() && !is_active {
                                                        state.active_tab = Some(meta.id);
                                                    }
                                                }
                                            },
                                        );
                                        ui.separator();
                                        ui.allocate_ui_with_layout(
                                            egui::vec2(content_width, body_height),
                                            egui::Layout::top_down(egui::Align::Min),
                                            |ui| {
                                                ui.set_width(content_width);
                                                egui::ScrollArea::vertical()
                                                    .auto_shrink([false, false])
                                                    .max_width(content_width)
                                                    .max_height(body_height)
                                                    .show(ui, |ui| {
                                                        ui.set_max_width(content_width);
                                                        let active_id = match state.active_tab {
                                                            Some(id) => id,
                                                            None => return,
                                                        };
                                                        render_active_tab(
                                                            ui,
                                                            world,
                                                            &mut state,
                                                            active_id,
                                                        );
                                                    });
                                            },
                                        );
                                    },
                                );
                            },
                        );
                    });
            });

        if close_requested {
            state.open = false;
        }
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
            .press(DEFAULT_TOGGLE_KEY);
        app.update();
        assert!(app.world().resource::<SituationCenterState>().open);

        // Frame 2: release, no change.
        {
            let mut keys = app.world_mut().resource_mut::<ButtonInput<KeyCode>>();
            keys.release(DEFAULT_TOGGLE_KEY);
            keys.clear_just_pressed(DEFAULT_TOGGLE_KEY);
        }
        app.update();
        assert!(app.world().resource::<SituationCenterState>().open);

        // Frame 3: re-press → flips back.
        app.world_mut()
            .resource_mut::<ButtonInput<KeyCode>>()
            .press(DEFAULT_TOGGLE_KEY);
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
