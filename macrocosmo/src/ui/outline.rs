use bevy::prelude::*;
use bevy_egui::egui;

use crate::colony::{BuildQueue, BuildingQueue, Buildings, Colony, Production};
use crate::components::Position;
use crate::galaxy::{Planet, StarSystem, SystemAttributes};
use crate::knowledge::{KnowledgeStore, ShipSnapshotState};
use crate::ship::Owner;
use crate::ship::fleet::{Fleet, FleetMembers};
use crate::ship::{Cargo, Ship, ShipHitpoints, ShipState, SurveyData};
use crate::ship_design::ShipDesignRegistry;
use crate::visualization::{OutlineExpandedSystems, SelectedShip, SelectedShips, SelectedSystem};

// #491: The `ShipOutlineView` / `ship_outline_view` helpers were factored
// out of this module into [`crate::knowledge::ship_view`] (data shape +
// selection) and [`crate::ui::ship_view`] (egui-adjacent formatters) so
// every UI panel can share the same projection-/snapshot-mediated read
// path. The names here remain exported as backward-compatibility aliases —
// existing imports (incl. `tests/outline_tree_ftl_leak.rs`) continue to
// work unchanged.
pub use crate::knowledge::ShipView as ShipOutlineView;
pub use crate::knowledge::ship_view as ship_outline_view;

/// #487: Status label for the "In Transit" section. Returns `None` for
/// steady-state variants (`InSystem` / `Refitting`) so the caller can
/// filter them out — they belong in the docked / Stationed-Elsewhere
/// sections.
///
/// #491 (D-H-4): SubLight/FTL transit are surfaced separately so the
/// player sees that an FTL ship cannot be intercepted.
fn snapshot_status_in_transit_label(state: &ShipSnapshotState) -> Option<&'static str> {
    match state {
        ShipSnapshotState::InSystem | ShipSnapshotState::Refitting => None,
        ShipSnapshotState::InTransitSubLight => Some("In Transit"),
        ShipSnapshotState::InTransitFTL => Some("FTL"),
        ShipSnapshotState::Surveying => Some("Surveying"),
        ShipSnapshotState::Settling => Some("Settling"),
        ShipSnapshotState::Loitering { .. } => Some("Loitering"),
        ShipSnapshotState::Destroyed => Some("Destroyed"),
        ShipSnapshotState::Missing => Some("Missing"),
    }
}

/// #487: Tooltip status label — handles every variant including the
/// steady-state ones since the tooltip is shown regardless of section.
fn snapshot_status_tooltip_label(state: &ShipSnapshotState) -> &'static str {
    match state {
        ShipSnapshotState::InSystem => "Docked",
        ShipSnapshotState::InTransitSubLight => "In Transit",
        ShipSnapshotState::InTransitFTL => "FTL",
        ShipSnapshotState::Surveying => "Surveying",
        ShipSnapshotState::Settling => "Settling",
        ShipSnapshotState::Refitting => "Refitting",
        ShipSnapshotState::Loitering { .. } => "Loitering",
        ShipSnapshotState::Destroyed => "Destroyed",
        ShipSnapshotState::Missing => "Missing",
    }
}

/// Draw a ship tooltip on hover.
///
/// #487: `view_state` is the light-coherent [`ShipSnapshotState`] derived
/// from [`ship_outline_view`] — never the realtime ECS [`ShipState`] for
/// own-empire ships. `None` means the viewing empire has no knowledge of
/// the ship (rendered as "Unknown").
fn ship_tooltip(
    ui: &mut egui::Ui,
    ship: &Ship,
    view_state: Option<&ShipSnapshotState>,
    hp: &ShipHitpoints,
    design_name: &str,
) {
    ui.label(egui::RichText::new(&ship.name).strong());
    ui.label(format!("Design: {}", design_name));
    let status = view_state
        .map(snapshot_status_tooltip_label)
        .unwrap_or("Unknown");
    ui.label(format!("Status: {}", status));
    ui.label(format!("HP: {:.0}/{:.0}", hp.hull, hp.hull_max));
    if hp.armor_max > 0.0 {
        ui.label(format!("Armor: {:.0}/{:.0}", hp.armor, hp.armor_max));
    }
    if hp.shield_max > 0.0 {
        ui.label(format!("Shield: {:.0}/{:.0}", hp.shield, hp.shield_max));
    }
}

