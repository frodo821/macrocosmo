//! Lua-backed Situation Center adapters.
//!
//! `LuaOngoingTabAdapter` is the older event-tree placeholder for a future
//! `define_situation_tab` API. `LuaUiFragmentTab` is the first live bridge for
//! the newer Lua UI DSL: it renders an already-registered `define_ui_fragment`
//! descriptor through the host-agnostic egui renderer, without dispatching
//! actions.

use std::any::Any;

use bevy::prelude::*;
use macrocosmo_ui_dsl::{UiDslRenderer, lua::parse_ui_fragment_definitions};

use super::state::TabState;
use super::tab::{OngoingTab, SituationTab, TabBadge, TabMeta};
use super::types::Event;

/// Registration descriptor for a Lua-defined ongoing tab. The Lua API
/// issue will construct one of these from the Lua table and push an
/// [`LuaOngoingTabAdapter`] wrapping it onto the registry.
#[derive(Clone, Debug)]
pub struct LuaTabRegistration {
    pub id: &'static str,
    pub display_name: &'static str,
    pub order: i32,
    /// Handle to the Lua `collect` function. The ESC-1 placeholder
    /// stores only the diagnostic string; the Lua API issue will
    /// replace this with a `mlua::RegistryKey` (or similar) and invoke
    /// the function each frame inside `collect`.
    pub lua_callback_id: String,
}

/// Rust-side adapter that forwards `collect` to a Lua function.
///
/// ESC-1 placeholder — `collect` always returns an empty `Vec<Event>`.
/// The Lua API issue wires the real invocation.
pub struct LuaOngoingTabAdapter {
    pub registration: LuaTabRegistration,
}

impl OngoingTab for LuaOngoingTabAdapter {
    fn meta(&self) -> TabMeta {
        TabMeta {
            id: self.registration.id,
            display_name: self.registration.display_name,
            order: self.registration.order,
        }
    }

    fn collect(&self, _world: &World) -> Vec<Event> {
        // Placeholder: the Lua API issue will:
        //   1. Look up the Lua function via `registration.lua_callback_id`.
        //   2. Build a read-only gamestate view (see
        //      `scripting::gamestate_view`) and pass it to the callback.
        //   3. Convert the returned Lua table into `Vec<Event>`.
        //   4. Apply timeout + error containment (#349 dispatch pattern).
        Vec::new()
    }

    fn badge(&self, _world: &World) -> Option<TabBadge> {
        None
    }
}

/// Read-only Situation Center tab backed by one Lua UI DSL fragment.
///
/// This deliberately performs no command dispatch: the renderer returns clicked
/// command ids, but host validation/dispatch is still a separate migration
/// step. Until then the tab is useful as a live smoke test for real game-loaded
/// Lua UI definitions.
pub struct LuaUiFragmentTab {
    pub tab_id: &'static str,
    pub display_name: &'static str,
    pub order: i32,
    pub fragment_id: &'static str,
}

impl SituationTab for LuaUiFragmentTab {
    fn meta(&self) -> TabMeta {
        TabMeta {
            id: self.tab_id,
            display_name: self.display_name,
            order: self.order,
        }
    }

    fn badge(&self, _world: &World) -> Option<TabBadge> {
        None
    }

    fn render(&self, ui: &mut bevy_egui::egui::Ui, world: &World, _state: &mut TabState) {
        let Some(engine) = world.get_resource::<crate::scripting::ScriptEngine>() else {
            ui.label(bevy_egui::egui::RichText::new("(Lua scripting is unavailable)").weak());
            return;
        };

        let lua = engine.lua();
        let registry = match parse_ui_fragment_definitions(lua) {
            Ok(registry) => registry,
            Err(err) => {
                ui.label(
                    bevy_egui::egui::RichText::new(format!(
                        "Failed to parse Lua UI fragments: {err}"
                    ))
                    .weak(),
                );
                return;
            }
        };

        let Some(fragment) = registry.get(self.fragment_id) else {
            ui.label(
                bevy_egui::egui::RichText::new(format!(
                    "Lua UI fragment '{}' is not registered",
                    self.fragment_id
                ))
                .weak(),
            );
            return;
        };

        let view = match lua.create_table() {
            Ok(view) => view,
            Err(err) => {
                ui.label(
                    bevy_egui::egui::RichText::new(format!("Failed to create view: {err}")).weak(),
                );
                return;
            }
        };
        let node = match fragment.inflate(lua, view) {
            Ok(node) => node,
            Err(err) => {
                ui.label(
                    bevy_egui::egui::RichText::new(format!(
                        "Failed to render Lua UI fragment '{}': {err}",
                        self.fragment_id
                    ))
                    .weak(),
                );
                return;
            }
        };

        let mut renderer = UiDslRenderer::default();
        let _ = renderer.render(ui, &node);
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::situation_center::registry::{AppSituationExt, SituationTabRegistry};
    use crate::ui::situation_center::tab::OngoingTabAdapter;

    #[test]
    fn lua_adapter_registers_through_standard_api() {
        // Placeholder Lua-defined tabs use exactly the same registration
        // path as Rust ongoing tabs — no special-case API required. If
        // this test still passes after the Lua API issue lands, the
        // future-proofing contract holds.
        let mut app = App::new();
        let adapter = LuaOngoingTabAdapter {
            registration: LuaTabRegistration {
                id: "lua_stub",
                display_name: "Lua Stub",
                order: 500,
                lua_callback_id: "stub::collect".into(),
            },
        };
        app.register_ongoing_situation_tab(adapter);

        let world = app.world();
        let registry = world.resource::<SituationTabRegistry>();
        let tab = registry.get("lua_stub").expect("lua tab registered");
        assert_eq!(tab.meta().display_name, "Lua Stub");
        // Adapter's collect returns an empty Vec, so no badge surfaces.
        assert!(tab.badge(world).is_none());
        // Silence unused-import warnings on toolchains where
        // OngoingTabAdapter / SituationTab are trivially reachable.
        let _ = std::any::TypeId::of::<OngoingTabAdapter<LuaOngoingTabAdapter>>();
        let _: &dyn SituationTab = tab;
    }

    #[test]
    fn lua_ui_fragment_tab_renders_registered_fragment() {
        let mut world = World::new();
        world.insert_resource(
            crate::scripting::ScriptEngine::new_with_scripts_dir(std::path::PathBuf::from(
                "scripts",
            ))
            .expect("script engine"),
        );

        // Use the real engine resource's Lua state because the tab reads from World.
        let engine = world.resource::<crate::scripting::ScriptEngine>();
        engine
            .lua()
            .load(
                r#"
                local ui = require("macrocosmo.ui")
                define_ui_fragment {
                    id = "core.ui.esc.notifications",
                    render = function(view)
                        return ui.section {
                            title = "ESC Notifications",
                            children = { ui.text("Survey complete") },
                        }
                    end,
                }
                "#,
            )
            .exec()
            .expect("define fragment on engine");

        let tab = LuaUiFragmentTab {
            tab_id: "lua_ui_preview",
            display_name: "Lua UI",
            order: 800,
            fragment_id: "core.ui.esc.notifications",
        };
        let ctx = bevy_egui::egui::Context::default();
        let mut state = TabState::default();
        let _ =
            ctx.run(Default::default(), |ctx| {
                bevy_egui::egui::Area::new(bevy_egui::egui::Id::new("lua_ui_fragment_tab_test"))
                    .show(ctx, |ui| {
                        tab.render(ui, &world, &mut state);
                    });
            });
    }
}
