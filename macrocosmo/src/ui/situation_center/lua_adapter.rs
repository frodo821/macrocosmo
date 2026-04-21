//! Lua `OngoingTab` adapter placeholder (#344 / ESC-1).
//!
//! **Scope note**: The real `define_situation_tab` Lua API is a separate
//! issue. ESC-1 ships only the Rust-side skeleton so downstream work
//! (#345, #346, the Lua API issue) can agree on the contract:
//!
//! 1. A Lua-defined tab is registered via
//!    `define_situation_tab { id = ..., display_name = ..., collect = fn }`
//!    which (in a future commit) pushes a [`LuaOngoingTabAdapter`] onto
//!    the `SituationTabRegistry`.
//! 2. `collect` is a Lua function that returns a table shaped like a
//!    `Vec<Event>`. The conversion path is documented in
//!    `docs/plan-326-esc.md` §Lua boundary.
//! 3. The adapter implements [`OngoingTab`] so the framework's default
//!    Event-tree renderer handles display — **no refactor of the
//!    `SituationTab` / `OngoingTab` traits is required when the Lua API
//!    lands.**
//!
//! The current implementation returns an empty `Vec<Event>` from
//! `collect`. It exists to (a) prove the trait fits a Lua-callback
//! shape, and (b) give the Lua API issue a named type to populate.

use bevy::prelude::*;

use super::tab::{OngoingTab, TabBadge, TabMeta};
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::situation_center::registry::{AppSituationExt, SituationTabRegistry};
    use crate::ui::situation_center::tab::OngoingTabAdapter;
    use crate::ui::situation_center::tab::SituationTab;

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
}
