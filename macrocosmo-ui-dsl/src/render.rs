//! Minimal egui renderer for typed UI DSL descriptors.
//!
//! This is intentionally host-agnostic: it renders data-only `UiNode` trees and
//! returns clicked command ids for the host to validate and dispatch.

use crate::{UiAlignItems, UiConditionDisplay, UiConditionOperator, UiNode};

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct UiRenderStats {
    pub nodes: usize,
    pub sections: usize,
    pub text: usize,
    pub progress: usize,
    pub tabs: usize,
    pub tooltips: usize,
    pub modified_values: usize,
    pub buttons: usize,
    pub disabled_buttons: usize,
    pub commands: usize,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct UiRenderOutput {
    pub clicked_commands: Vec<String>,
    pub stats: UiRenderStats,
}

#[derive(Default)]
pub struct UiDslRenderer {
    next_id: usize,
}

impl UiDslRenderer {
    pub fn render(&mut self, ui: &mut egui::Ui, node: &UiNode) -> UiRenderOutput {
        let mut output = UiRenderOutput::default();
        ui.scope(|ui| {
            apply_visual_tuning(ui);
            self.render_node(ui, node, &mut output);
        });
        output
    }

    pub fn render_many<'a>(
        &mut self,
        ui: &mut egui::Ui,
        nodes: impl IntoIterator<Item = &'a UiNode>,
    ) -> UiRenderOutput {
        let mut output = UiRenderOutput::default();
        ui.scope(|ui| {
            apply_visual_tuning(ui);
            for node in nodes {
                self.render_node(ui, node, &mut output);
            }
        });
        output
    }

    fn render_node(
        &mut self,
        ui: &mut egui::Ui,
        node: &UiNode,
        output: &mut UiRenderOutput,
    ) -> Option<egui::Response> {
        output.stats.nodes += 1;

        match node {
            UiNode::Section { title, children } => {
                output.stats.sections += 1;
                let mut child_response = None;
                let inner = ui.vertical(|ui| {
                    if let Some(title) = title {
                        child_response = merge_response(
                            child_response.take(),
                            ui.label(egui::RichText::new(title).strong()),
                        );
                        if !children.is_empty() {
                            ui.add_space(2.0);
                        }
                    }
                    child_response = merge_optional_response(
                        child_response.take(),
                        render_children(self, ui, children, output),
                    );
                });
                merge_response(child_response, inner.response)
            }
            UiNode::VStack {
                align_items,
                justify_content: _,
                children,
            } => {
                let mut child_response = None;
                let inner = match align_items {
                    UiAlignItems::Center => ui.vertical_centered(|ui| {
                        child_response = render_children(self, ui, children, output);
                    }),
                    UiAlignItems::Start | UiAlignItems::End => ui.vertical(|ui| {
                        child_response = render_children(self, ui, children, output);
                    }),
                };
                merge_response(child_response, inner.response)
            }
            UiNode::HStack {
                align_items,
                justify_content: _,
                children,
            }
            | UiNode::Row {
                align_items,
                justify_content: _,
                children,
            } => {
                let mut child_response = None;
                let inner = match align_items {
                    UiAlignItems::Center => ui.horizontal_centered(|ui| {
                        child_response = render_children(self, ui, children, output);
                    }),
                    UiAlignItems::Start | UiAlignItems::End => ui.horizontal(|ui| {
                        child_response = render_children(self, ui, children, output);
                    }),
                };
                merge_response(child_response, inner.response)
            }
            UiNode::Grid { columns, children } => {
                let id = self.allocate_id("ui-dsl-grid");
                let mut child_response = None;
                let inner = egui::Grid::new(id)
                    .num_columns(*columns)
                    .spacing([12.0, 4.0])
                    .striped(true)
                    .show(ui, |ui| {
                        for (index, child) in children.iter().enumerate() {
                            child_response = merge_optional_response(
                                child_response.take(),
                                self.render_node(ui, child, output),
                            );
                            if (index + 1) % columns == 0 {
                                ui.end_row();
                            }
                        }
                    });
                merge_response(child_response, inner.response)
            }
            UiNode::Text { value } => {
                output.stats.text += 1;
                Some(ui.label(value))
            }
            UiNode::Progress { value } => {
                output.stats.progress += 1;
                Some(ui.add(egui::ProgressBar::new(value.clamp(0.0, 1.0)).show_percentage()))
            }
            UiNode::Tabs { tabs } => {
                output.stats.tabs += 1;
                let mut child_response = None;
                let inner = ui.horizontal(|ui| {
                    for tab in tabs {
                        output.stats.buttons += 1;
                        output.stats.commands += 1;
                        if tab.disabled {
                            output.stats.disabled_buttons += 1;
                        }
                        let response = ui.add_enabled(
                            !tab.disabled,
                            egui::Button::new(&tab.label).selected(tab.selected),
                        );
                        if !tab.disabled && response.clicked() {
                            output.clicked_commands.push(tab.command.clone());
                        }
                        child_response = merge_response(child_response.take(), response);
                    }
                });
                merge_response(child_response, inner.response)
            }
            UiNode::Tooltip { content, tooltip } => {
                output.stats.tooltips += 1;
                let response = self.render_node(ui, content, output)?;
                Some(response.on_hover_ui(|ui| {
                    let _ = render_children(self, ui, tooltip, output);
                }))
            }
            UiNode::ModifiedValue {
                label,
                base,
                final_value,
                modifiers,
            } => {
                output.stats.modified_values += 1;
                let response = ui.label(format!("{label}: {final_value}"));
                Some(response.on_hover_ui(|ui| {
                    ui.set_min_width(220.0);
                    ui.label(egui::RichText::new(format!("{label}: {final_value}")).strong());
                    ui.separator();
                    ui.label(format!("Base: {base}"));

                    if modifiers.is_empty() {
                        ui.label("(no modifiers)");
                    } else {
                        for modifier in modifiers {
                            let parts = if modifier.parts.is_empty() {
                                "(no numeric contribution)".to_string()
                            } else {
                                modifier.parts.join(", ")
                            };
                            let mut line = format!("[{}]  {}", modifier.label, parts);
                            if let Some(remaining) = modifier.remaining_duration {
                                line.push_str(&format!("  ({} hd left)", remaining));
                            }
                            ui.label(line);
                        }
                    }

                    ui.separator();
                    ui.label(format!("Final: {final_value}"));
                }))
            }
            UiNode::Button {
                label,
                command,
                secondary_command,
                secondary_shift_command,
                full_width,
                disabled,
                disabled_when,
            } => {
                output.stats.buttons += 1;
                if command.is_some() {
                    output.stats.commands += 1;
                }
                if secondary_command.is_some() {
                    output.stats.commands += 1;
                }
                if secondary_shift_command.is_some() {
                    output.stats.commands += 1;
                }
                if *disabled {
                    output.stats.disabled_buttons += 1;
                }
                let button = egui::Button::new(label);
                let response = if *full_width {
                    ui.add_enabled_ui(!disabled, |ui| {
                        ui.add_sized([ui.available_width(), 0.0], button)
                    })
                    .inner
                } else {
                    ui.add_enabled(!disabled, button)
                };
                let response = attach_disabled_when_tooltip(response, *disabled, disabled_when);
                if !disabled {
                    let command_clicked = response.clicked();
                    let shift_held = ui.input(|input| input.modifiers.shift);
                    let secondary_requested = response.secondary_clicked()
                        || (command_clicked && ui.input(|input| input.modifiers.command));
                    if secondary_requested {
                        if shift_held && let Some(command) = secondary_shift_command {
                            output.clicked_commands.push(command.clone());
                        } else if let Some(command) = secondary_command {
                            output.clicked_commands.push(command.clone());
                        } else if command_clicked && let Some(command) = command {
                            output.clicked_commands.push(command.clone());
                        }
                    } else if command_clicked && let Some(command) = command {
                        output.clicked_commands.push(command.clone());
                    }
                }
                Some(response)
            }
            UiNode::Action {
                label,
                command,
                secondary_command,
                secondary_shift_command,
                full_width,
                disabled,
                disabled_when,
            } => {
                output.stats.buttons += 1;
                output.stats.commands += 1;
                if secondary_command.is_some() {
                    output.stats.commands += 1;
                }
                if secondary_shift_command.is_some() {
                    output.stats.commands += 1;
                }
                if *disabled {
                    output.stats.disabled_buttons += 1;
                }
                let button = egui::Button::new(label);
                let response = if *full_width {
                    ui.add_enabled_ui(!disabled, |ui| {
                        ui.add_sized([ui.available_width(), 0.0], button)
                    })
                    .inner
                } else {
                    ui.add_enabled(!disabled, button)
                };
                let response = attach_disabled_when_tooltip(response, *disabled, disabled_when);
                if !disabled {
                    let command_clicked = response.clicked();
                    let shift_held = ui.input(|input| input.modifiers.shift);
                    let secondary_requested = response.secondary_clicked()
                        || (command_clicked && ui.input(|input| input.modifiers.command));
                    if secondary_requested {
                        if shift_held && let Some(command) = secondary_shift_command {
                            output.clicked_commands.push(command.clone());
                        } else if let Some(command) = secondary_command {
                            output.clicked_commands.push(command.clone());
                        } else if command_clicked {
                            output.clicked_commands.push(command.clone());
                        }
                    } else if command_clicked {
                        output.clicked_commands.push(command.clone());
                    }
                }
                Some(response)
            }
        }
    }

    fn allocate_id(&mut self, prefix: &'static str) -> egui::Id {
        let id = egui::Id::new((prefix, self.next_id));
        self.next_id += 1;
        id
    }
}

