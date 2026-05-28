//! Compatibility re-export for Lua-facing UI DSL helpers.

pub use macrocosmo_ui_dsl::lua::*;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scripting::globals::setup_globals;
    use mlua::prelude::*;
    use std::path::Path;

    #[test]
    fn define_ui_fragment_accumulates_tables() {
        let lua = Lua::new();
        setup_globals(&lua, Path::new(".")).expect("setup globals");

        let returned: LuaTable = lua
            .load(
                r#"
                local ui = require("macrocosmo.ui")
                return define_ui_fragment {
                    id = "test.fragment",
                    labels = { "test" },
                    render = function(view)
                        return ui.text("hello")
                    end,
                }
                "#,
            )
            .eval()
            .expect("define_ui_fragment returns table");

        assert_eq!(returned.get::<String>("_def_type").unwrap(), "ui_fragment");
        assert_eq!(returned.get::<String>("id").unwrap(), "test.fragment");

        let defs: LuaTable = lua.globals().get(UI_FRAGMENT_ACCUMULATOR).unwrap();
        assert_eq!(defs.len().unwrap(), 1);
    }
}
