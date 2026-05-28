//! Lua-facing UI DSL registration helpers.

use crate::{
    UiConditionDisplay, UiConditionOperator, UiContextBinding, UiContextKey, UiContextValueType,
    UiFragmentContextSpec, UiFragmentMeta, UiFragmentSource, UiModifierDisplayLine, UiNode,
};
use mlua::prelude::*;
use std::{
    collections::{BTreeMap, BTreeSet},
    error::Error,
    fmt,
};

pub const UI_FRAGMENT_ACCUMULATOR: &str = "_ui_fragment_definitions";
pub const UI_FRAGMENT_SOURCE_FIELD: &str = "_ui_source";

/// Register `define_ui_fragment { ... }` and its accumulator.
///
/// This is intentionally separate from the game's generic `define_xxx`
/// helper so UI fragments can capture source diagnostics without changing
/// unrelated Lua definition shapes.
pub fn register_ui_fragment_definition_accumulator(lua: &Lua) -> LuaResult<()> {
    let globals = lua.globals();
    let acc = lua.create_table()?;
    globals.set(UI_FRAGMENT_ACCUMULATOR, acc)?;

    let define = lua.create_function(|lua, table: LuaTable| {
        table.set("_def_type", "ui_fragment")?;

        let defs: LuaTable = lua.globals().get(UI_FRAGMENT_ACCUMULATOR)?;
        let registration_order = defs.len()? + 1;
        table.set("_registration_order", registration_order)?;

        if let Some((source, short_src, line)) = lua.inspect_stack(1, |debug| {
            let source = debug.source();
            (
                source.source.map(|value| value.into_owned()),
                source.short_src.map(|value| value.into_owned()),
                debug.current_line(),
            )
        }) {
            let source_table = lua.create_table()?;
            if let Some(source) = source {
                source_table.set("source", source)?;
            }
            if let Some(short_src) = short_src {
                source_table.set("short_src", short_src)?;
            }
            if let Some(line) = line {
                source_table.set("line", line)?;
            }
            source_table.set("registration_order", registration_order)?;
            table.set(UI_FRAGMENT_SOURCE_FIELD, source_table)?;
        }

        defs.set(registration_order, table.clone())?;
        Ok(table)
    })?;
    globals.set("define_ui_fragment", define)?;

    Ok(())
}

/// Register UI primitive table builders.
///
/// The builders return plain descriptor tables so Lua scripts can author the
/// future DSL shape before Rust renderers are wired to consume it.
pub fn register_ui_dsl_helpers(lua: &Lua) -> LuaResult<()> {
    let globals = lua.globals();
    globals.set("ui", create_ui_dsl_module(lua)?)?;
    Ok(())
}

pub fn register_ui_dsl_module(lua: &Lua) -> LuaResult<()> {
    lua.register_module("macrocosmo.ui", create_ui_dsl_module(lua)?)
}

pub fn create_ui_dsl_module(lua: &Lua) -> LuaResult<LuaTable> {
    let ui = lua.create_table()?;

    ui.set("section", make_node_builder(lua, "section")?)?;
    ui.set("row", make_node_builder(lua, "row")?)?;
    ui.set("vstack", make_node_builder(lua, "vstack")?)?;
    ui.set("hstack", make_node_builder(lua, "hstack")?)?;
    ui.set("grid", make_node_builder(lua, "grid")?)?;
    ui.set("tabs", make_node_builder(lua, "tabs")?)?;

    let text = lua.create_function(|lua, value: String| {
        let t = lua.create_table()?;
        t.set("_ui_node", "text")?;
        t.set("value", value)?;
        Ok(t)
    })?;
    ui.set("text", text)?;

    let progress = lua.create_function(|lua, value: f64| {
        let t = lua.create_table()?;
        t.set("_ui_node", "progress")?;
        t.set("value", value)?;
        Ok(t)
    })?;
    ui.set("progress", progress)?;

    let tooltip = lua.create_function(|_, opts: LuaTable| {
        opts.set("_ui_node", "tooltip")?;
        Ok(opts)
    })?;
    ui.set("tooltip", tooltip)?;

    let modified_value = lua.create_function(|_, opts: LuaTable| {
        opts.set("_ui_node", "modified_value")?;
        Ok(opts)
    })?;
    ui.set("modified_value", modified_value)?;

    let button = lua.create_function(|_, opts: LuaTable| {
        opts.set("_ui_node", "button")?;
        Ok(opts)
    })?;
    ui.set("button", button)?;

    let action = lua.create_function(|_, opts: LuaTable| {
        opts.set("_ui_node", "action")?;
        Ok(opts)
    })?;
    ui.set("action", action)?;

    Ok(ui)
}

fn make_node_builder(lua: &Lua, kind: &'static str) -> LuaResult<LuaFunction> {
    lua.create_function(move |_, opts: LuaTable| {
        opts.set("_ui_node", kind)?;
        Ok(opts)
    })
}

/// Lua-backed fragment definition parsed from the accumulator.
pub struct LuaUiFragmentDefinition {
    pub meta: UiFragmentMeta,
    pub render: LuaRegistryKey,
}