fn apply_visual_tuning(ui: &mut egui::Ui) {
    let style = ui.style_mut();
    style.spacing.item_spacing = egui::vec2(6.0, 4.0);
    style.spacing.button_padding = egui::vec2(6.0, 3.0);
    style.spacing.indent = 12.0;
}

fn render_children(
    renderer: &mut UiDslRenderer,
    ui: &mut egui::Ui,
    children: &[UiNode],
    output: &mut UiRenderOutput,
) -> Option<egui::Response> {
    let mut response = None;
    for child in children {
        response = merge_optional_response(response, renderer.render_node(ui, child, output));
    }
    response
}

fn merge_response(current: Option<egui::Response>, next: egui::Response) -> Option<egui::Response> {
    Some(match current {
        Some(current) => current.union(next),
        None => next,
    })
}

fn merge_optional_response(
    current: Option<egui::Response>,
    next: Option<egui::Response>,
) -> Option<egui::Response> {
    match next {
        Some(next) => merge_response(current, next),
        None => current,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lua::{
        parse_ui_fragment_definitions, register_ui_dsl_module,
        register_ui_fragment_definition_accumulator,
    };
    use mlua::Lua;

    #[test]
    fn renders_typed_descriptor_to_egui_without_dispatching_actions() {
        let node = UiNode::Section {
            title: Some("Overview".to_string()),
            children: vec![UiNode::VStack {
                align_items: crate::UiAlignItems::Start,
                justify_content: crate::UiJustifyContent::Start,
                children: vec![
                    UiNode::Text {
                        value: "Minerals".to_string(),
                    },
                    UiNode::Progress { value: 0.5 },
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
            }],
        };
        let ctx = egui::Context::default();
        let mut renderer = UiDslRenderer::default();
        let mut output = UiRenderOutput::default();

        let _ = ctx.run(Default::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                output = renderer.render(ui, &node);
            });
        });

        assert_eq!(output.clicked_commands, Vec::<String>::new());
        assert_eq!(
            output.stats,
            UiRenderStats {
                nodes: 5,
                sections: 1,
                text: 1,
                progress: 1,
                tabs: 0,
                tooltips: 0,
                modified_values: 0,
                buttons: 1,
                disabled_buttons: 0,
                commands: 1,
            }
        );
    }

    #[test]
    fn renders_tabs_as_command_buttons() {
        let node = UiNode::Tabs {
            tabs: vec![
                crate::UiTabItem {
                    label: "Overview".to_string(),
                    command: "tab.overview".to_string(),
                    selected: true,
                    disabled: false,
                },
                crate::UiTabItem {
                    label: "Locked".to_string(),
                    command: "tab.locked".to_string(),
                    selected: false,
                    disabled: true,
                },
            ],
        };
        let ctx = egui::Context::default();
        let mut renderer = UiDslRenderer::default();
        let mut output = UiRenderOutput::default();

        let _ = ctx.run(Default::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                output = renderer.render(ui, &node);
            });
        });

        assert_eq!(output.clicked_commands, Vec::<String>::new());
        assert_eq!(output.stats.tabs, 1);
        assert_eq!(output.stats.buttons, 2);
        assert_eq!(output.stats.commands, 2);
        assert_eq!(output.stats.disabled_buttons, 1);
    }

    #[test]
    fn renders_modified_value_with_tooltip_breakdown() {
        let node = UiNode::ModifiedValue {
            label: "Range".to_string(),
            base: "10".to_string(),
            final_value: "19".to_string(),
            modifiers: vec![crate::UiModifierDisplayLine {
                label: "Tech A".to_string(),
                parts: vec![
                    "+2 (base add)".to_string(),
                    "x1.5 (mult)".to_string(),
                    "+1 (add)".to_string(),
                ],
                remaining_duration: Some(15),
            }],
        };
        let ctx = egui::Context::default();
        let mut renderer = UiDslRenderer::default();
        let mut output = UiRenderOutput::default();

        let _ = ctx.run(Default::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                output = renderer.render(ui, &node);
            });
        });

        assert_eq!(output.stats.nodes, 1);
        assert_eq!(output.stats.modified_values, 1);
        assert_eq!(output.stats.commands, 0);
    }

    #[test]
    fn renders_generic_tooltip_wrapper() {
        let node = UiNode::Tooltip {
            content: Box::new(UiNode::Text {
                value: "Status".to_string(),
            }),
            tooltip: vec![
                UiNode::Text {
                    value: "Derived from host state".to_string(),
                },
                UiNode::Grid {
                    columns: 2,
                    children: vec![
                        UiNode::Text {
                            value: "A".to_string(),
                        },
                        UiNode::Text {
                            value: "true".to_string(),
                        },
                    ],
                },
            ],
        };
        let ctx = egui::Context::default();
        let mut renderer = UiDslRenderer::default();
        let mut output = UiRenderOutput::default();

        let _ = ctx.run(Default::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                output = renderer.render(ui, &node);
            });
        });

        assert_eq!(output.stats.nodes, 2);
        assert_eq!(output.stats.tooltips, 1);
        assert_eq!(output.stats.text, 1);
    }

    #[test]
    fn renders_disabled_button_with_condition_tooltip() {
        let node = UiNode::Button {
            label: "Build".to_string(),
            command: Some("colony.build".to_string()),
            secondary_command: None,
            secondary_shift_command: None,
            full_width: false,
            disabled: true,
            disabled_when: Some(UiConditionDisplay {
                label: "Can build mine".to_string(),
                satisfied: false,
                operator: UiConditionOperator::All,
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
        };
        let ctx = egui::Context::default();
        let mut renderer = UiDslRenderer::default();
        let mut output = UiRenderOutput::default();

        let _ = ctx.run(Default::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                output = renderer.render(ui, &node);
            });
        });

        assert_eq!(output.stats.buttons, 1);
        assert_eq!(output.stats.disabled_buttons, 1);
        assert_eq!(output.clicked_commands, Vec::<String>::new());
    }

    #[test]
    fn formats_condition_display_as_tree_lines() {
        let condition = UiConditionDisplay {
            label: "次のいずれかを満たす".to_string(),
            satisfied: false,
            operator: UiConditionOperator::Any,
            children: vec![
                UiConditionDisplay {
                    label: "人口500を超えるコロニーを持つ".to_string(),
                    satisfied: false,
                    operator: UiConditionOperator::Leaf,
                    children: Vec::new(),
                },
                UiConditionDisplay {
                    label: "次のすべてを満たす".to_string(),
                    satisfied: false,
                    operator: UiConditionOperator::All,
                    children: vec![
                        UiConditionDisplay {
                            label: "軌道上居住地のテクノロジーを研究済み".to_string(),
                            satisfied: true,
                            operator: UiConditionOperator::Leaf,
                            children: Vec::new(),
                        },
                        UiConditionDisplay {
                            label: "気候工学のテクノロジーを研究済み".to_string(),
                            satisfied: false,
                            operator: UiConditionOperator::Leaf,
                            children: Vec::new(),
                        },
                    ],
                },
            ],
        };

        assert_eq!(
            condition_display_lines(&condition),
            vec![
                "fail [ANY] 次のいずれかを満たす",
                "  fail 人口500を超えるコロニーを持つ",
                "  fail [ALL] 次のすべてを満たす",
                "    ok 軌道上居住地のテクノロジーを研究済み",
                "    fail 気候工学のテクノロジーを研究済み",
            ]
        );
    }

    #[test]
    fn renders_lua_cloned_existing_ui_fragments_to_egui() {
        let lua = Lua::new();
        register_ui_fragment_definition_accumulator(&lua).expect("register accumulator");
        register_ui_dsl_module(&lua).expect("register module");
        lua.load(include_str!("../../macrocosmo/scripts/ui/init.lua"))
            .set_name("macrocosmo/scripts/ui/init.lua")
            .exec()
            .expect("load cloned UI");

        let registry = parse_ui_fragment_definitions(&lua).expect("parse fragments");
        let nodes: Vec<UiNode> = registry
            .iter()
            .map(|definition| {
                let view = lua.create_table().expect("view");
                definition.inflate(&lua, view).expect("inflate")
            })
            .collect();

        let ctx = egui::Context::default();
        let mut renderer = UiDslRenderer::default();
        let mut output = UiRenderOutput::default();

        let _ = ctx.run(Default::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                output = renderer.render_many(ui, &nodes);
            });
        });

        assert_eq!(registry.len(), 30);
        assert_eq!(output.clicked_commands, Vec::<String>::new());
        assert!(output.stats.nodes > 250, "{:?}", output.stats);
        assert!(output.stats.sections >= 40, "{:?}", output.stats);
        assert!(output.stats.text > 100, "{:?}", output.stats);
        assert!(output.stats.buttons > 50, "{:?}", output.stats);
        assert!(output.stats.progress >= 3, "{:?}", output.stats);
    }
}

