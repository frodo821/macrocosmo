//! Plots tab: select metrics and render their recent samples as line
//! charts using a hand-rolled `egui::Painter` routine (no external
//! `egui_plot` dependency).

use bevy_egui::egui;
use macrocosmo_ai::{AiBus, MetricId};

use super::PlotsState;

/// Available window presets (in ticks) shown in the header.
const WINDOW_PRESETS: &[i64] = &[100, 500, 2000];

pub fn draw_plots(
    ui: &mut egui::Ui,
    state: &mut PlotsState,
    bus: &AiBus,
    now: i64,
) {
    ui.horizontal(|ui| {
        ui.label("Window:");
        for &preset in WINDOW_PRESETS {
            let label = format!("{preset}");
            if ui
                .selectable_label(state.window_ticks == preset, label)
                .clicked()
            {
                state.window_ticks = preset;
            }
        }
        ui.add(
            egui::DragValue::new(&mut state.window_ticks)
                .range(10..=100_000)
                .prefix("ticks="),
        );
    });
    ui.separator();

    egui::SidePanel::right("ai_debug_plots_selector")
        .resizable(true)
        .default_width(220.0)
        .show_inside(ui, |ui| {
            ui.label(egui::RichText::new("Select metrics").strong());
            egui::ScrollArea::vertical()
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    let snapshot = bus.snapshot();
                    let mut ids: Vec<&MetricId> = snapshot.metrics.keys().collect();
                    ids.sort();
                    for id in ids {
                        let mut checked = state.selected_metrics.iter().any(|m| m == id);
                        let prev = checked;
                        ui.checkbox(&mut checked, id.as_str());
                        if checked && !prev {
                            state.selected_metrics.push(id.clone());
                        } else if !checked && prev {
                            state.selected_metrics.retain(|m| m != id);
                        }
                    }
                });
        });

    egui::CentralPanel::default().show_inside(ui, |ui| {
        if state.selected_metrics.is_empty() {
            ui.label(
                egui::RichText::new(
                    "Pick one or more metrics in the right panel to plot.",
                )
                .weak()
                .italics(),
            );
            return;
        }
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                for id in state.selected_metrics.clone() {
                    draw_metric_plot(ui, bus, &id, now, state.window_ticks);
                    ui.add_space(6.0);
                }
            });
    });
}

fn draw_metric_plot(
    ui: &mut egui::Ui,
    bus: &AiBus,
    id: &MetricId,
    now: i64,
    window: i64,
) {
    let points: Vec<(i64, f64)> = bus
        .window(id, now, window)
        .map(|tv| (tv.at, tv.value))
        .collect();

    ui.label(egui::RichText::new(id.as_str()).strong());

    let desired = egui::vec2(ui.available_width().min(640.0), 120.0);
    let (rect, _response) = ui.allocate_exact_size(desired, egui::Sense::hover());
    let painter = ui.painter_at(rect);
    let visuals = ui.visuals();

    painter.rect_filled(rect, 4.0, visuals.extreme_bg_color);
    painter.rect_stroke(
        rect,
        4.0,
        egui::Stroke::new(1.0, visuals.widgets.noninteractive.bg_stroke.color),
        egui::StrokeKind::Inside,
    );

    if points.len() < 2 {
        painter.text(
            rect.center(),
            egui::Align2::CENTER_CENTER,
            if points.is_empty() {
                "(no samples in window)"
            } else {
                "(need >= 2 samples)"
            },
            egui::FontId::proportional(12.0),
            visuals.weak_text_color(),
        );
        return;
    }

    let t_min = points.first().unwrap().0 as f64;
    let t_max = points.last().unwrap().0 as f64;
    let (mut v_min, mut v_max) = (f64::INFINITY, f64::NEG_INFINITY);
    for (_, v) in &points {
        if *v < v_min {
            v_min = *v;
        }
        if *v > v_max {
            v_max = *v;
        }
    }
    // Pad degenerate ranges so the line is visible.
    if (v_max - v_min).abs() < f64::EPSILON {
        v_min -= 0.5;
        v_max += 0.5;
    }
    let t_span = (t_max - t_min).max(1.0);
    let v_span = (v_max - v_min).max(f64::EPSILON);

    let project = |t: f64, v: f64| -> egui::Pos2 {
        let tx = (t - t_min) / t_span;
        let ty = 1.0 - (v - v_min) / v_span;
        egui::pos2(
            rect.left() + (tx as f32) * rect.width(),
            rect.top() + (ty as f32) * rect.height(),
        )
    };

    let mut prev = project(points[0].0 as f64, points[0].1);
    let line_color = visuals.hyperlink_color;
    for (t, v) in points.iter().skip(1) {
        let cur = project(*t as f64, *v);
        painter.line_segment([prev, cur], egui::Stroke::new(1.5, line_color));
        prev = cur;
    }

    // Axis labels (min/max of both axes, lightweight).
    let weak = visuals.weak_text_color();
    painter.text(
        egui::pos2(rect.left() + 4.0, rect.top() + 4.0),
        egui::Align2::LEFT_TOP,
        format!("{:.3}", v_max),
        egui::FontId::proportional(10.0),
        weak,
    );
    painter.text(
        egui::pos2(rect.left() + 4.0, rect.bottom() - 4.0),
        egui::Align2::LEFT_BOTTOM,
        format!("{:.3}", v_min),
        egui::FontId::proportional(10.0),
        weak,
    );
    painter.text(
        egui::pos2(rect.left() + 4.0, rect.bottom() - 14.0),
        egui::Align2::LEFT_BOTTOM,
        format!("t={}", t_min as i64),
        egui::FontId::proportional(10.0),
        weak,
    );
    painter.text(
        egui::pos2(rect.right() - 4.0, rect.bottom() - 14.0),
        egui::Align2::RIGHT_BOTTOM,
        format!("t={}", t_max as i64),
        egui::FontId::proportional(10.0),
        weak,
    );
}