impl LuaUiFragmentDefinition {
    pub fn inflate(&self, lua: &Lua, view: LuaTable) -> LuaResult<UiNode> {
        let render: LuaFunction = lua.registry_value(&self.render)?;
        let descriptor: LuaTable = render.call(view)?;
        parse_ui_node(&descriptor)
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct LuaUiFragmentFrame {
    pub fragment_id: String,
    pub node: UiNode,
}

/// Render every registered Lua fragment once with caller-supplied view tables.
///
/// This is a headless dynamic harness: tests and CLI tools can run multiple
/// frames by calling it repeatedly with changing view values, without pulling in
/// a native window backend or the game crate.
pub fn render_lua_fragment_frame(
    lua: &Lua,
    registry: &LuaUiFragmentRegistry,
    mut make_view: impl FnMut(&LuaUiFragmentDefinition, &Lua) -> LuaResult<LuaTable>,
) -> LuaResult<Vec<LuaUiFragmentFrame>> {
    registry
        .iter()
        .map(|definition| {
            let view = make_view(definition, lua)?;
            Ok(LuaUiFragmentFrame {
                fragment_id: definition.meta.id.clone(),
                node: definition.inflate(lua, view)?,
            })
        })
        .collect()
}

/// Parsed Lua fragment definitions in deterministic registry order.
#[derive(Default)]
pub struct LuaUiFragmentRegistry {
    definitions: Vec<LuaUiFragmentDefinition>,
    context_menu_index: Vec<usize>,
}

impl LuaUiFragmentRegistry {
    pub fn iter(&self) -> impl Iterator<Item = &LuaUiFragmentDefinition> {
        self.definitions.iter()
    }

    pub fn get(&self, id: &str) -> Option<&LuaUiFragmentDefinition> {
        self.definitions
            .iter()
            .find(|definition| definition.meta.id == id)
    }

    pub fn get_by_tag(&self, key: &str, value: &str) -> Option<&LuaUiFragmentDefinition> {
        self.definitions.iter().find(|definition| {
            definition
                .meta
                .tags
                .get(key)
                .is_some_and(|tag| tag == value)
        })
    }

    pub fn context_menu_fragments<'a>(
        &'a self,
        query: &UiFacetQuery,
    ) -> Vec<&'a LuaUiFragmentDefinition> {
        self.context_menu_index
            .iter()
            .filter_map(|index| self.definitions.get(*index))
            .filter(|definition| query.matches(&definition.meta.tags))
            .collect()
    }

    pub fn len(&self) -> usize {
        self.definitions.len()
    }

    pub fn is_empty(&self) -> bool {
        self.definitions.is_empty()
    }
}