/// Draw an expandable system header with separate collapse toggle and selection.
/// Returns whether the section is expanded.
fn draw_system_header(
    ui: &mut egui::Ui,
    system_entity: Entity,
    system_name: &str,
    is_capital: bool,
    is_selected: bool,
    expanded: &mut OutlineExpandedSystems,
    selected_system: &mut SelectedSystem,
    planets: &Query<&Planet>,
) -> bool {
    let is_expanded = expanded.0.contains(&system_entity);

    ui.horizontal(|ui| {
        // Collapse/expand toggle
        let arrow = if is_expanded { "\u{25BC}" } else { "\u{25B6}" };
        if ui.small_button(arrow).clicked() {
            if is_expanded {
                expanded.0.remove(&system_entity);
            } else {
                expanded.0.insert(system_entity);
            }
        }

        // System name as selectable label
        let display_name = if is_capital {
            format!("{} \u{2605}", system_name)
        } else {
            system_name.to_string()
        };
        let label_color = if is_selected {
            egui::Color32::from_rgb(0, 255, 255)
        } else {
            egui::Color32::from_rgb(200, 200, 200)
        };
        let response = ui.selectable_label(
            is_selected,
            egui::RichText::new(&display_name).color(label_color),
        );
        let response = response.on_hover_ui(|ui| {
            ui.label(egui::RichText::new(system_name).strong());
            if is_capital {
                ui.label("Capital system");
            }
            let planet_count = planets.iter().filter(|p| p.system == system_entity).count();
            ui.label(format!("Planets: {}", planet_count));
            ui.label("Colonized");
        });
        if response.clicked() {
            selected_system.0 = Some(system_entity);
            // Don't touch selected_ship -- selections are independent
        }
    });

    is_expanded
}

/// Draw an expandable header for an unowned system (ships stationed elsewhere).
fn draw_unowned_system_header(
    ui: &mut egui::Ui,
    system_entity: Entity,
    system_name: &str,
    is_selected: bool,
    expanded: &mut OutlineExpandedSystems,
    selected_system: &mut SelectedSystem,
    planets: &Query<&Planet>,
) -> bool {
    let is_expanded = expanded.0.contains(&system_entity);

    ui.horizontal(|ui| {
        let arrow = if is_expanded { "\u{25BC}" } else { "\u{25B6}" };
        if ui.small_button(arrow).clicked() {
            if is_expanded {
                expanded.0.remove(&system_entity);
            } else {
                expanded.0.insert(system_entity);
            }
        }

        let label_color = if is_selected {
            egui::Color32::from_rgb(0, 255, 255)
        } else {
            egui::Color32::from_rgb(160, 160, 160)
        };
        let response = ui.selectable_label(
            is_selected,
            egui::RichText::new(system_name).color(label_color),
        );
        let response = response.on_hover_ui(|ui| {
            ui.label(egui::RichText::new(system_name).strong());
            let planet_count = planets.iter().filter(|p| p.system == system_entity).count();
            ui.label(format!("Planets: {}", planet_count));
        });
        if response.clicked() {
            selected_system.0 = Some(system_entity);
        }
    });

    is_expanded
}

