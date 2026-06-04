use std::{env, fs, process::ExitCode};

use macrocosmo_ui_dsl::{
    UiDslRenderer,
    lua::{
        parse_ui_fragment_definitions, register_ui_dsl_module,
        register_ui_fragment_definition_accumulator,
    },
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
            "usage: cargo run -p macrocosmo-ui-dsl --example ui_dsl_render_smoke -- <lua-file>"
                .into(),
        );
    };

    let source = fs::read_to_string(&path)?;
    let lua = Lua::new();
    register_ui_fragment_definition_accumulator(&lua)?;
    register_ui_dsl_module(&lua)?;
    lua.load(&source).set_name(&path).exec()?;

    let registry = parse_ui_fragment_definitions(&lua)?;
    let nodes = registry
        .iter()
        .map(|definition| {
            let view = lua.create_table()?;
            definition.inflate(&lua, view)
        })
        .collect::<mlua::Result<Vec<_>>>()?;

    let ctx = egui::Context::default();
    let mut renderer = UiDslRenderer::default();
    let mut output = Default::default();
    let _ = ctx.run(Default::default(), |ctx| {
        egui::CentralPanel::default().show(ctx, |ui| {
            output = renderer.render_many(ui, &nodes);
        });
    });

    println!("loaded fragments: {}", registry.len());
    println!("rendered nodes: {}", output.stats.nodes);
    println!("sections: {}", output.stats.sections);
    println!("text nodes: {}", output.stats.text);
    println!("progress bars: {}", output.stats.progress);
    println!("tooltips: {}", output.stats.tooltips);
    println!("modified values: {}", output.stats.modified_values);
    println!("buttons/actions: {}", output.stats.buttons);
    println!(
        "disabled buttons/actions: {}",
        output.stats.disabled_buttons
    );
    println!("commands: {}", output.stats.commands);
    println!(
        "clicked commands in smoke frame: {}",
        output.clicked_commands.len()
    );

    Ok(())
}