/// Parse `define_ui_fragment` accumulator contents into typed definitions.
pub fn parse_ui_fragment_definitions(lua: &Lua) -> LuaResult<LuaUiFragmentRegistry> {
    let defs: LuaTable = lua.globals().get(UI_FRAGMENT_ACCUMULATOR)?;
    let mut parsed = Vec::with_capacity(defs.len()? as usize);
    let mut seen_ids = BTreeSet::new();

    for index in 1..=defs.len()? {
        let table: LuaTable = defs.get(index)?;
        let definition = parse_ui_fragment_definition(lua, &table, index as usize)?;
        if !seen_ids.insert(definition.meta.id.clone()) {
            return Err(LuaError::external(UiFragmentParseError::new(
                definition.meta.id.clone(),
                "id",
                "duplicate ui fragment id",
            )));
        }
        parsed.push(definition);
    }

    parsed.sort_by(|a, b| {
        a.meta
            .order
            .cmp(&b.meta.order)
            .then_with(|| a.meta.id.cmp(&b.meta.id))
    });

    let context_menu_index = build_context_menu_index(&parsed)?;

    Ok(LuaUiFragmentRegistry {
        definitions: parsed,
        context_menu_index,
    })
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct UiFacetQuery {
    pub facets: BTreeMap<String, String>,
}

impl UiFacetQuery {
    pub fn new<K, V, I>(facets: I) -> Self
    where
        K: Into<String>,
        V: Into<String>,
        I: IntoIterator<Item = (K, V)>,
    {
        Self {
            facets: facets
                .into_iter()
                .map(|(key, value)| (key.into(), value.into()))
                .collect(),
        }
    }

    pub fn matches(&self, fragment_tags: &BTreeMap<String, String>) -> bool {
        fragment_tags
            .iter()
            .filter(|(key, _)| is_context_menu_facet_key(key))
            .all(|(key, value)| {
                self.facets
                    .get(key)
                    .is_some_and(|query_value| query_value == value)
            })
    }
}

fn build_context_menu_index(definitions: &[LuaUiFragmentDefinition]) -> LuaResult<Vec<usize>> {
    let mut indexes = Vec::new();
    for (index, definition) in definitions.iter().enumerate() {
        if definition
            .meta
            .tags
            .get("part_of")
            .is_some_and(|value| value == "context_menu")
        {
            validate_context_menu_facets(&definition.meta)?;
            indexes.push(index);
        }
    }
    Ok(indexes)
}

fn validate_context_menu_facets(meta: &UiFragmentMeta) -> LuaResult<()> {
    for (key, value) in &meta.tags {
        if is_context_menu_facet_key(key) || key == "command" {
            validate_context_menu_facet_value(&meta.id, key, value)?;
        }
    }
    if !meta.tags.contains_key("target") {
        return Err(parse_error(
            meta.id.clone(),
            "tags.target",
            "context_menu fragment requires target facet",
        ));
    }
    Ok(())
}

fn is_context_menu_facet_key(key: &str) -> bool {
    matches!(
        key,
        "part_of" | "target" | "ctx:selected" | "ctx:selected:ship:class"
    )
}

fn validate_context_menu_facet_value(fragment_id: &str, key: &str, value: &str) -> LuaResult<()> {
    let allowed = match key {
        "part_of" => matches!(value, "context_menu"),
        "target" => matches!(value, "entity:system" | "entity:ship" | "entity:planet"),
        "ctx:selected" => matches!(value, "entity:ship" | "entity:system" | "none"),
        "ctx:selected:ship:class" => matches!(
            value,
            "colonizer" | "surveyor" | "military" | "transport" | "civilian"
        ),
        "command" => value.chars().all(|ch| {
            ch.is_ascii_lowercase() || ch.is_ascii_digit() || matches!(ch, '.' | '_' | '-')
        }),
        _ => true,
    };
    if allowed {
        Ok(())
    } else {
        Err(parse_error(
            fragment_id.to_string(),
            format!("tags.{key}"),
            format!("unsupported context menu facet value '{value}'"),
        ))
    }
}

fn parse_ui_fragment_definition(
    lua: &Lua,
    table: &LuaTable,
    index: usize,
) -> LuaResult<LuaUiFragmentDefinition> {
    let id: String = table
        .get("id")
        .map_err(|_| parse_error(format!("#{index}"), "id", "expected string id"))?;

    let labels = parse_string_array(table, "labels", &id)?;
    let tags = parse_tags(table, &id)?;
    let order = table.get::<Option<i32>>("order")?.unwrap_or(0);
    let context = parse_context_spec(table, &id)?;
    let source = parse_source(table, index)?;
    let render: LuaFunction = table
        .get("render")
        .map_err(|_| parse_error(id.clone(), "render", "expected render function"))?;

    Ok(LuaUiFragmentDefinition {
        meta: UiFragmentMeta {
            id,
            labels,
            tags,
            order,
            context,
            source,
        },
        render: lua.create_registry_value(render)?,
    })
}

fn parse_tags(
    table: &LuaTable,
    fragment_id: &str,
) -> LuaResult<std::collections::BTreeMap<String, String>> {
    let Some(tags): Option<LuaTable> = table.get("tags")? else {
        return Ok(Default::default());
    };
    let mut parsed = std::collections::BTreeMap::new();
    for pair in tags.pairs::<String, String>() {
        let (key, value) = pair.map_err(|_| {
            parse_error(
                fragment_id.to_string(),
                "tags",
                "expected string-to-string tag table",
            )
        })?;
        if parsed.insert(key.clone(), value).is_some() {
            return Err(parse_error(
                fragment_id.to_string(),
                format!("tags.{key}"),
                "duplicate tag key",
            ));
        }
    }
    Ok(parsed)
}

fn parse_context_spec(table: &LuaTable, id: &str) -> LuaResult<UiFragmentContextSpec> {
    let Some(context): Option<LuaTable> = table.get("context")? else {
        return Ok(UiFragmentContextSpec::default());
    };

    Ok(UiFragmentContextSpec {
        requires: parse_context_bindings(&context, "requires", id)?,
        optional: parse_context_bindings(&context, "optional", id)?,
    })
}

fn parse_context_bindings(
    table: &LuaTable,
    field: &'static str,
    fragment_id: &str,
) -> LuaResult<Vec<UiContextBinding>> {
    let Some(values): Option<LuaTable> = table.get(field)? else {
        return Ok(Vec::new());
    };

    let mut parsed = Vec::new();
    let mut seen = BTreeSet::new();

    for index in 1..=values.len()? {
        let key = values.get::<String>(index).map_err(|_| {
            parse_error(
                fragment_id.to_string(),
                format!("{field}[{index}]"),
                "expected string context key",
            )
        })?;
        if !seen.insert(key.clone()) {
            return Err(parse_error(
                fragment_id.to_string(),
                format!("{field}.{key}"),
                "duplicate context key",
            ));
        }
        parsed.push(UiContextBinding::untyped(key));
    }

    let mut keyed = Vec::new();
    for pair in values.pairs::<LuaValue, LuaValue>() {
        let (key, value) = pair?;
        let LuaValue::String(key) = key else {
            continue;
        };
        let key = key.to_str()?.to_string();
        let LuaValue::String(value_type) = value else {
            return Err(parse_error(
                fragment_id.to_string(),
                format!("{field}.{key}"),
                "expected string context value type",
            ));
        };
        let tag = value_type.to_str()?;
        let Some(value_type) = UiContextValueType::from_lua_tag(&tag) else {
            return Err(parse_error(
                fragment_id.to_string(),
                format!("{field}.{key}"),
                format!("unknown context value type '{tag}'"),
            ));
        };
        keyed.push((key, value_type));
    }

    keyed.sort_by(|(left, _), (right, _)| left.cmp(right));
    for (key, value_type) in keyed {
        if !seen.insert(key.clone()) {
            return Err(parse_error(
                fragment_id.to_string(),
                format!("{field}.{key}"),
                "duplicate context key",
            ));
        }
        parsed.push(UiContextBinding::typed(key, value_type));
    }

    Ok(parsed)
}

fn parse_string_array(
    table: &LuaTable,
    field: &'static str,
    fragment_id: &str,
) -> LuaResult<Vec<UiContextKey>> {
    let Some(values): Option<LuaTable> = table.get(field)? else {
        return Ok(Vec::new());
    };

    let mut parsed = Vec::with_capacity(values.len()? as usize);
    for index in 1..=values.len()? {
        let value = values.get::<String>(index).map_err(|_| {
            parse_error(
                fragment_id.to_string(),
                format!("{field}[{index}]"),
                "expected string",
            )
        })?;
        parsed.push(value);
    }
    Ok(parsed)
}

fn parse_source(table: &LuaTable, fallback_order: usize) -> LuaResult<Option<UiFragmentSource>> {
    let Some(source): Option<LuaTable> = table.get(UI_FRAGMENT_SOURCE_FIELD)? else {
        let registration_order = table
            .get::<Option<usize>>("_registration_order")?
            .unwrap_or(fallback_order);
        return Ok(Some(UiFragmentSource {
            source: None,
            short_src: None,
            line: None,
            registration_order,
        }));
    };

    Ok(Some(UiFragmentSource {
        source: source.get("source")?,
        short_src: source.get("short_src")?,
        line: source.get("line")?,
        registration_order: source
            .get::<Option<usize>>("registration_order")?
            .unwrap_or(fallback_order),
    }))
}

pub fn parse_ui_node(table: &LuaTable) -> LuaResult<UiNode> {
    parse_ui_node_at(table, "$", 0)
}

fn parse_ui_node_at(table: &LuaTable, path: &str, depth: usize) -> LuaResult<UiNode> {
    const MAX_DESCRIPTOR_DEPTH: usize = 64;
    if depth > MAX_DESCRIPTOR_DEPTH {
        return Err(parse_error(
            "<descriptor>",
            path,
            "descriptor tree is too deep or cyclic",
        ));
    }

    let kind: String = table
        .get("_ui_node")
        .map_err(|_| parse_error("<descriptor>", path, "missing _ui_node"))?;

    match kind.as_str() {
        "section" => Ok(UiNode::Section {
            title: table.get("title")?,
            children: parse_children(table, path, depth)?,
        }),
        "vstack" => Ok(UiNode::VStack {
            align_items: parse_align_items(table, path)?,
            justify_content: parse_justify_content(table, path)?,
            children: parse_children(table, path, depth)?,
        }),
        "hstack" => Ok(UiNode::HStack {
            align_items: parse_align_items(table, path)?,
            justify_content: parse_justify_content(table, path)?,
            children: parse_children(table, path, depth)?,
        }),
        "grid" => {
            let columns = table.get::<Option<usize>>("columns")?.unwrap_or(1);
            if columns == 0 {
                return Err(parse_error(
                    "<descriptor>",
                    format!("{path}.columns"),
                    "grid columns must be greater than zero",
                ));
            }
            Ok(UiNode::Grid {
                columns,
                children: parse_children(table, path, depth)?,
            })
        }
        "row" => Ok(UiNode::Row {
            align_items: parse_align_items(table, path)?,
            justify_content: parse_justify_content(table, path)?,
            children: parse_children(table, path, depth)?,
        }),
        "text" => Ok(UiNode::Text {
            value: table.get("value").map_err(|_| {
                parse_error("<descriptor>", format!("{path}.value"), "expected string")
            })?,
        }),
        "progress" => {
            let value = table.get::<f32>("value").map_err(|_| {
                parse_error("<descriptor>", format!("{path}.value"), "expected number")
            })?;
            if !value.is_finite() {
                return Err(parse_error(
                    "<descriptor>",
                    format!("{path}.value"),
                    "expected finite number",
                ));
            }
            Ok(UiNode::Progress { value })
        }
        "tabs" => Ok(UiNode::Tabs {
            tabs: parse_tabs(table, path)?,
        }),
        "tooltip" => {
            let content: LuaTable = table.get("content").map_err(|_| {
                parse_error(
                    "<descriptor>",
                    format!("{path}.content"),
                    "expected content node table",
                )
            })?;
            Ok(UiNode::Tooltip {
                content: Box::new(parse_ui_node_at(
                    &content,
                    &format!("{path}.content"),
                    depth + 1,
                )?),
                tooltip: parse_tooltip_nodes(table, path, depth)?,
            })
        }
        "modified_value" => Ok(UiNode::ModifiedValue {
            label: table.get("label").map_err(|_| {
                parse_error("<descriptor>", format!("{path}.label"), "expected string")
            })?,
            base: table.get("base").map_err(|_| {
                parse_error("<descriptor>", format!("{path}.base"), "expected string")
            })?,
            final_value: table
                .get("final")
                .or_else(|_| table.get("final_value"))
                .map_err(|_| {
                    parse_error(
                        "<descriptor>",
                        format!("{path}.final"),
                        "expected final value string",
                    )
                })?,
            modifiers: parse_modifier_display_lines(table, path)?,
        }),
        "button" => Ok(UiNode::Button {
            label: table.get("label").map_err(|_| {
                parse_error("<descriptor>", format!("{path}.label"), "expected string")
            })?,
            command: table.get("command")?,
            secondary_command: table.get("secondary_command")?,
            secondary_shift_command: table.get("secondary_shift_command")?,
            full_width: table.get::<Option<bool>>("full_width")?.unwrap_or(false),
            disabled: table.get::<Option<bool>>("disabled")?.unwrap_or(false),
            disabled_when: parse_optional_condition_display(table, "disabled_when", path)?,
        }),
        "action" => Ok(UiNode::Action {
            label: table.get("label").map_err(|_| {
                parse_error("<descriptor>", format!("{path}.label"), "expected string")
            })?,
            command: table.get("command").map_err(|_| {
                parse_error("<descriptor>", format!("{path}.command"), "expected string")
            })?,
            secondary_command: table.get("secondary_command")?,
            secondary_shift_command: table.get("secondary_shift_command")?,
            full_width: table.get::<Option<bool>>("full_width")?.unwrap_or(false),
            disabled: table.get::<Option<bool>>("disabled")?.unwrap_or(false),
            disabled_when: parse_optional_condition_display(table, "disabled_when", path)?,
        }),
        _ => Err(parse_error(
            "<descriptor>",
            path,
            format!("unknown ui node kind '{kind}'"),
        )),
    }
}

fn parse_align_items(table: &LuaTable, path: &str) -> LuaResult<crate::UiAlignItems> {
    match table.get::<Option<String>>("align_items")?.as_deref() {
        None | Some("start") => Ok(crate::UiAlignItems::Start),
        Some("center") => Ok(crate::UiAlignItems::Center),
        Some("end") => Ok(crate::UiAlignItems::End),
        Some(_) => Err(parse_error(
            "<descriptor>",
            format!("{path}.align_items"),
            "expected start, center, or end",
        )),
    }
}

fn parse_justify_content(table: &LuaTable, path: &str) -> LuaResult<crate::UiJustifyContent> {
    match table.get::<Option<String>>("justify_content")?.as_deref() {
        None | Some("start") => Ok(crate::UiJustifyContent::Start),
        Some("center") => Ok(crate::UiJustifyContent::Center),
        Some("end") => Ok(crate::UiJustifyContent::End),
        Some(_) => Err(parse_error(
            "<descriptor>",
            format!("{path}.justify_content"),
            "expected start, center, or end",
        )),
    }
}

fn parse_tabs(table: &LuaTable, path: &str) -> LuaResult<Vec<crate::UiTabItem>> {
    let tabs_table: LuaTable = table
        .get("tabs")
        .or_else(|_| table.get("children"))
        .map_err(|_| parse_error("<descriptor>", format!("{path}.tabs"), "expected tab list"))?;
    let mut tabs = Vec::new();
    for pair in tabs_table.sequence_values::<LuaTable>() {
        let item = pair?;
        tabs.push(crate::UiTabItem {
            label: item.get("label").map_err(|_| {
                parse_error(
                    "<descriptor>",
                    format!("{path}.tabs[].label"),
                    "expected string",
                )
            })?,
            command: item.get("command").map_err(|_| {
                parse_error(
                    "<descriptor>",
                    format!("{path}.tabs[].command"),
                    "expected string",
                )
            })?,
            selected: item.get::<Option<bool>>("selected")?.unwrap_or(false),
            disabled: item.get::<Option<bool>>("disabled")?.unwrap_or(false),
        });
    }
    Ok(tabs)
}

fn parse_optional_condition_display(
    table: &LuaTable,
    field: &'static str,
    path: &str,
) -> LuaResult<Option<UiConditionDisplay>> {
    let Some(condition): Option<LuaTable> = table.get(field)? else {
        return Ok(None);
    };
    Ok(Some(parse_condition_display(
        &condition,
        &format!("{path}.{field}"),
    )?))
}

fn parse_condition_display(table: &LuaTable, path: &str) -> LuaResult<UiConditionDisplay> {
    let children = if let Some(children) = table.get::<Option<LuaTable>>("children")? {
        let mut parsed = Vec::with_capacity(children.len()? as usize);
        for index in 1..=children.len()? {
            let child: LuaTable = children.get(index).map_err(|_| {
                parse_error(
                    "<descriptor>",
                    format!("{path}.children[{index}]"),
                    "expected condition child table",
                )
            })?;
            parsed.push(parse_condition_display(
                &child,
                &format!("{path}.children[{index}]"),
            )?);
        }
        parsed
    } else {
        Vec::new()
    };

    Ok(UiConditionDisplay {
        label: table
            .get("label")
            .map_err(|_| parse_error("<descriptor>", format!("{path}.label"), "expected string"))?,
        satisfied: table.get("satisfied").map_err(|_| {
            parse_error(
                "<descriptor>",
                format!("{path}.satisfied"),
                "expected boolean",
            )
        })?,
        operator: parse_condition_operator(table, path, children.is_empty())?,
        children,
    })
}

fn parse_condition_operator(
    table: &LuaTable,
    path: &str,
    is_leaf: bool,
) -> LuaResult<UiConditionOperator> {
    let Some(op): Option<String> = table.get("op")? else {
        return Ok(if is_leaf {
            UiConditionOperator::Leaf
        } else {
            UiConditionOperator::Group
        });
    };

    match op.as_str() {
        "leaf" => Ok(UiConditionOperator::Leaf),
        "all" => Ok(UiConditionOperator::All),
        "any" => Ok(UiConditionOperator::Any),
        "not" => Ok(UiConditionOperator::Not),
        "group" => Ok(UiConditionOperator::Group),
        _ => Err(parse_error(
            "<descriptor>",
            format!("{path}.op"),
            format!("unknown condition operator '{op}'"),
        )),
    }
}

fn parse_tooltip_nodes(table: &LuaTable, path: &str, depth: usize) -> LuaResult<Vec<UiNode>> {
    let tooltip: LuaTable = table.get("tooltip").map_err(|_| {
        parse_error(
            "<descriptor>",
            format!("{path}.tooltip"),
            "expected tooltip node table or array",
        )
    })?;

    if tooltip.contains_key("_ui_node")? {
        return Ok(vec![parse_ui_node_at(
            &tooltip,
            &format!("{path}.tooltip"),
            depth + 1,
        )?]);
    }

    let mut parsed = Vec::with_capacity(tooltip.len()? as usize);
    for index in 1..=tooltip.len()? {
        let child: LuaTable = tooltip.get(index).map_err(|_| {
            parse_error(
                "<descriptor>",
                format!("{path}.tooltip[{index}]"),
                "expected tooltip child node table",
            )
        })?;
        parsed.push(parse_ui_node_at(
            &child,
            &format!("{path}.tooltip[{index}]"),
            depth + 1,
        )?);
    }
    Ok(parsed)
}

fn parse_modifier_display_lines(
    table: &LuaTable,
    path: &str,
) -> LuaResult<Vec<UiModifierDisplayLine>> {
    let Some(modifiers): Option<LuaTable> = table.get("modifiers")? else {
        return Ok(Vec::new());
    };

    let mut parsed = Vec::with_capacity(modifiers.len()? as usize);
    for index in 1..=modifiers.len()? {
        let modifier: LuaTable = modifiers.get(index).map_err(|_| {
            parse_error(
                "<descriptor>",
                format!("{path}.modifiers[{index}]"),
                "expected modifier table",
            )
        })?;
        parsed.push(UiModifierDisplayLine {
            label: modifier.get("label").map_err(|_| {
                parse_error(
                    "<descriptor>",
                    format!("{path}.modifiers[{index}].label"),
                    "expected string",
                )
            })?,
            parts: parse_string_array(&modifier, "parts", &format!("{path}.modifiers[{index}]"))?,
            remaining_duration: modifier.get("remaining_duration")?,
        });
    }
    Ok(parsed)
}

fn parse_children(table: &LuaTable, path: &str, depth: usize) -> LuaResult<Vec<UiNode>> {
    let Some(children): Option<LuaTable> = table.get("children")? else {
        return Ok(Vec::new());
    };

    let mut parsed = Vec::with_capacity(children.len()? as usize);
    for index in 1..=children.len()? {
        let child: LuaTable = children.get(index).map_err(|_| {
            parse_error(
                "<descriptor>",
                format!("{path}.children[{index}]"),
                "expected child node table",
            )
        })?;
        parsed.push(parse_ui_node_at(
            &child,
            &format!("{path}.children[{index}]"),
            depth + 1,
        )?);
    }
    Ok(parsed)
}

fn parse_error(
    fragment_id: impl Into<String>,
    field: impl Into<String>,
    message: impl Into<String>,
) -> LuaError {
    LuaError::external(UiFragmentParseError::new(fragment_id, field, message))
}

#[derive(Debug)]
struct UiFragmentParseError {
    fragment_id: String,
    field: String,
    message: String,
}

impl UiFragmentParseError {
    fn new(
        fragment_id: impl Into<String>,
        field: impl Into<String>,
        message: impl Into<String>,
    ) -> Self {
        Self {
            fragment_id: fragment_id.into(),
            field: field.into(),
            message: message.into(),
        }
    }
}

impl fmt::Display for UiFragmentParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "ui fragment '{}' field '{}': {}",
            self.fragment_id, self.field, self.message
        )
    }
}