fn attach_disabled_when_tooltip(
    response: egui::Response,
    disabled: bool,
    disabled_when: &Option<UiConditionDisplay>,
) -> egui::Response {
    if !disabled {
        return response;
    }

    let Some(condition) = disabled_when else {
        return response;
    };

    response.on_disabled_hover_ui(|ui| {
        ui.set_min_width(320.0);
        render_condition_tree(ui, condition);
    })
}

#[derive(Clone)]
struct ConditionTreeRow<'a> {
    condition: &'a UiConditionDisplay,
    depth: usize,
    is_last: bool,
    ancestor_has_next: Vec<bool>,
}

fn render_condition_tree(ui: &mut egui::Ui, condition: &UiConditionDisplay) {
    let mut rows = Vec::new();
    flatten_condition_tree(condition, 0, true, &mut Vec::new(), &mut rows);

    let tree_left = 14.0;
    let indent = 22.0;
    let row_height = 24.0;
    let status_radius = 5.0;
    let branch_color = ui.visuals().widgets.noninteractive.fg_stroke.color;
    let text_color = ui.visuals().text_color();
    let ok_color = egui::Color32::from_rgb(72, 150, 88);
    let fail_color = egui::Color32::from_rgb(190, 72, 72);

    for row in rows {
        let desired_size = egui::vec2(ui.available_width().max(320.0), row_height);
        let (rect, _) = ui.allocate_exact_size(desired_size, egui::Sense::hover());
        let painter = ui.painter();
        let center_y = rect.center().y;
        let node_x = |depth: usize| rect.left() + tree_left + depth as f32 * indent;

        for (ancestor_depth, has_next) in row.ancestor_has_next.iter().enumerate() {
            if *has_next {
                let x = node_x(ancestor_depth);
                painter.line_segment(
                    [
                        egui::pos2(x, rect.top() - 2.0),
                        egui::pos2(x, rect.bottom() + 2.0),
                    ],
                    egui::Stroke::new(1.0, branch_color),
                );
            }
        }

        if row.depth > 0 {
            let branch_depth = row.depth - 1;
            let x = node_x(branch_depth);
            let child_x = node_x(row.depth);
            painter.line_segment(
                [egui::pos2(x, rect.top() - 2.0), egui::pos2(x, center_y)],
                egui::Stroke::new(1.0, branch_color),
            );
            painter.line_segment(
                [
                    egui::pos2(x, center_y),
                    egui::pos2(child_x + status_radius, center_y),
                ],
                egui::Stroke::new(1.0, branch_color),
            );
            if !row.is_last {
                painter.line_segment(
                    [egui::pos2(x, center_y), egui::pos2(x, rect.bottom() + 2.0)],
                    egui::Stroke::new(1.0, branch_color),
                );
            }
        }

        let status_x = node_x(row.depth);
        let status_color = if row.condition.satisfied {
            ok_color
        } else {
            fail_color
        };
        painter.circle_filled(egui::pos2(status_x, center_y), status_radius, status_color);

        let mut x = status_x + 12.0;
        if let Some(label) = condition_operator_label(row.condition.operator) {
            let galley = painter.layout_no_wrap(
                label.to_string(),
                egui::FontId::monospace(11.0),
                text_color,
            );
            let badge_rect = egui::Rect::from_min_size(
                egui::pos2(x, center_y - 8.0),
                egui::vec2(galley.size().x + 10.0, 16.0),
            );
            painter.rect_filled(badge_rect, 3.0, ui.visuals().faint_bg_color);
            painter.rect_stroke(
                badge_rect,
                3.0,
                egui::Stroke::new(1.0, branch_color),
                egui::StrokeKind::Inside,
            );
            painter.galley(
                egui::pos2(
                    badge_rect.left() + 5.0,
                    badge_rect.center().y - galley.size().y / 2.0,
                ),
                galley,
                text_color,
            );
            x = badge_rect.right() + 6.0;
        }

        painter.text(
            egui::pos2(x, center_y),
            egui::Align2::LEFT_CENTER,
            &row.condition.label,
            egui::TextStyle::Body.resolve(ui.style()),
            text_color,
        );
    }
}