/// Draw the ship list for an expanded system section.
/// #407: Shift+click toggles multi-select via `SelectedShips`.
/// #408: Right-click context menu on each entry.
/// #487: Tooltip status text is fed by [`ship_outline_view`] so it
/// shows the projection-derived state, not realtime ECS.
#[allow(clippy::too_many_arguments)]
fn draw_ship_list(
    ui: &mut egui::Ui,
    ship_entries: &[(Entity, String, String)],
    ships: &Query<(
        Entity,
        &mut Ship,
        &mut ShipState,
        Option<&mut Cargo>,
        &ShipHitpoints,
        Option<&SurveyData>,
    )>,
    selected_ship: &mut SelectedShip,
    selected_ships: &mut SelectedShips,
    design_registry: &ShipDesignRegistry,
    parent_system: Option<Entity>,
    selected_system: &mut SelectedSystem,
    is_station: bool,
    viewing_knowledge: Option<&KnowledgeStore>,
    viewing_empire: Option<Entity>,
) {
    if ship_entries.is_empty() {
        ui.label(egui::RichText::new("  (no ships)").weak().italics());
    } else {
        for (ship_entity, name, design_id) in ship_entries {
            let design_name = design_registry
                .get(design_id)
                .map(|d| d.name.as_str())
                .unwrap_or(design_id);
            let label = if is_station {
                // #406: Teal color for station entries
                egui::RichText::new(format!("  {} ({})", name, design_name))
                    .color(egui::Color32::from_rgb(0, 200, 180))
            } else {
                egui::RichText::new(format!("  {} ({})", name, design_name))
            };
            let is_selected = selected_ships.contains(*ship_entity);
            let mut response = ui.selectable_label(is_selected, label);
            if let Ok((_, ship, state, _, hp, _)) = ships.get(*ship_entity) {
                let view = ship_outline_view(
                    *ship_entity,
                    &ship,
                    &state,
                    viewing_knowledge,
                    viewing_empire,
                );
                response = response.on_hover_ui(|ui| {
                    ship_tooltip(ui, &ship, view.as_ref().map(|v| &v.state), &hp, design_name);
                });
            }
            // #408: Context menu on right-click
            let can_survey = design_registry.can_survey(design_id);
            let can_colonize = design_registry.can_colonize(design_id);
            let ctx_name = name.clone();
            let ctx_design_name = design_name.to_string();
            let ctx_ship_entity = *ship_entity;
            let ctx_parent = parent_system;
            response.context_menu(|ui| {
                draw_ship_context_menu(
                    ui,
                    &ctx_name,
                    &ctx_design_name,
                    ctx_ship_entity,
                    ctx_parent,
                    can_survey,
                    can_colonize,
                    is_station,
                    selected_ship,
                    selected_ships,
                    selected_system,
                );
            });
            if response.clicked() {
                let shift_held = ui.input(|i| i.modifiers.shift);
                if shift_held {
                    selected_ships.toggle(*ship_entity);
                } else {
                    selected_ships.set_single(*ship_entity);
                }
                selected_ship.0 = selected_ships.primary();
                // Don't touch selected_system -- selections are independent
            }
        }
    }
}

/// #408: Draw the context menu content for a ship/station entry in the outline.
#[allow(clippy::too_many_arguments)]
fn draw_ship_context_menu(
    ui: &mut egui::Ui,
    name: &str,
    design_name: &str,
    ship_entity: Entity,
    parent_system: Option<Entity>,
    can_survey: bool,
    can_colonize: bool,
    is_station: bool,
    selected_ship: &mut SelectedShip,
    selected_ships: &mut SelectedShips,
    selected_system: &mut SelectedSystem,
) {
    ui.label(egui::RichText::new(name).strong());
    if is_station {
        ui.label(format!("Station: {}", design_name));
    } else {
        ui.label(format!("Design: {}", design_name));
    }
    if can_survey {
        ui.label("Capability: Survey");
    }
    if can_colonize {
        ui.label("Capability: Colonize");
    }
    ui.separator();
    let select_label = if is_station {
        "Select station"
    } else {
        "Select ship"
    };
    if ui.button(select_label).clicked() {
        selected_ships.set_single(ship_entity);
        selected_ship.0 = selected_ships.primary();
        ui.close_menu();
    }
    if let Some(sys) = parent_system {
        if ui.button("Select system").clicked() {
            selected_system.0 = Some(sys);
            ui.close_menu();
        }
    }
}

/// #487: One row of the "In Transit" list. Plain data so the egui-free
/// unit tests in `tests/outline_tree_ftl_leak.rs` can pin the FTL-leak
/// invariant without booting the egui draw pipeline.
#[derive(Clone, Debug, PartialEq)]
pub struct InTransitEntry {
    pub entity: Entity,
    pub name: String,
    pub design_id: String,
    /// Static label rendered in the `[…]` suffix.
    pub status: &'static str,
}

