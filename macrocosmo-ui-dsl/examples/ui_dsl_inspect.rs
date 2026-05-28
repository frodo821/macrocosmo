use std::{env, fs, process::ExitCode};

use macrocosmo_ui_dsl::lua::{
    parse_ui_fragment_definitions, register_ui_dsl_module,
    register_ui_fragment_definition_accumulator,
};
use mlua::Lua;

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("{err}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<(), Box<dyn std::error::Error>> {
    let Some(path) = env::args().nth(1) else {
        return Err(
            "usage: cargo run -p macrocosmo-ui-dsl --example ui_dsl_inspect -- <lua-file>".into(),
        );
    };

    let source = fs::read_to_string(&path)?;
    let lua = Lua::new();
    register_ui_fragment_definition_accumulator(&lua)?;
    register_ui_dsl_module(&lua)?;

    lua.load(&source).set_name(&path).exec()?;

    let registry = parse_ui_fragment_definitions(&lua)?;
    println!("loaded {} UI fragments from {path}", registry.len());

    for definition in registry.iter() {
        println!();
        println!("== {} ==", definition.meta.id);
        println!("labels: {}", definition.meta.labels.join(", "));
        println!("order: {}", definition.meta.order);
        println!(
            "requires: {}",
            format_bindings(&definition.meta.context.requires)
        );
        println!(
            "optional: {}",
            format_bindings(&definition.meta.context.optional)
        );
        if let Some(source) = &definition.meta.source {
            if let Some(short_src) = &source.short_src {
                println!("source: {short_src}:{}", source.line.unwrap_or_default());
            }
        }

        let view = lua.create_table()?;
        let node = definition.inflate(&lua, view)?;
        println!("{node:#?}");
    }

    Ok(())
}

fn format_bindings(bindings: &[macrocosmo_ui_dsl::UiContextBinding]) -> String {
    if bindings.is_empty() {
        return "-".to_string();
    }

    bindings
        .iter()
        .map(|binding| match binding.value_type {
            Some(value_type) => format!("{}:{}", binding.key, value_type.lua_tag()),
            None => binding.key.clone(),
        })
        .collect::<Vec<_>>()
        .join(", ")
}
