use std::{env, fs, process::ExitCode};

use macrocosmo_ui_dsl::{
    UiDslRenderer, UiFragmentMeta, UiNode,
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
    let source = match env::args().nth(1) {
        Some(path) => CatalogueSource::File(path),
        None => CatalogueSource::Bundled,
    };
    let fragments = load_fragments(&source)?;
    let title = match &source {
        CatalogueSource::Bundled => "Macrocosmo UI DSL Catalogue".to_string(),
        CatalogueSource::File(path) => format!("Macrocosmo UI DSL Catalogue - {path}"),
    };

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default().with_inner_size([1200.0, 800.0]),
        ..Default::default()
    };

    eframe::run_native(
        &title,
        options,
        Box::new(move |_| Ok(Box::new(CatalogueApp::new(fragments)))),
    )?;

    Ok(())
}

enum CatalogueSource {
    Bundled,
    File(String),
}

#[derive(Clone)]
struct CatalogueFragment {
    meta: UiFragmentMeta,
    node: UiNode,
}

fn load_fragments(
    source: &CatalogueSource,
) -> Result<Vec<CatalogueFragment>, Box<dyn std::error::Error>> {
    let (source_name, source_text) = match source {
        CatalogueSource::Bundled => (
            "macrocosmo-ui-dsl/examples/catalogue.lua".to_string(),
            include_str!("catalogue.lua").to_string(),
        ),
        CatalogueSource::File(path) => (path.clone(), fs::read_to_string(path)?),
    };

    let lua = Lua::new();
    register_ui_fragment_definition_accumulator(&lua)?;
    register_ui_dsl_module(&lua)?;
    lua.load(&source_text).set_name(&source_name).exec()?;

    let registry = parse_ui_fragment_definitions(&lua)?;
    let mut fragments = Vec::with_capacity(registry.len());

    for definition in registry.iter() {
        let view = lua.create_table()?;
        view.set("tick", 42)?;
        fragments.push(CatalogueFragment {
            meta: definition.meta.clone(),
            node: definition.inflate(&lua, view)?,
        });
    }

    Ok(fragments)
}

struct CatalogueApp {
    fragments: Vec<CatalogueFragment>,
    selected: usize,
    show_all: bool,
    command_log: Vec<String>,
}

impl CatalogueApp {
    fn new(fragments: Vec<CatalogueFragment>) -> Self {
        Self {
            fragments,
            selected: 0,
            show_all: false,
            command_log: Vec::new(),
        }
    }

    fn selected_fragment(&self) -> Option<&CatalogueFragment> {
        self.fragments.get(self.selected)
    }
}

impl eframe::App for CatalogueApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        egui::TopBottomPanel::top("catalogue_header").show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.heading("UI DSL Catalogue");
                ui.separator();
                ui.label(format!("{} fragments", self.fragments.len()));
                ui.checkbox(&mut self.show_all, "Render all");
            });
        });

        egui::SidePanel::left("catalogue_fragments")
            .resizable(true)
            .default_width(300.0)
            .show(ctx, |ui| {
                ui.heading("Fragments");
                ui.separator();
                egui::ScrollArea::vertical().show(ui, |ui| {
                    for (index, fragment) in self.fragments.iter().enumerate() {
                        let selected = self.selected == index && !self.show_all;
                        if ui.selectable_label(selected, &fragment.meta.id).clicked() {
                            self.selected = index;
                            self.show_all = false;
                        }
                        ui.small(fragment.meta.labels.join(", "));
                        ui.add_space(6.0);
                    }
                });
            });

        egui::SidePanel::right("catalogue_inspector")
            .resizable(true)
            .default_width(300.0)
            .show(ctx, |ui| {
                ui.heading("Inspector");
                ui.separator();
                if let Some(fragment) = self.selected_fragment() {
                    ui.label(format!("id: {}", fragment.meta.id));
                    ui.label(format!("labels: {}", fragment.meta.labels.join(", ")));
                    ui.label(format!("order: {}", fragment.meta.order));
                    ui.label(format!(
                        "requires: {}",
                        format_bindings(&fragment.meta.context.requires)
                    ));
                    ui.label(format!(
                        "optional: {}",
                        format_bindings(&fragment.meta.context.optional)
                    ));
                }

                ui.separator();
                ui.heading("Commands");
                if ui.button("Clear").clicked() {
                    self.command_log.clear();
                }
                egui::ScrollArea::vertical().show(ui, |ui| {
                    for command in self.command_log.iter().rev().take(32) {
                        ui.monospace(command);
                    }
                });
            });

        egui::CentralPanel::default().show(ctx, |ui| {
            egui::ScrollArea::both().show(ui, |ui| {
                let mut renderer = UiDslRenderer::default();
                let output = if self.show_all {
                    renderer.render_many(ui, self.fragments.iter().map(|fragment| &fragment.node))
                } else if let Some(fragment) = self.selected_fragment() {
                    renderer.render(ui, &fragment.node)
                } else {
                    Default::default()
                };

                self.command_log.extend(output.clicked_commands);
            });
        });
    }
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