fn flatten_condition_tree<'a>(
    condition: &'a UiConditionDisplay,
    depth: usize,
    is_last: bool,
    ancestor_has_next: &mut Vec<bool>,
    rows: &mut Vec<ConditionTreeRow<'a>>,
) {
    rows.push(ConditionTreeRow {
        condition,
        depth,
        is_last,
        ancestor_has_next: ancestor_has_next.clone(),
    });

    let child_count = condition.children.len();
    if child_count == 0 {
        return;
    }

    ancestor_has_next.push(!is_last);
    for (index, child) in condition.children.iter().enumerate() {
        flatten_condition_tree(
            child,
            depth + 1,
            index + 1 == child_count,
            ancestor_has_next,
            rows,
        );
    }
    ancestor_has_next.pop();
}

fn condition_operator_label(operator: UiConditionOperator) -> Option<&'static str> {
    match operator {
        UiConditionOperator::Leaf => None,
        UiConditionOperator::All => Some("ALL"),
        UiConditionOperator::Any => Some("ANY"),
        UiConditionOperator::Not => Some("NOT"),
        UiConditionOperator::Group => Some("GROUP"),
    }
}

#[cfg(test)]
fn condition_display_lines(condition: &UiConditionDisplay) -> Vec<String> {
    fn collect(condition: &UiConditionDisplay, depth: usize, out: &mut Vec<String>) {
        let status = if condition.satisfied { "ok" } else { "fail" };
        let op = condition_operator_label(condition.operator)
            .map(|op| format!("[{op}] "))
            .unwrap_or_default();
        out.push(format!(
            "{}{status} {op}{}",
            "  ".repeat(depth),
            condition.label
        ));
        for child in &condition.children {
            collect(child, depth + 1, out);
        }
    }

    let mut lines = Vec::new();
    collect(condition, 0, &mut lines);
    lines
}