impl Error for UiFragmentParseError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ui_primitive_helpers_return_descriptor_tables() {
        let lua = Lua::new();
        register_ui_dsl_helpers(&lua).expect("register helpers");

        let node: LuaTable = lua
            .load(
                r#"
                return ui.section {
                    title = "Overview",
                    children = {
                        ui.vstack {
                            children = {
                                ui.hstack { children = { ui.text("A"), ui.progress(0.5) } },
                                ui.grid { columns = 2, children = { ui.text("K"), ui.text("V") } },
                            },
                        },
                        ui.button { label = "Open", command = "ui.open" },
                        ui.action { label = "Go", command = "ship.move" },
                        ui.tabs {
                            tabs = {
                                { label = "Overview", command = "tab.overview", selected = true },
                            },
                        },
                    },
                }
                "#,
            )
            .eval()
            .expect("build descriptor");

        assert_eq!(node.get::<String>("_ui_node").unwrap(), "section");
        let children: LuaTable = node.get("children").unwrap();
        let stack: LuaTable = children.get(1).unwrap();
        assert_eq!(stack.get::<String>("_ui_node").unwrap(), "vstack");
        let button: LuaTable = children.get(2).unwrap();
        assert_eq!(button.get::<String>("_ui_node").unwrap(), "button");
        let tabs: LuaTable = children.get(4).unwrap();
        assert_eq!(tabs.get::<String>("_ui_node").unwrap(), "tabs");
    }

    #[test]
    fn parses_lua_descriptor_table_into_ui_node() {
        let lua = Lua::new();
        register_ui_dsl_helpers(&lua).expect("register helpers");

        let descriptor: LuaTable = lua
            .load(
                r#"
                return ui.section {
                    title = "Overview",
                    children = {
                        ui.hstack {
                            align_items = "center",
                            justify_content = "start",
                            children = { ui.text("A"), ui.progress(0.25) },
                        },
                        ui.tabs {
                            tabs = {
                                { label = "Overview", command = "tab.overview", selected = true },
                                { label = "Details", command = "tab.details", disabled = true },
                            },
                        },
                        ui.button { label = "Open", command = "ui.open" },
                    },
                }
                "#,
            )
            .eval()
            .expect("build descriptor");

        assert_eq!(
            parse_ui_node(&descriptor).expect("parse descriptor"),
            UiNode::Section {
                title: Some("Overview".to_string()),
                children: vec![
                    UiNode::HStack {
                        align_items: crate::UiAlignItems::Center,
                        justify_content: crate::UiJustifyContent::Start,
                        children: vec![
                            UiNode::Text {
                                value: "A".to_string()
                            },
                            UiNode::Progress { value: 0.25 },
                        ],
                    },
                    UiNode::Tabs {
                        tabs: vec![
                            crate::UiTabItem {
                                label: "Overview".to_string(),
                                command: "tab.overview".to_string(),
                                selected: true,
                                disabled: false,
                            },
                            crate::UiTabItem {
                                label: "Details".to_string(),
                                command: "tab.details".to_string(),
                                selected: false,
                                disabled: true,
                            },
                        ],
                    },
                    UiNode::Button {
                        label: "Open".to_string(),
                        command: Some("ui.open".to_string()),
                        secondary_command: None,
                        secondary_shift_command: None,
                        full_width: false,
                        disabled: false,
                        disabled_when: None,
                    },
                ],
            }
        );
    }

    #[test]
    fn parses_modified_value_descriptor_with_modifier_tooltip_lines() {
        let lua = Lua::new();
        register_ui_dsl_helpers(&lua).expect("register helpers");

        let descriptor: LuaTable = lua
            .load(
                r#"
                return ui.modified_value {
                    label = "Range",
                    base = "10",
                    final = "19",
                    modifiers = {
                        {
                            label = "Tech A",
                            parts = { "+2 (base add)", "x1.5 (mult)", "+1 (add)" },
                            remaining_duration = 15,
                        },
                    },
                }
                "#,
            )
            .eval()
            .expect("build descriptor");

        assert_eq!(
            parse_ui_node(&descriptor).expect("parse descriptor"),
            UiNode::ModifiedValue {
                label: "Range".to_string(),
                base: "10".to_string(),
                final_value: "19".to_string(),
                modifiers: vec![UiModifierDisplayLine {
                    label: "Tech A".to_string(),
                    parts: vec![
                        "+2 (base add)".to_string(),
                        "x1.5 (mult)".to_string(),
                        "+1 (add)".to_string(),
                    ],
                    remaining_duration: Some(15),
                }],
            }
        );
    }

    #[test]
    fn parses_generic_tooltip_descriptor() {
        let lua = Lua::new();
        register_ui_dsl_helpers(&lua).expect("register helpers");

        let descriptor: LuaTable = lua
            .load(
                r#"
                return ui.tooltip {
                    content = ui.text("Status"),
                    tooltip = {
                        ui.text("Condition A: true"),
                        ui.text("Condition B: false"),
                    },
                }
                "#,
            )
            .eval()
            .expect("build descriptor");

        assert_eq!(
            parse_ui_node(&descriptor).expect("parse descriptor"),
            UiNode::Tooltip {
                content: Box::new(UiNode::Text {
                    value: "Status".to_string(),
                }),
                tooltip: vec![
                    UiNode::Text {
                        value: "Condition A: true".to_string(),
                    },
                    UiNode::Text {
                        value: "Condition B: false".to_string(),
                    },
                ],
            }
        );
    }

    #[test]
    fn parses_disabled_button_condition_display() {
        let lua = Lua::new();
        register_ui_dsl_helpers(&lua).expect("register helpers");

        let descriptor: LuaTable = lua
            .load(
                r#"
                return ui.button {
                    label = "Build",
                    command = "colony.build",
                    disabled = true,
                    disabled_when = {
                        label = "Can build mine",
                        satisfied = false,
                        children = {
                            { label = "Has colony", satisfied = true },
                            { label = "Enough minerals", satisfied = false },
                        },
                    },
                }
                "#,
            )
            .eval()
            .expect("build descriptor");

        assert_eq!(
            parse_ui_node(&descriptor).expect("parse descriptor"),
            UiNode::Button {
                label: "Build".to_string(),
                command: Some("colony.build".to_string()),
                secondary_command: None,
                secondary_shift_command: None,
                full_width: false,
                disabled: true,
                disabled_when: Some(UiConditionDisplay {
                    label: "Can build mine".to_string(),
                    satisfied: false,
                    operator: UiConditionOperator::Group,
                    children: vec![
                        UiConditionDisplay {
                            label: "Has colony".to_string(),
                            satisfied: true,
                            operator: UiConditionOperator::Leaf,
                            children: Vec::new(),
                        },
                        UiConditionDisplay {
                            label: "Enough minerals".to_string(),
                            satisfied: false,
                            operator: UiConditionOperator::Leaf,
                            children: Vec::new(),
                        },
                    ],
                }),
            }
        );
    }

    #[test]
    fn descriptor_parser_rejects_unknown_node_kind() {
        let lua = Lua::new();
        let descriptor = lua.create_table().expect("table");
        descriptor.set("_ui_node", "mystery").expect("kind");

        let err = match parse_ui_node(&descriptor) {
            Ok(_) => panic!("unknown node kind should fail"),
            Err(err) => err,
        };
        assert!(err.to_string().contains("unknown ui node kind"));
    }

    #[test]
    fn inflates_lua_fragment_to_typed_descriptor() {
        let lua = Lua::new();
        register_ui_fragment_definition_accumulator(&lua).expect("register accumulator");
        register_ui_dsl_helpers(&lua).expect("register helpers");

        lua.load(
            r#"
            define_ui_fragment {
                id = "inspectable",
                render = function(view)
                    return ui.text("Inspectable")
                end,
            }
            "#,
        )
        .exec()
        .expect("define fragment");

        let registry = parse_ui_fragment_definitions(&lua).expect("parse fragments");
        let fragment = registry.iter().next().expect("fragment");
        let view = lua.create_table().expect("view");

        assert_eq!(
            fragment.inflate(&lua, view).expect("inflate"),
            UiNode::Text {
                value: "Inspectable".to_string()
            }
        );
    }

    #[test]
    fn dynamic_frame_render_reflects_view_changes() {
        let lua = Lua::new();
        register_ui_fragment_definition_accumulator(&lua).expect("register accumulator");
        register_ui_dsl_helpers(&lua).expect("register helpers");

        lua.load(
            r#"
            define_ui_fragment {
                id = "dynamic",
                render = function(view)
                    return ui.section {
                        title = "Frame " .. tostring(view.tick),
                        children = {
                            ui.text(view.label),
                            ui.progress(view.progress),
                        },
                    }
                end,
            }
            "#,
        )
        .exec()
        .expect("define fragment");

        let registry = parse_ui_fragment_definitions(&lua).expect("parse fragments");
        let render_frame = |tick: i64, label: &str, progress: f64| {
            render_lua_fragment_frame(&lua, &registry, |_, lua| {
                let view = lua.create_table()?;
                view.set("tick", tick)?;
                view.set("label", label)?;
                view.set("progress", progress)?;
                Ok(view)
            })
            .expect("render frame")
            .pop()
            .expect("frame")
            .node
        };

        let first = render_frame(1, "one", 0.1);
        let second = render_frame(2, "two", 0.8);

        assert_ne!(first, second);
        assert_eq!(
            first,
            UiNode::Section {
                title: Some("Frame 1".to_string()),
                children: vec![
                    UiNode::Text {
                        value: "one".to_string()
                    },
                    UiNode::Progress { value: 0.1 },
                ],
            }
        );
        assert_eq!(
            second,
            UiNode::Section {
                title: Some("Frame 2".to_string()),
                children: vec![
                    UiNode::Text {
                        value: "two".to_string()
                    },
                    UiNode::Progress { value: 0.8 },
                ],
            }
        );
    }

    #[test]
    fn dynamic_frame_render_reports_invalid_frame_descriptor() {
        let lua = Lua::new();
        register_ui_fragment_definition_accumulator(&lua).expect("register accumulator");
        register_ui_dsl_helpers(&lua).expect("register helpers");

        lua.load(
            r#"
            define_ui_fragment {
                id = "sometimes_bad",
                render = function(view)
                    if view.bad then
                        return { _ui_node = "unknown_runtime_node" }
                    end
                    return ui.text("ok")
                end,
            }
            "#,
        )
        .exec()
        .expect("define fragment");

        let registry = parse_ui_fragment_definitions(&lua).expect("parse fragments");
        let err = match render_lua_fragment_frame(&lua, &registry, |_, lua| {
            let view = lua.create_table()?;
            view.set("bad", true)?;
            Ok(view)
        }) {
            Ok(_) => panic!("invalid frame descriptor should fail"),
            Err(err) => err,
        };

        assert!(err.to_string().contains("unknown ui node kind"));
    }

    #[test]
    fn parses_define_ui_fragment_accumulator_in_registry_order_with_source() {
        let lua = Lua::new();
        register_ui_fragment_definition_accumulator(&lua).expect("register accumulator");
        register_ui_dsl_helpers(&lua).expect("register helpers");

        lua.load(
            r#"
            define_ui_fragment {
                id = "late",
                labels = { "esc", "late" },
                tags = { esc_tab = "construction_overview" },
                order = 20,
                context = { requires = { "empire" }, optional = { "system" } },
                render = function(view) return ui.text("late") end,
            }
            define_ui_fragment {
                id = "early",
                labels = { "esc", "early" },
                order = 10,
                render = function(view) return ui.text("early") end,
            }
            "#,
        )
        .set_name("scripts/ui/init.lua")
        .exec()
        .expect("define fragments");

        let registry = parse_ui_fragment_definitions(&lua).expect("parse fragments");
        let ids: Vec<&str> = registry
            .iter()
            .map(|definition| definition.meta.id.as_str())
            .collect();
        assert_eq!(ids, vec!["early", "late"]);

        let late = registry
            .iter()
            .find(|definition| definition.meta.id == "late")
            .expect("late fragment");
        assert_eq!(late.meta.labels, vec!["esc", "late"]);
        assert_eq!(
            late.meta.tags.get("esc_tab").map(String::as_str),
            Some("construction_overview")
        );
        assert_eq!(
            registry
                .get_by_tag("esc_tab", "construction_overview")
                .map(|definition| definition.meta.id.as_str()),
            Some("late")
        );
        assert_eq!(
            late.meta.context.requires,
            vec![UiContextBinding::untyped("empire")]
        );
        assert_eq!(
            late.meta.context.optional,
            vec![UiContextBinding::untyped("system")]
        );
        let source = late.meta.source.as_ref().expect("source metadata");
        assert!(
            source
                .short_src
                .as_deref()
                .is_some_and(|short_src| short_src.contains("scripts/ui/init.lua"))
        );
        assert_eq!(source.registration_order, 1);
    }

    #[test]
    fn context_menu_fragment_index_matches_static_facets() {
        let lua = Lua::new();
        register_ui_fragment_definition_accumulator(&lua).expect("register accumulator");
        register_ui_dsl_helpers(&lua).expect("register helpers");

        lua.load(
            r#"
            define_ui_fragment {
                id = "ship_move",
                tags = {
                    part_of = "context_menu",
                    target = "entity:system",
                    ["ctx:selected"] = "entity:ship",
                    command = "ship.move",
                },
                render = function(view) return ui.text("move") end,
            }
            define_ui_fragment {
                id = "ship_survey",
                tags = {
                    part_of = "context_menu",
                    target = "entity:system",
                    ["ctx:selected"] = "entity:ship",
                    ["ctx:selected:ship:class"] = "surveyor",
                    command = "ship.survey",
                },
                render = function(view) return ui.text("survey") end,
            }
            define_ui_fragment {
                id = "ship_colonize",
                tags = {
                    part_of = "context_menu",
                    target = "entity:system",
                    ["ctx:selected"] = "entity:ship",
                    ["ctx:selected:ship:class"] = "colonizer",
                    command = "ship.colonize",
                },
                render = function(view) return ui.text("colonize") end,
            }
            define_ui_fragment {
                id = "unrelated",
                tags = { part_of = "esc" },
                render = function(view) return ui.text("other") end,
            }
            "#,
        )
        .exec()
        .expect("define fragments");

        let registry = parse_ui_fragment_definitions(&lua).expect("parse fragments");

        let base_query = UiFacetQuery::new([
            ("part_of", "context_menu"),
            ("target", "entity:system"),
            ("ctx:selected", "entity:ship"),
        ]);
        let base_ids: Vec<&str> = registry
            .context_menu_fragments(&base_query)
            .into_iter()
            .map(|definition| definition.meta.id.as_str())
            .collect();
        assert_eq!(base_ids, vec!["ship_move"]);

        let survey_query = UiFacetQuery::new([
            ("part_of", "context_menu"),
            ("target", "entity:system"),
            ("ctx:selected", "entity:ship"),
            ("ctx:selected:ship:class", "surveyor"),
        ]);
        let survey_ids: Vec<&str> = registry
            .context_menu_fragments(&survey_query)
            .into_iter()
            .map(|definition| definition.meta.id.as_str())
            .collect();
        assert_eq!(survey_ids, vec!["ship_move", "ship_survey"]);

        let colonize_query = UiFacetQuery::new([
            ("part_of", "context_menu"),
            ("target", "entity:system"),
            ("ctx:selected", "entity:ship"),
            ("ctx:selected:ship:class", "colonizer"),
        ]);
        let colonize_ids: Vec<&str> = registry
            .context_menu_fragments(&colonize_query)
            .into_iter()
            .map(|definition| definition.meta.id.as_str())
            .collect();
        assert_eq!(colonize_ids, vec!["ship_colonize", "ship_move"]);
    }

    #[test]
    fn context_menu_fragment_index_rejects_invalid_static_facets() {
        let lua = Lua::new();
        register_ui_fragment_definition_accumulator(&lua).expect("register accumulator");
        register_ui_dsl_helpers(&lua).expect("register helpers");

        lua.load(
            r#"
            define_ui_fragment {
                id = "bad_target",
                tags = {
                    part_of = "context_menu",
                    target = "runtime:anything",
                },
                render = function(view) return ui.text("bad") end,
            }
            "#,
        )
        .exec()
        .expect("define fragment");

        let err = match parse_ui_fragment_definitions(&lua) {
            Ok(_) => panic!("invalid context menu facet should fail"),
            Err(err) => err,
        };
        assert!(
            err.to_string()
                .contains("unsupported context menu facet value")
        );
    }

    #[test]
    fn parses_typed_context_bindings() {
        let lua = Lua::new();
        register_ui_fragment_definition_accumulator(&lua).expect("register accumulator");
        register_ui_dsl_helpers(&lua).expect("register helpers");

        lua.load(
            r#"
            define_ui_fragment {
                id = "typed",
                context = {
                    requires = {
                        colony = "entity",
                        build_queue = "view",
                    },
                    optional = {
                        selected_tab = "state",
                        filters = "strings",
                    },
                },
                render = function(view) return ui.text("typed") end,
            }
            "#,
        )
        .exec()
        .expect("define fragment");

        let registry = parse_ui_fragment_definitions(&lua).expect("parse fragments");
        let fragment = registry.iter().next().expect("fragment");
        assert_eq!(
            fragment.meta.context.requires,
            vec![
                UiContextBinding::typed("build_queue", UiContextValueType::ViewRef),
                UiContextBinding::typed("colony", UiContextValueType::Entity),
            ]
        );
        assert_eq!(
            fragment.meta.context.optional,
            vec![
                UiContextBinding::typed("filters", UiContextValueType::StringList),
                UiContextBinding::typed("selected_tab", UiContextValueType::StateRef),
            ]
        );
    }

    #[test]
    fn parsing_rejects_unknown_context_value_type() {
        let lua = Lua::new();
        register_ui_fragment_definition_accumulator(&lua).expect("register accumulator");
        register_ui_dsl_helpers(&lua).expect("register helpers");

        lua.load(
            r#"
            define_ui_fragment {
                id = "bad_context",
                context = { requires = { colony = "planetish" } },
                render = function(view) return ui.text("bad") end,
            }
            "#,
        )
        .exec()
        .expect("define fragment");

        let err = match parse_ui_fragment_definitions(&lua) {
            Ok(_) => panic!("unknown context type should fail"),
            Err(err) => err,
        };
        assert!(err.to_string().contains("unknown context value type"));
    }

    #[test]
    fn parsing_rejects_duplicate_fragment_ids() {
        let lua = Lua::new();
        register_ui_fragment_definition_accumulator(&lua).expect("register accumulator");
        register_ui_dsl_helpers(&lua).expect("register helpers");

        lua.load(
            r#"
            define_ui_fragment { id = "dup", render = function(view) return ui.text("a") end }
            define_ui_fragment { id = "dup", render = function(view) return ui.text("b") end }
            "#,
        )
        .exec()
        .expect("define fragments");

        let err = match parse_ui_fragment_definitions(&lua) {
            Ok(_) => panic!("duplicate ids fail"),
            Err(err) => err,
        };
        assert!(err.to_string().contains("duplicate ui fragment id"));
    }
}