/// #487: Compute the list of own-empire ships that should appear in the
/// outline tree's "In Transit" section, **filtered through the viewing
/// empire's `KnowledgeStore` projection** (= the FTL-leak fix).
///
/// Iterates the realtime ship query but consults the viewing empire's
/// projection store for the gating state. Ships whose projected state is
/// `InSystem` / `Refitting` are excluded (they belong in the docked or
/// Stationed-Elsewhere sections); ships with no projection at all are
/// also skipped — the "freshly-spawned own-ship before its seed
/// projection lands" edge case (test coverage:
/// `outline_no_projection_handles_gracefully`).
pub fn compute_in_transit_entries(
    ships: &Query<(
        Entity,
        &mut Ship,
        &mut ShipState,
        Option<&mut Cargo>,
        &ShipHitpoints,
        Option<&SurveyData>,
    )>,
    viewing_knowledge: Option<&KnowledgeStore>,
    viewing_empire: Option<Entity>,
) -> Vec<InTransitEntry> {
    let mut out: Vec<InTransitEntry> = Vec::new();
    for (entity, ship, state, _, _, _) in ships.iter() {
        if ship.is_immobile() {
            continue;
        }
        // #491 (D-M-9): Only own-empire ships flow through the In
        // Transit section. The realtime fallback path
        // (`viewing_knowledge.is_none()`, e.g. early Startup) shows
        // every ship since there's no empire-side filter to apply yet.
        if viewing_knowledge.is_some() {
            if let Some(viewing) = viewing_empire {
                if ship.owner != Owner::Empire(viewing) {
                    continue;
                }
            }
        }
        let Some(view) =
            ship_outline_view(entity, &ship, &state, viewing_knowledge, viewing_empire)
        else {
            // No projection / snapshot — skip. (= a freshly-spawned
            // ship before its seed projection lands; #481 covers the
            // common case.)
            continue;
        };
        let Some(status) = snapshot_status_in_transit_label(&view.state) else {
            continue;
        };
        out.push(InTransitEntry {
            entity,
            name: ship.name.clone(),
            design_id: ship.design_id.clone(),
            status,
        });
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
}

/// #487: Compute the "Stationed Elsewhere" groupings. Each tuple is
/// `(system_entity, system_name, ship_entries)` where `ship_entries` is
/// `(entity, name, design_id)` — the same shape
/// `draw_fleet_grouped_ship_list` expects.
///
/// Routes own-ship system membership through the viewing empire's
/// projection (= the same FTL-leak gate as the "In Transit" section).
pub fn compute_stationed_elsewhere(
    ships: &Query<(
        Entity,
        &mut Ship,
        &mut ShipState,
        Option<&mut Cargo>,
        &ShipHitpoints,
        Option<&SurveyData>,
    )>,
    stars: &Query<(Entity, &StarSystem, &Position, Option<&SystemAttributes>)>,
    owned_system_entities: &[Entity],
    viewing_knowledge: Option<&KnowledgeStore>,
    viewing_empire: Option<Entity>,
) -> Vec<(Entity, String, Vec<(Entity, String, String)>)> {
    let mut out: Vec<(Entity, String, Vec<(Entity, String, String)>)> = Vec::new();
    for (entity, ship, state, _, _, _) in ships.iter() {
        if ship.is_immobile() {
            continue;
        }
        // #491 (D-M-9): only own-empire ships, except in the realtime
        // fallback (early Startup with no KnowledgeStore yet).
        if viewing_knowledge.is_some() {
            if let Some(viewing) = viewing_empire {
                if ship.owner != Owner::Empire(viewing) {
                    continue;
                }
            }
        }
        let Some(view) =
            ship_outline_view(entity, &ship, &state, viewing_knowledge, viewing_empire)
        else {
            continue;
        };
        if !matches!(view.state, ShipSnapshotState::InSystem) {
            continue;
        }
        let Some(system) = view.system else {
            continue;
        };
        if owned_system_entities.contains(&system) {
            continue;
        }
        let Ok((_, star, _, _)) = stars.get(system) else {
            continue;
        };
        if let Some(entry) = out.iter_mut().find(|(e, _, _)| *e == system) {
            entry
                .2
                .push((entity, ship.name.clone(), ship.design_id.clone()));
        } else {
            out.push((
                system,
                star.name.clone(),
                vec![(entity, ship.name.clone(), ship.design_id.clone())],
            ));
        }
    }
    out.sort_by(|a, b| a.1.cmp(&b.1));
    for entry in &mut out {
        entry.2.sort_by(|a, b| a.1.cmp(&b.1));
    }
    out
}

/// Draws the left-side outline panel showing owned systems and ships.
/// #432: `viewed_empire` is the active empire entity (PlayerEmpire or
/// ObserverView). When `is_observer` is false, colonies and ships are
/// filtered to only those owned by the viewed empire. In observer mode
/// all objects are shown.
/// #487: `viewing_knowledge` is the viewing empire's `KnowledgeStore`
/// (resolved by the caller via `resolve_ui_empire`). Ship-state queries
/// for own-empire ships go through this store's projection table —
/// never realtime ECS state — closing the FTL leak in the In Transit /
/// Stationed Elsewhere / docked / station sections.
#[allow(clippy::too_many_arguments)]
pub fn draw_outline(
    ctx: &egui::Context,
    stars: &Query<(Entity, &StarSystem, &Position, Option<&SystemAttributes>)>,
    colonies: &Query<(
        Entity,
        &Colony,
        Option<&Production>,
        Option<&mut BuildQueue>,
        Option<&Buildings>,
        Option<&mut BuildingQueue>,
        Option<&crate::colony::MaintenanceCost>,
        Option<&crate::colony::FoodConsumption>,
    )>,
    ships: &Query<(
        Entity,
        &mut Ship,
        &mut ShipState,
        Option<&mut Cargo>,
        &ShipHitpoints,
        Option<&SurveyData>,
    )>,
    selected_system: &mut SelectedSystem,
    selected_ship: &mut SelectedShip,
    selected_ships: &mut SelectedShips,
    planets: &Query<&Planet>,
    expanded: &mut OutlineExpandedSystems,
    design_registry: &ShipDesignRegistry,
    fleets: &Query<(Entity, &Fleet, &FleetMembers)>,
    faction_owners: &Query<&crate::faction::FactionOwner>,
    viewed_empire: Option<Entity>,
    is_observer: bool,
    home_system_entity: Option<Entity>,
    viewing_knowledge: Option<&KnowledgeStore>,
) {
    // #432: Ownership predicate — returns true when the entity belongs to
    // the viewed empire (or always true in observer mode / no empire).
    let is_own_colony = |colony_entity: Entity| -> bool {
        if is_observer || viewed_empire.is_none() {
            return true;
        }
        let ve = viewed_empire.unwrap();
        faction_owners
            .get(colony_entity)
            .map(|fo| fo.0 == ve)
            .unwrap_or(false)
    };

    egui::SidePanel::left("outline_panel")
        .min_width(180.0)
        .max_width(220.0)
        .show(ctx, |ui| {
            ui.heading("Empire");
            ui.separator();

            egui::ScrollArea::vertical().show(ui, |ui| {
                // Collect systems that have colonies (owned systems)
                // #432: Only include colonies belonging to the viewed empire.
                let mut owned_systems: Vec<(Entity, String, bool)> = Vec::new();
                for (colony_entity, colony, _, _, _, _, _, _) in colonies.iter() {
                    if !is_own_colony(colony_entity) {
                        continue;
                    }
                    if let Some(sys) = colony.system(planets) {
                        if let Ok((entity, star, _, _)) = stars.get(sys) {
                            // Avoid duplicates if multiple colonies on same system
                            if !owned_systems.iter().any(|(e, _, _)| *e == entity) {
                                owned_systems.push((
                                    entity,
                                    star.name.clone(),
                                    Some(entity) == home_system_entity,
                                ));
                            }
                        }
                    }
                }

                // Sort: capital first, then alphabetical
                owned_systems.sort_by(|a, b| b.2.cmp(&a.2).then_with(|| a.1.cmp(&b.1)));

                // Auto-expand capital system on first encounter
                for (entity, _, is_capital) in &owned_systems {
                    if *is_capital && !expanded.0.contains(entity) {
                        // Check if we've ever toggled this -- use a sentinel approach:
                        // On first frame, expand capital systems by default.
                        // We use a simple heuristic: if expanded set is empty, initialize defaults.
                    }
                }
                // Initialize defaults: if the expanded set has never been populated,
                // expand capital systems by default.
                if expanded.0.is_empty() && !owned_systems.is_empty() {
                    for (entity, _, is_capital) in &owned_systems {
                        if *is_capital {
                            expanded.0.insert(*entity);
                        }
                    }
                }

                for (system_entity, system_name, is_capital) in &owned_systems {
                    let is_system_selected = selected_system.0 == Some(*system_entity);

                    let is_expanded = draw_system_header(
                        ui,
                        *system_entity,
                        system_name,
                        *is_capital,
                        is_system_selected,
                        expanded,
                        selected_system,
                        planets,
                    );

                    if is_expanded {
                        // #395/#406: Show stations separately from ships with visual distinction
                        // #432: Filter by viewed empire unless observer mode.
                        let ship_owner_filter = if is_observer { None } else { viewed_empire };
                        let system_stations = stations_at(
                            *system_entity,
                            ships,
                            ship_owner_filter,
                            viewing_knowledge,
                            viewed_empire,
                        );
                        let docked = ships_docked_at(
                            *system_entity,
                            ships,
                            ship_owner_filter,
                            viewing_knowledge,
                            viewed_empire,
                        );
                        let has_both = !system_stations.is_empty() && !docked.is_empty();
                        if !system_stations.is_empty() {
                            ui.indent(format!("outline_stations_{:?}", system_entity), |ui| {
                                // #406: Anchor icon + teal color for stations header
                                ui.label(
                                    egui::RichText::new("\u{2693} Stations")
                                        .small()
                                        .color(egui::Color32::from_rgb(0, 200, 180)),
                                );
                                draw_ship_list(
                                    ui,
                                    &system_stations,
                                    ships,
                                    selected_ship,
                                    selected_ships,
                                    design_registry,
                                    Some(*system_entity),
                                    selected_system,
                                    true,
                                    viewing_knowledge,
                                    viewed_empire,
                                );
                            });
                        }
                        // #406: Separator between stations and ships when both exist
                        if has_both {
                            ui.separator();
                        }
                        // #407: Group docked ships by fleet
                        ui.indent(format!("outline_ships_{:?}", system_entity), |ui| {
                            // #406: Ships sub-header when both sections exist
                            if has_both {
                                ui.label(
                                    egui::RichText::new("\u{2694} Ships")
                                        .small()
                                        .color(egui::Color32::from_rgb(200, 200, 120)),
                                );
                            }
                            draw_fleet_grouped_ship_list(
                                ui,
                                &docked,
                                ships,
                                selected_ship,
                                selected_ships,
                                design_registry,
                                fleets,
                                Some(*system_entity),
                                selected_system,
                                viewing_knowledge,
                                viewed_empire,
                            );
                        });
                    }
                }

                // Collect owned system entities for lookup
                let owned_system_entities: Vec<Entity> =
                    owned_systems.iter().map(|(e, _, _)| *e).collect();

                // "Stationed Elsewhere" section for ships docked at unowned systems
                // #395: Immobile ships (stations) are excluded here.
                // #432: Filter by viewed empire ownership.
                // #487: Routed through `compute_stationed_elsewhere` so own-ship
                // system membership respects the projection store.
                let unowned_system_ships = compute_stationed_elsewhere(
                    ships,
                    stars,
                    &owned_system_entities,
                    viewing_knowledge,
                    viewed_empire,
                );

                if !unowned_system_ships.is_empty() {
                    ui.separator();
                    egui::CollapsingHeader::new("Stationed Elsewhere")
                        .default_open(true)
                        .show(ui, |ui| {
                            // Auto-expand unowned system headers on first encounter
                            for (system_entity, _, _) in &unowned_system_ships {
                                if !expanded.0.contains(system_entity) {
                                    expanded.0.insert(*system_entity);
                                }
                            }

                            for (system_entity, system_name, docked) in &unowned_system_ships {
                                let is_system_selected = selected_system.0 == Some(*system_entity);

                                let is_expanded = draw_unowned_system_header(
                                    ui,
                                    *system_entity,
                                    system_name,
                                    is_system_selected,
                                    expanded,
                                    selected_system,
                                    planets,
                                );

                                if is_expanded {
                                    ui.indent(
                                        format!("outline_unowned_ships_{:?}", system_entity),
                                        |ui| {
                                            draw_fleet_grouped_ship_list(
                                                ui,
                                                docked,
                                                ships,
                                                selected_ship,
                                                selected_ships,
                                                design_registry,
                                                fleets,
                                                Some(*system_entity),
                                                selected_system,
                                                viewing_knowledge,
                                                viewed_empire,
                                            );
                                        },
                                    );
                                }
                            }
                        });
                }

                // "In Transit" section for ships not docked
                // #395: Immobile ships are excluded (they should never be in transit).
                // #432: Filter by viewed empire ownership.
                // #487: Routed through `compute_in_transit_entries` so the
                // gating state comes from the viewing empire's
                // `ShipProjection`, never realtime ECS.
                let in_transit =
                    compute_in_transit_entries(ships, viewing_knowledge, viewed_empire);

                if !in_transit.is_empty() {
                    ui.separator();
                    egui::CollapsingHeader::new("In Transit")
                        .default_open(true)
                        .show(ui, |ui| {
                            for InTransitEntry {
                                entity,
                                name,
                                design_id,
                                status,
                            } in &in_transit
                            {
                                let label = format!("{} [{}]", name, status);
                                let is_selected = selected_ships.contains(*entity);
                                let design_name = design_registry
                                    .get(design_id.as_str())
                                    .map(|d| d.name.clone())
                                    .unwrap_or_else(|| design_id.clone());
                                let mut response = ui.selectable_label(is_selected, &label);
                                if let Ok((_, ship, state, _, hp, _)) = ships.get(*entity) {
                                    let view = ship_outline_view(
                                        *entity,
                                        &ship,
                                        &state,
                                        viewing_knowledge,
                                        viewed_empire,
                                    );
                                    response = response.on_hover_ui(|ui| {
                                        ship_tooltip(
                                            ui,
                                            &ship,
                                            view.as_ref().map(|v| &v.state),
                                            &hp,
                                            &design_name,
                                        );
                                    });
                                }
                                // #408: Context menu on in-transit ships
                                let can_survey = design_registry.can_survey(design_id);
                                let can_colonize = design_registry.can_colonize(design_id);
                                let ctx_name = name.clone();
                                let ctx_design_name = design_name.clone();
                                let ctx_entity = *entity;
                                response.context_menu(|ui| {
                                    draw_ship_context_menu(
                                        ui,
                                        &ctx_name,
                                        &ctx_design_name,
                                        ctx_entity,
                                        None, // no parent system for in-transit
                                        can_survey,
                                        can_colonize,
                                        false,
                                        selected_ship,
                                        selected_ships,
                                        selected_system,
                                    );
                                });
                                if response.clicked() {
                                    let shift_held = ui.input(|i| i.modifiers.shift);
                                    if shift_held {
                                        selected_ships.toggle(*entity);
                                    } else {
                                        selected_ships.set_single(*entity);
                                    }
                                    selected_ship.0 = selected_ships.primary();
                                }
                            }
                        });
                }
            }); // ScrollArea
        });
}

/// Helper to collect mobile ships docked at a given system.
/// #395: Immobile ships (stations) are excluded — they appear in the "Stations" section.
/// #432: `owner_filter` optionally restricts results to ships owned by the given empire.
/// #487: Routed through [`ship_outline_view`] so own-ship docking is decided
/// by the viewing empire's `ShipProjection`, not realtime ECS state.
fn ships_docked_at(
    system: Entity,
    ships: &Query<(
        Entity,
        &mut Ship,
        &mut ShipState,
        Option<&mut Cargo>,
        &ShipHitpoints,
        Option<&SurveyData>,
    )>,
    owner_filter: Option<Entity>,
    viewing_knowledge: Option<&KnowledgeStore>,
    viewing_empire: Option<Entity>,
) -> Vec<(Entity, String, String)> {
    let mut result: Vec<(Entity, String, String)> = ships
        .iter()
        .filter_map(|(e, ship, state, _, _, _)| {
            if ship.is_immobile() {
                return None;
            }
            if let Some(empire) = owner_filter {
                if ship.owner != Owner::Empire(empire) {
                    return None;
                }
            }
            let view = ship_outline_view(e, &ship, &state, viewing_knowledge, viewing_empire)?;
            if matches!(view.state, ShipSnapshotState::InSystem) && view.system == Some(system) {
                return Some((e, ship.name.clone(), ship.design_id.clone()));
            }
            None
        })
        .collect();
    result.sort_by(|a, b| a.1.cmp(&b.1));
    result
}

/// #407: Draw ships grouped by fleet. Ships that share a fleet are shown
/// under a collapsible fleet header; solo-fleet ships (fleet with 1 member)
/// are shown directly without a header to avoid clutter.
#[allow(clippy::too_many_arguments)]
fn draw_fleet_grouped_ship_list(
    ui: &mut egui::Ui,
    ship_entries: &[(Entity, String, String)],
    ships: &Query<(
        Entity,
        &mut Ship,
        &mut ShipState,
        Option<&mut Cargo>,
        &ShipHitpoints,
        Option<&SurveyData>,
    )>,
    selected_ship: &mut SelectedShip,
    selected_ships: &mut SelectedShips,
    design_registry: &ShipDesignRegistry,
    fleets: &Query<(Entity, &Fleet, &FleetMembers)>,
    parent_system: Option<Entity>,
    selected_system: &mut SelectedSystem,
    viewing_knowledge: Option<&KnowledgeStore>,
    viewing_empire: Option<Entity>,
) {
    if ship_entries.is_empty() {
        ui.label(egui::RichText::new("  (no ships)").weak().italics());
        return;
    }

    // Group ship entries by fleet entity.
    let mut fleet_groups: std::collections::BTreeMap<
        Option<Entity>,
        Vec<(Entity, String, String)>,
    > = std::collections::BTreeMap::new();
    for (entity, name, design_id) in ship_entries {
        let fleet_entity = ships
            .get(*entity)
            .ok()
            .and_then(|(_, ship, _, _, _, _)| ship.fleet);
        fleet_groups.entry(fleet_entity).or_default().push((
            *entity,
            name.clone(),
            design_id.clone(),
        ));
    }

    // Partition into multi-ship fleets and solo ships.
    let mut multi_fleets: Vec<(Entity, String, Vec<(Entity, String, String)>)> = Vec::new();
    let mut solo_ships: Vec<(Entity, String, String)> = Vec::new();

    for (fleet_opt, members) in &fleet_groups {
        if let Some(fleet_entity) = fleet_opt {
            if let Ok((_, fleet, fleet_members)) = fleets.get(*fleet_entity) {
                if fleet_members.len() > 1 {
                    multi_fleets.push((*fleet_entity, fleet.name.clone(), members.clone()));
                    continue;
                }
            }
        }
        solo_ships.extend(members.iter().cloned());
    }

    // Draw multi-ship fleets with collapsible headers.
    for (fleet_entity, fleet_name, members) in &multi_fleets {
        let header_label = format!("{} ({} ships)", fleet_name, members.len());
        egui::CollapsingHeader::new(
            egui::RichText::new(&header_label)
                .small()
                .color(egui::Color32::from_rgb(150, 200, 255)),
        )
        .id_salt(format!("fleet_{:?}", fleet_entity))
        .default_open(true)
        .show(ui, |ui| {
            draw_ship_list(
                ui,
                members,
                ships,
                selected_ship,
                selected_ships,
                design_registry,
                parent_system,
                selected_system,
                false,
                viewing_knowledge,
                viewing_empire,
            );
        });
    }

    // Draw solo ships directly.
    if !solo_ships.is_empty() {
        draw_ship_list(
            ui,
            &solo_ships,
            ships,
            selected_ship,
            selected_ships,
            design_registry,
            parent_system,
            selected_system,
            false,
            viewing_knowledge,
            viewing_empire,
        );
    }
}

/// #395: Collect immobile ships (stations) docked at a given system.
/// #432: `owner_filter` optionally restricts results to ships owned by the given empire.
/// #487: Routed through [`ship_outline_view`] for the same reason as
/// [`ships_docked_at`] — even stations should reflect the viewing
/// empire's projection in normal play.
fn stations_at(
    system: Entity,
    ships: &Query<(
        Entity,
        &mut Ship,
        &mut ShipState,
        Option<&mut Cargo>,
        &ShipHitpoints,
        Option<&SurveyData>,
    )>,
    owner_filter: Option<Entity>,
    viewing_knowledge: Option<&KnowledgeStore>,
    viewing_empire: Option<Entity>,
) -> Vec<(Entity, String, String)> {
    let mut result: Vec<(Entity, String, String)> = ships
        .iter()
        .filter_map(|(e, ship, state, _, _, _)| {
            if !ship.is_immobile() {
                return None;
            }
            if let Some(empire) = owner_filter {
                if ship.owner != Owner::Empire(empire) {
                    return None;
                }
            }
            let view = ship_outline_view(e, &ship, &state, viewing_knowledge, viewing_empire)?;
            if matches!(view.state, ShipSnapshotState::InSystem) && view.system == Some(system) {
                return Some((e, ship.name.clone(), ship.design_id.clone()));
            }
            None
        })
        .collect();
    result.sort_by(|a, b| a.1.cmp(&b.1));
    result
}
