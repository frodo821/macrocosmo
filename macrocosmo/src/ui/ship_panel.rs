use bevy::prelude::*;
use bevy_egui::egui;

use crate::amount::Amt;
use crate::colony::{
    BuildQueue, BuildingQueue, Buildings, Colony, ConstructionParams, FoodConsumption,
    MaintenanceCost, Production, ResourceCapacity, ResourceStockpile, SystemBuildingQueue,
    SystemBuildings,
};
use crate::components::Position;
use crate::galaxy::{Planet, StarSystem, SystemAttributes};
use crate::modifier::ModifiedValue;
use crate::physics;
use crate::player::{AboardShip, Player, StationedAt};
use crate::ship::{
    Cargo, CommandQueue, CourierMode, CourierRoute, DockedAt, PendingShipCommand, QueuedCommand,
    RulesOfEngagement, Ship, ShipHitpoints, ShipModifiers, ShipState, ShipStats, SurveyData,
};
use crate::ship_design::{HullRegistry, ShipDesignRegistry};
use crate::time_system::GameClock;
use crate::ui::{draw_modifier_breakdown, modified_value_label_with_tooltip};
use crate::visualization::SelectedShip;

/// Action returned from draw_ship_panel when the player clicks "Scrap Ship".
/// Processed in draw_all_ui where Commands is available for despawning.
pub struct ShipScrapAction {
    pub ship_entity: Entity,
    pub colony_entity: Entity,
    pub ship_name: String,
    pub system_name: String,
    pub minerals_refund: Amt,
    pub energy_refund: Amt,
}

/// #123: Action for applying the current registered design to a docked ship.
/// All cost / module computation is re-resolved at apply time using the
/// ship's `design_id`, so this carries only the identifying information.
pub struct ShipRefitAction {
    pub ship_entity: Entity,
    pub system_entity: Entity,
}

/// #123: Apply Refit to every refit-eligible ship in a fleet, in one batch.
/// Each ship is processed independently — if some can't afford it, others
/// still proceed (in registry-order). Resolution happens at apply time.
pub struct FleetRefitAction {
    pub fleet_entity: Entity,
}

/// #123: Display data for the design-based refit panel.
struct RefitInfo {
    /// The design's current revision (the one we'd refit *to*).
    target_revision: u64,
    /// The ship's current revision (revisions behind = `target - current`).
    current_revision: u64,
    /// Display name of the design (e.g. "Explorer Mk.I").
    design_name: String,
    cost_minerals: Amt,
    cost_energy: Amt,
    refit_time: i64,
}

/// #123: Aggregate summary of fleet-wide refit availability for the panel.
struct FleetRefitSummary {
    fleet_entity: Entity,
    fleet_name: String,
    /// Number of fleet members that are docked AND have an outdated design.
    eligible_count: usize,
    /// Sum of refit costs across all eligible members.
    total_cost_minerals: Amt,
    total_cost_energy: Amt,
    /// Worst (longest) refit time across eligible members — fleet refit
    /// completes when the slowest member is done.
    max_refit_time: i64,
}

/// All actions that can be triggered from the ship panel UI.
/// Processed in draw_all_ui where mutable access is available.
#[derive(Default)]
pub struct ShipPanelActions {
    pub scrap: Option<ShipScrapAction>,
    pub cancel_command_index: Option<usize>,
    pub clear_commands: bool,
    pub cancel_current: bool,
    pub refit: Option<ShipRefitAction>,
    /// #123: Apply Refit to all refit-eligible ships in a fleet.
    pub fleet_refit: Option<FleetRefitAction>,
    /// #57: ROE change action — (ship_entity, new_roe, command_delay)
    pub set_roe: Option<(Entity, RulesOfEngagement, i64)>,
    /// #59: Player wants to board the selected ship
    pub board_ship: Option<Entity>,
    /// #59: Player wants to disembark from the selected ship
    pub disembark: bool,
    /// #117: Add the currently-selected system as a waypoint to the ship's
    /// CourierRoute (creating one if absent).
    pub courier_add_waypoint: Option<(Entity, Entity, CourierMode)>,
    /// #117: Toggle the paused flag on the courier's route.
    pub courier_toggle_pause: Option<Entity>,
    /// #117: Remove the entire CourierRoute from the ship.
    pub courier_clear_route: Option<Entity>,
    /// #117: Change the route's mode.
    pub courier_set_mode: Option<(Entity, CourierMode)>,
    /// #229 / #240: Player clicked Deploy next to a cargo deliverable. The
    /// outer system stashes this in `DeployMode`, then the next map click
    /// becomes a `QueuedCommand::DeployDeliverable` (snapping to a nearby
    /// star if one is within the snap radius, otherwise deploying at the
    /// cursor's world position). Payload: (ship, cargo item_index).
    pub deploy_mode_request: Option<(Entity, usize)>,
    /// #229: Player requested resources transferred from the ship's Cargo
    /// into a ConstructionPlatform's accumulator pool. Payload:
    /// (ship, structure, minerals, energy).
    pub transfer_request: Option<(Entity, Entity, Amt, Amt)>,
    /// #229: Player requested draining a Scrapyard into the ship's Cargo.
    /// Payload: (ship, structure).
    pub load_from_scrapyard_request: Option<(Entity, Entity)>,
    /// #389: Dock ship at a harbour. Payload: (ship, harbour).
    pub dock_at: Option<(Entity, Entity)>,
    /// #389: Undock ship from its current harbour. Payload: ship entity.
    pub undock: Option<Entity>,
}

/// #229: Display info about a deep-space structure close enough to the ship
/// that Transfer / LoadFromScrapyard commands will resolve without a long
/// sublight cruise. The outer system pre-computes the list.
#[derive(Clone, Debug)]
pub struct NearbyStructure {
    pub entity: Entity,
    pub name: String,
    pub is_platform: bool,
    pub is_scrapyard: bool,
    /// Distance from the ship, in light-years. Used to label entries so the
    /// player knows the ship needs to move first.
    pub distance_ly: f64,
}

/// Resolve an Entity to a star system name, falling back to "Unknown".
pub(super) fn system_name(
    entity: Entity,
    stars: &Query<(Entity, &StarSystem, &Position, Option<&SystemAttributes>)>,
) -> String {
    stars
        .get(entity)
        .map(|(_, s, _, _)| s.name.clone())
        .unwrap_or_else(|_| "Unknown".to_string())
}

/// Collected status information for the ship panel UI.
struct ShipStatusInfo {
    label: String,
    /// Progress fraction 0.0..=1.0, if applicable.
    progress: Option<(i64, i64, f32)>, // (elapsed, total, fraction)
}

/// Build a detailed status string (and optional progress) from a ShipState.
fn build_status_info(
    state: &ShipState,
    clock: &GameClock,
    stars: &Query<(Entity, &StarSystem, &Position, Option<&SystemAttributes>)>,
) -> ShipStatusInfo {
    match state {
        ShipState::InSystem { system } => ShipStatusInfo {
            label: format!("Docked at {}", system_name(*system, stars)),
            progress: None,
        },
        ShipState::SubLight {
            target_system,
            departed_at,
            arrival_at,
            ..
        } => {
            let total = (arrival_at - departed_at).max(1);
            let elapsed = (clock.elapsed - departed_at).clamp(0, total);
            let pct = elapsed as f32 / total as f32;
            let dest_name = target_system
                .map(|e| system_name(e, stars))
                .unwrap_or_else(|| "deep space".to_string());
            ShipStatusInfo {
                label: format!(
                    "Moving to {} ({}/{} hd, {:.0}%)",
                    dest_name,
                    elapsed,
                    total,
                    pct * 100.0
                ),
                progress: Some((elapsed, total, pct)),
            }
        }
        ShipState::InFTL {
            destination_system,
            departed_at,
            arrival_at,
            ..
        } => {
            let total = (arrival_at - departed_at).max(1);
            let elapsed = (clock.elapsed - departed_at).clamp(0, total);
            let pct = elapsed as f32 / total as f32;
            ShipStatusInfo {
                label: format!(
                    "FTL to {} ({}/{} hd, {:.0}%)",
                    system_name(*destination_system, stars),
                    elapsed,
                    total,
                    pct * 100.0
                ),
                progress: Some((elapsed, total, pct)),
            }
        }
        ShipState::Surveying {
            target_system,
            started_at,
            completes_at,
        } => {
            let total = (completes_at - started_at).max(1);
            let elapsed = (clock.elapsed - started_at).clamp(0, total);
            let pct = elapsed as f32 / total as f32;
            ShipStatusInfo {
                label: format!(
                    "Surveying {} ({}/{} hd, {:.0}%)",
                    system_name(*target_system, stars),
                    elapsed,
                    total,
                    pct * 100.0
                ),
                progress: Some((elapsed, total, pct)),
            }
        }
        ShipState::Settling {
            system,
            started_at,
            completes_at,
            ..
        } => {
            let total = (completes_at - started_at).max(1);
            let elapsed = (clock.elapsed - started_at).clamp(0, total);
            let pct = elapsed as f32 / total as f32;
            ShipStatusInfo {
                label: format!(
                    "Settling {} ({}/{} hd, {:.0}%)",
                    system_name(*system, stars),
                    elapsed,
                    total,
                    pct * 100.0
                ),
                progress: Some((elapsed, total, pct)),
            }
        }
        ShipState::Refitting {
            system,
            started_at,
            completes_at,
            ..
        } => {
            let total = (completes_at - started_at).max(1);
            let elapsed = (clock.elapsed - started_at).clamp(0, total);
            let pct = elapsed as f32 / total as f32;
            ShipStatusInfo {
                label: format!(
                    "Refitting at {} ({}/{} hd, {:.0}%)",
                    system_name(*system, stars),
                    elapsed,
                    total,
                    pct * 100.0
                ),
                progress: Some((elapsed, total, pct)),
            }
        }
        // #185: Loitering at deep-space coordinates.
        ShipState::Loitering { position } => ShipStatusInfo {
            label: format!(
                "Loitering at ({:.2}, {:.2}, {:.2})",
                position[0], position[1], position[2]
            ),
            progress: None,
        },
        // #217: Scouting — display like Surveying but labelled "Scouting".
        ShipState::Scouting {
            target_system,
            started_at,
            completes_at,
            ..
        } => {
            let total = (completes_at - started_at).max(1);
            let elapsed = (clock.elapsed - started_at).clamp(0, total);
            let pct = elapsed as f32 / total as f32;
            ShipStatusInfo {
                label: format!(
                    "Scouting {} ({}/{} hd, {:.0}%)",
                    system_name(*target_system, stars),
                    elapsed,
                    total,
                    pct * 100.0
                ),
                progress: Some((elapsed, total, pct)),
            }
        }
    }
}

/// #229: Pure-string formatter for the deliverable-family `QueuedCommand`
/// variants. Separated from `format_queued_command` so the tests below can
/// exercise the new variants without mocking Bevy's `Query<StarSystem>`.
/// Returns `None` for non-deliverable variants (which still need a query to
/// resolve system names).
fn format_deliverable_command(cmd: &QueuedCommand) -> Option<String> {
    match cmd {
        QueuedCommand::LoadDeliverable {
            stockpile_index, ..
        } => Some(format!("Load deliverable #{}", stockpile_index)),
        QueuedCommand::DeployDeliverable {
            position,
            item_index,
        } => Some(format!(
            "Deploy #{} -> ({:.1}, {:.1}, {:.1})",
            item_index, position[0], position[1], position[2]
        )),
        QueuedCommand::TransferToStructure {
            minerals, energy, ..
        } => Some(format!(
            "Transfer {}m / {}e to structure",
            minerals.to_f64(),
            energy.to_f64()
        )),
        QueuedCommand::LoadFromScrapyard { .. } => Some("Salvage scrapyard".to_string()),
        _ => None,
    }
}

/// Format a QueuedCommand as a human-readable string.
fn format_queued_command(
    cmd: &QueuedCommand,
    stars: &Query<(Entity, &StarSystem, &Position, Option<&SystemAttributes>)>,
) -> String {
    if let Some(s) = format_deliverable_command(cmd) {
        return s;
    }
    match cmd {
        QueuedCommand::MoveTo { system, .. } => {
            format!("Move -> {}", system_name(*system, stars))
        }
        QueuedCommand::Survey { system, .. } => {
            format!("Survey {}", system_name(*system, stars))
        }
        QueuedCommand::Colonize { .. } => "Colonize".to_string(),
        QueuedCommand::MoveToCoordinates { target } => {
            format!(
                "Move -> ({:.1}, {:.1}, {:.1})",
                target[0], target[1], target[2]
            )
        }
        // #217: Scout command display.
        QueuedCommand::Scout {
            target_system,
            observation_duration,
            report_mode,
        } => {
            format!(
                "Scout {} ({}hx, {})",
                system_name(*target_system, stars),
                observation_duration,
                match report_mode {
                    crate::ship::ReportMode::FtlComm => "FTL comm",
                    crate::ship::ReportMode::Return => "return",
                }
            )
        }
        // Deliverable variants are handled above; the catch-all is
        // unreachable in practice.
        QueuedCommand::LoadDeliverable { .. }
        | QueuedCommand::DeployDeliverable { .. }
        | QueuedCommand::TransferToStructure { .. }
        | QueuedCommand::LoadFromScrapyard { .. } => unreachable!(),
    }
}

/// Helper to collect mobile ships docked at a given system.
/// #395: Immobile ships (stations) are excluded — use `stations_docked_at` for those.
pub(super) fn ships_docked_at(
    system: Entity,
    ships: &Query<(
        Entity,
        &mut Ship,
        &mut ShipState,
        Option<&mut Cargo>,
        &ShipHitpoints,
        Option<&SurveyData>,
    )>,
) -> Vec<(Entity, String, String)> {
    let mut result: Vec<(Entity, String, String)> = ships
        .iter()
        .filter_map(|(e, ship, state, _, _, _)| {
            if ship.is_immobile() {
                return None;
            }
            if let ShipState::InSystem { system: s } = &*state {
                if *s == system {
                    return Some((e, ship.name.clone(), ship.design_id.clone()));
                }
            }
            None
        })
        .collect();
    result.sort_by(|a, b| a.1.cmp(&b.1));
    result
}

/// #395: Collect immobile ships (stations) docked at a given system.
pub(super) fn stations_docked_at(
    system: Entity,
    ships: &Query<(
        Entity,
        &mut Ship,
        &mut ShipState,
        Option<&mut Cargo>,
        &ShipHitpoints,
        Option<&SurveyData>,
    )>,
) -> Vec<(Entity, String, String)> {
    let mut result: Vec<(Entity, String, String)> = ships
        .iter()
        .filter_map(|(e, ship, state, _, _, _)| {
            if !ship.is_immobile() {
                return None;
            }
            if let ShipState::InSystem { system: s } = &*state {
                if *s == system {
                    return Some((e, ship.name.clone(), ship.design_id.clone()));
                }
            }
            None
        })
        .collect();
    result.sort_by(|a, b| a.1.cmp(&b.1));
    result
}

/// #229: Named aggregate of ship panel display data. Previously this was an
/// anonymous tuple; after adding cargo_items / structure-related fields the
/// positional form became unreadable.
struct ShipPanelData {
    ship_entity: Entity,
    name: String,
    design_id: String,
    hull_hp: f64,
    hull_max: f64,
    armor: f64,
    armor_max: f64,
    shield: f64,
    shield_max: f64,
    ftl_range: f64,
    sublight_speed: f64,
    status_info: ShipStatusInfo,
    docked_system: Option<Entity>,
    cargo_data: Option<(Amt, Amt)>,
    /// #229: Cloned list of non-resource items in the ship's Cargo. Each
    /// entry can be deployed via the Deploy button.
    cargo_items: Vec<crate::ship::CargoItem>,
    queued_cmds: Vec<String>,
    _home_port: Entity,
    home_port_name: String,
    maintenance_cost: Amt,
    docked_at_colony: Option<Entity>,
    is_cancellable: bool,
    pending_arrives_at: Option<i64>,
    has_survey_data: bool,
    survey_data_system: Option<String>,
    _ship_hull_id: String,
    ship_modules: Vec<crate::ship::EquippedModule>,
    is_refitting: bool,
    refit_info: Option<RefitInfo>,
    fleet_refit_summary: Option<FleetRefitSummary>,
    current_roe: RulesOfEngagement,
    roe_command_delay: i64,
    is_player_aboard: bool,
    can_board: bool,
    can_disembark: bool,
    /// #389: Harbour capacity (0 = not a harbour).
    harbour_capacity: u32,
    /// #389: Current docked size / list of docked ship names.
    harbour_docked_size: u32,
    harbour_docked_ships: Vec<String>,
    /// #389: If this ship is docked at a harbour, the harbour entity and name.
    docked_at_harbour: Option<(Entity, String)>,
    /// #391: Cloned modifier snapshots for tooltip breakdowns.
    mod_speed: Option<ModifiedValue>,
    mod_ftl_range: Option<ModifiedValue>,
    mod_attack: Option<ModifiedValue>,
    mod_defense: Option<ModifiedValue>,
    mod_evasion: Option<ModifiedValue>,
    mod_armor_max: Option<ModifiedValue>,
    mod_shield_max: Option<ModifiedValue>,
}

/// Draws the floating ship details panel when a ship is selected.
/// #53: Simplified - command buttons moved to context menu
/// #62: Detailed status display with progress bars and command queue
/// #64: Shows home port info and "Set Home Port" button
#[allow(clippy::too_many_arguments)]
pub fn draw_ship_panel(
    ctx: &egui::Context,
    selected_ship: &mut SelectedShip,
    ships_query: &mut Query<(
        Entity,
        &mut Ship,
        &mut ShipState,
        Option<&mut Cargo>,
        &ShipHitpoints,
        Option<&SurveyData>,
    )>,
    clock: &GameClock,
    colonies: &mut Query<(
        Entity,
        &Colony,
        Option<&Production>,
        Option<&mut BuildQueue>,
        Option<&Buildings>,
        Option<&mut BuildingQueue>,
        Option<&MaintenanceCost>,
        Option<&FoodConsumption>,
    )>,
    system_stockpiles: &mut Query<
        (&mut ResourceStockpile, Option<&ResourceCapacity>),
        With<StarSystem>,
    >,
    stars: &Query<(Entity, &StarSystem, &Position, Option<&SystemAttributes>)>,
    command_queues: &Query<&mut CommandQueue>,
    planets: &Query<&Planet>,
    pending_commands: &Query<&PendingShipCommand>,
    hull_registry: &crate::ship_design::HullRegistry,
    module_registry: &crate::ship_design::ModuleRegistry,
    design_registry: &ShipDesignRegistry,
    clock_elapsed: i64,
    roe_query: &Query<&RulesOfEngagement>,
    positions: &Query<&Position>,
    player_stationed: Option<Entity>,
    player_aboard_ship: Option<Entity>,
    courier_routes: &Query<&CourierRoute>,
    selected_system: Option<Entity>,
    fleet_members: &Query<&crate::ship::FleetMembers>,
    fleets: &Query<&crate::ship::Fleet>,
    nearby_structures: &[NearbyStructure],
    ship_stats: &Query<&ShipStats>,
    docked_at_query: &Query<(Entity, &DockedAt)>,
    docked_check: &Query<&DockedAt>,
    hull_reg: &HullRegistry,
    ship_modifiers_query: &Query<&ShipModifiers>,
) -> ShipPanelActions {
    // Collect ship data into locals first, then draw UI, then apply mutations
    let ship_data = selected_ship.0.and_then(|ship_entity| {
        let (_, ship, state, cargo, ship_hp, survey_data) = ships_query.get(ship_entity).ok()?;
        let docked_system = if let ShipState::InSystem { system } = &*state {
            Some(*system)
        } else {
            None
        };
        let cargo_data = cargo.as_ref().map(|c| (c.minerals, c.energy));
        // #229: Clone the item list out so we can render Deploy buttons per
        // item without re-borrowing cargo mutably.
        let cargo_items: Vec<crate::ship::CargoItem> =
            cargo.map(|c| c.items.clone()).unwrap_or_default();
        let status_info = build_status_info(&state, clock, stars);
        let queued_cmds: Vec<String> = command_queues
            .get(ship_entity)
            .ok()
            .map(|q| {
                q.commands
                    .iter()
                    .map(|cmd| format_queued_command(cmd, stars))
                    .collect()
            })
            .unwrap_or_default();
        let home_port = ship.home_port;
        let home_port_name = stars
            .get(home_port)
            .map(|(_, s, _, _)| s.name.clone())
            .unwrap_or_else(|_| "Unknown".to_string());
        let maintenance_cost = design_registry.maintenance(&ship.design_id);
        // Check if docked at a system that has a colony (for "Set Home Port" button)
        let docked_at_colony = docked_system.and_then(|dock_sys| {
            colonies.iter().find_map(|(_, col, _, _, _, _, _, _)| {
                if col.system(planets) == Some(dock_sys) {
                    Some(dock_sys)
                } else {
                    None
                }
            })
        });
        // Check if ship is in a cancellable state (surveying or settling)
        let is_cancellable = matches!(
            &*state,
            ShipState::Surveying { .. } | ShipState::Settling { .. }
        );
        // #103: Check if ship carries unreported survey data
        let has_survey_data = survey_data.is_some();
        let survey_data_system = survey_data.map(|sd| sd.system_name.clone());
        // Collect pending commands for this ship
        let pending_info: Option<i64> = pending_commands
            .iter()
            .filter(|pc| pc.ship == ship_entity)
            .map(|pc| pc.arrives_at)
            .min();
        // #98: Collect hull_id and modules for refit UI
        let ship_hull_id = ship.hull_id.clone();
        let ship_modules: Vec<crate::ship::EquippedModule> = ship.modules.clone();
        // #98: Is the ship refitting?
        let is_refitting = matches!(&*state, ShipState::Refitting { .. });
        // #123: Pre-compute design-based refit eligibility / cost / time so the
        // panel can present a one-button "Apply Refit" once the design has
        // moved ahead of the ship.
        let refit_info: Option<RefitInfo> = (|| {
            let design = design_registry.get(&ship.design_id)?;
            if design.revision <= ship.design_revision {
                return None;
            }
            let hull = hull_registry.get(&ship.hull_id)?;
            let (cost_m, cost_e, time) = crate::ship_design::refit_cost_to_design(
                &ship.modules,
                design,
                hull,
                module_registry,
            );
            Some(RefitInfo {
                target_revision: design.revision,
                current_revision: ship.design_revision,
                design_name: design.name.clone(),
                cost_minerals: cost_m,
                cost_energy: cost_e,
                refit_time: time,
            })
        })();
        // #123: Fleet membership + per-fleet refit summary (if applicable).
        // #287 (γ-1): membership is read from `Ship.fleet` and the peer
        // `FleetMembers` component on the fleet entity.
        let fleet_refit_summary: Option<FleetRefitSummary> = ship.fleet.and_then(|fleet_entity| {
            let fleet = fleets.get(fleet_entity).ok()?;
            let members = fleet_members.get(fleet_entity).ok()?;
            let mut eligible = 0usize;
            let mut total_m = Amt::ZERO;
            let mut total_e = Amt::ZERO;
            let mut max_time: i64 = 0;
            for member in members.iter() {
                let Ok((_, m_ship, m_state, _, _, _)) = ships_query.get(*member) else {
                    continue;
                };
                let Some(m_design) = design_registry.get(&m_ship.design_id) else {
                    continue;
                };
                if m_design.revision <= m_ship.design_revision {
                    continue;
                }
                if !matches!(&*m_state, ShipState::InSystem { .. }) {
                    continue;
                }
                let Some(m_hull) = hull_registry.get(&m_ship.hull_id) else {
                    continue;
                };
                let (cm, ce, t) = crate::ship_design::refit_cost_to_design(
                    &m_ship.modules,
                    m_design,
                    m_hull,
                    module_registry,
                );
                eligible += 1;
                total_m = total_m.add(cm);
                total_e = total_e.add(ce);
                if t > max_time {
                    max_time = t;
                }
            }
            Some(FleetRefitSummary {
                fleet_entity,
                fleet_name: fleet.name.clone(),
                eligible_count: eligible,
                total_cost_minerals: total_m,
                total_cost_energy: total_e,
                max_refit_time: max_time,
            })
        });
        // #57: Current ROE
        let current_roe = roe_query.get(ship_entity).copied().unwrap_or_default();
        // #57: Command delay for ROE changes
        let roe_command_delay: i64 = {
            // Determine the system the ship is at (or heading to)
            let ship_system = docked_system.or_else(|| match &*state {
                ShipState::SubLight { target_system, .. } => *target_system,
                ShipState::InFTL {
                    destination_system, ..
                } => Some(*destination_system),
                ShipState::Surveying { target_system, .. } => Some(*target_system),
                ShipState::Settling { system, .. } => Some(*system),
                _ => None,
            });
            player_stationed
                .and_then(|player_sys| {
                    let player_pos = positions.get(player_sys).ok()?;
                    let ship_sys = ship_system?;
                    let ship_pos = positions.get(ship_sys).ok()?;
                    let dist = crate::physics::distance_ly(player_pos, ship_pos);
                    Some(crate::physics::light_delay_hexadies(dist))
                })
                .unwrap_or(0)
        };
        // #59: Player aboard this ship?
        let is_player_aboard = ship.player_aboard;
        // #59: Can player board this ship? (ship docked at player's system, player not aboard any ship)
        let can_board = !is_player_aboard
            && player_aboard_ship.is_none()
            && docked_system.is_some()
            && docked_system == player_stationed;
        // #59: Can player disembark? (player aboard this ship and ship is docked)
        let can_disembark = is_player_aboard && docked_system.is_some();
        // #389: Harbour capacity and docked ship info
        let (harbour_capacity, harbour_docked_size, harbour_docked_ships) = ship_stats
            .get(ship_entity)
            .ok()
            .filter(|s| s.harbour_capacity.cached().raw() > 0)
            .map(|s| {
                let cap = (s.harbour_capacity.cached().raw() / 1000) as u32;
                let mut used: u32 = 0;
                let mut names = Vec::new();
                for (docked_entity, da) in docked_at_query.iter() {
                    if da.0 == ship_entity {
                        if let Ok((_, docked_ship, _, _, _, _)) = ships_query.get(docked_entity) {
                            let sz = hull_reg
                                .get(&docked_ship.hull_id)
                                .map(|h| h.size)
                                .unwrap_or(1);
                            used = used.saturating_add(sz);
                            names.push(docked_ship.name.clone());
                        }
                    }
                }
                (cap, used, names)
            })
            .unwrap_or((0, 0, Vec::new()));
        // #389: Check if this ship is docked at a harbour
        let docked_at_harbour: Option<(Entity, String)> =
            docked_check.get(ship_entity).ok().and_then(|da| {
                ships_query
                    .get(da.0)
                    .ok()
                    .map(|(_, h_ship, _, _, _, _)| (da.0, h_ship.name.clone()))
            });
        Some(ShipPanelData {
            ship_entity,
            name: ship.name.clone(),
            design_id: ship.design_id.clone(),
            hull_hp: ship_hp.hull,
            hull_max: ship_hp.hull_max,
            armor: ship_hp.armor,
            armor_max: ship_hp.armor_max,
            shield: ship_hp.shield,
            shield_max: ship_hp.shield_max,
            ftl_range: ship.ftl_range,
            sublight_speed: ship.sublight_speed,
            status_info,
            docked_system,
            cargo_data,
            cargo_items,
            queued_cmds,
            _home_port: home_port,
            home_port_name,
            maintenance_cost,
            docked_at_colony,
            is_cancellable,
            pending_arrives_at: pending_info,
            has_survey_data,
            survey_data_system,
            _ship_hull_id: ship_hull_id,
            ship_modules,
            is_refitting,
            refit_info,
            fleet_refit_summary,
            current_roe,
            roe_command_delay,
            is_player_aboard,
            can_board,
            can_disembark,
            harbour_capacity,
            harbour_docked_size,
            harbour_docked_ships,
            docked_at_harbour,
            mod_speed: ship_modifiers_query
                .get(ship_entity)
                .ok()
                .map(|m| m.speed.value().clone()),
            mod_ftl_range: ship_modifiers_query
                .get(ship_entity)
                .ok()
                .map(|m| m.ftl_range.value().clone()),
            mod_attack: ship_modifiers_query
                .get(ship_entity)
                .ok()
                .map(|m| m.attack.value().clone()),
            mod_defense: ship_modifiers_query
                .get(ship_entity)
                .ok()
                .map(|m| m.defense.value().clone()),
            mod_evasion: ship_modifiers_query
                .get(ship_entity)
                .ok()
                .map(|m| m.evasion.value().clone()),
            mod_armor_max: ship_modifiers_query
                .get(ship_entity)
                .ok()
                .map(|m| m.armor_max.value().clone()),
            mod_shield_max: ship_modifiers_query
                .get(ship_entity)
                .ok()
                .map(|m| m.shield_max.value().clone()),
        })
    });

    let Some(ShipPanelData {
        ship_entity,
        name,
        design_id,
        hull_hp,
        hull_max,
        armor,
        armor_max,
        shield,
        shield_max,
        ftl_range,
        sublight_speed,
        status_info,
        docked_system,
        cargo_data,
        cargo_items,
        queued_cmds,
        _home_port: _,
        home_port_name,
        maintenance_cost,
        docked_at_colony,
        is_cancellable,
        pending_arrives_at,
        has_survey_data,
        survey_data_system,
        _ship_hull_id: _,
        ship_modules,
        is_refitting,
        refit_info,
        fleet_refit_summary,
        current_roe,
        roe_command_delay,
        is_player_aboard,
        can_board,
        can_disembark,
        harbour_capacity,
        harbour_docked_size,
        harbour_docked_ships,
        docked_at_harbour,
        mod_speed,
        mod_ftl_range,
        mod_attack,
        mod_defense,
        mod_evasion,
        mod_armor_max,
        mod_shield_max,
    }) = ship_data
    else {
        return ShipPanelActions::default();
    };

    let mut deselect_ship = false;
    let mut set_home_port: Option<Entity> = None;
    let mut actions = ShipPanelActions::default();

    // Cargo load/unload actions to apply after UI drawing
    #[derive(Default)]
    struct CargoAction {
        load_minerals: crate::amount::Amt,
        load_energy: crate::amount::Amt,
        unload_minerals: crate::amount::Amt,
        unload_energy: crate::amount::Amt,
    }
    let mut cargo_action = CargoAction::default();
    // Entity of the system at the dock (for cargo transfers via system stockpile)
    let system_entity_at_dock: Option<Entity> = docked_system.and_then(|dock_sys| {
        // Check if there's a colony at this system
        let has_colony = colonies
            .iter()
            .any(|(_, col, _, _, _, _, _, _)| col.system(planets) == Some(dock_sys));
        if has_colony { Some(dock_sys) } else { None }
    });

    let default_pos = {
        let rect = ctx.screen_rect();
        egui::pos2(rect.max.x - 270.0, rect.max.y - 130.0)
    };
    egui::Window::new("Selected Ship")
        .default_pos(default_pos)
        .resizable(false)
        .collapsible(true)
        .show(ctx, |ui| {
            ui.label(
                egui::RichText::new(format!("Ship: {}", name))
                    .strong()
                    .color(egui::Color32::from_rgb(100, 200, 255)),
            );
            let design_display_name = design_registry
                .get(&design_id)
                .map(|d| d.name.as_str())
                .unwrap_or(&design_id);
            ui.label(format!("Type: {}", design_display_name));
            // #59: Player aboard indicator
            if is_player_aboard {
                ui.label(
                    egui::RichText::new("[Player Aboard]")
                        .color(egui::Color32::from_rgb(50, 255, 50))
                        .strong(),
                );
            }
            ui.label(format!("Hull: {:.0}/{:.0}", hull_hp, hull_max));
            if armor_max > 0.0 {
                let response = ui.label(format!("Armor: {:.0}/{:.0}", armor, armor_max));
                if let Some(ref mv) = mod_armor_max {
                    if !mv.modifiers().is_empty() {
                        response.on_hover_ui(|tooltip_ui| {
                            draw_modifier_breakdown(
                                tooltip_ui,
                                "Armor (max)",
                                mv,
                                &|a| format!("{:.1}", a.to_f64()),
                                Some(clock_elapsed),
                            );
                        });
                    }
                }
            }
            if shield_max > 0.0 {
                let response = ui.label(format!("Shield: {:.0}/{:.0}", shield, shield_max));
                if let Some(ref mv) = mod_shield_max {
                    if !mv.modifiers().is_empty() {
                        response.on_hover_ui(|tooltip_ui| {
                            draw_modifier_breakdown(
                                tooltip_ui,
                                "Shield (max)",
                                mv,
                                &|a| format!("{:.1}", a.to_f64()),
                                Some(clock_elapsed),
                            );
                        });
                    }
                }
            }

            // #62: Detailed status with progress bar
            ui.label(&status_info.label);
            if let Some((elapsed, total, fraction)) = status_info.progress {
                ui.add(
                    egui::ProgressBar::new(fraction)
                        .text(format!("{}/{} hd", elapsed, total))
                        .desired_width(200.0),
                );
            }

            // #103: Show indicator if ship carries unreported survey data
            if has_survey_data {
                let sys_name = survey_data_system.as_deref().unwrap_or("unknown");
                ui.label(
                    egui::RichText::new(format!("Carrying survey data: {}", sys_name))
                        .color(egui::Color32::from_rgb(255, 200, 50)),
                );
            }

            if ftl_range > 0.0 {
                if let Some(ref mv) = mod_ftl_range {
                    modified_value_label_with_tooltip(
                        ui,
                        "FTL range",
                        mv,
                        |a| format!("{:.1} ly", a.to_f64()),
                        Some(clock_elapsed),
                    );
                } else {
                    ui.label(format!("FTL range: {:.1} ly", ftl_range));
                }
            } else {
                ui.label("No FTL capability");
            }
            if let Some(ref mv) = mod_speed {
                modified_value_label_with_tooltip(
                    ui,
                    "Sub-light speed",
                    mv,
                    |a| format!("{:.0}% c", a.to_f64() * 100.0),
                    Some(clock_elapsed),
                );
            } else {
                ui.label(format!("Sub-light speed: {:.0}% c", sublight_speed * 100.0));
            }

            if let Some(ref mv) = mod_attack {
                if mv.final_value() > Amt::ZERO || !mv.modifiers().is_empty() {
                    modified_value_label_with_tooltip(
                        ui,
                        "Attack",
                        mv,
                        |a| a.display(),
                        Some(clock_elapsed),
                    );
                }
            }
            if let Some(ref mv) = mod_defense {
                if mv.final_value() > Amt::ZERO || !mv.modifiers().is_empty() {
                    modified_value_label_with_tooltip(
                        ui,
                        "Defense",
                        mv,
                        |a| a.display(),
                        Some(clock_elapsed),
                    );
                }
            }
            if let Some(ref mv) = mod_evasion {
                if mv.final_value() > Amt::ZERO || !mv.modifiers().is_empty() {
                    modified_value_label_with_tooltip(
                        ui,
                        "Evasion",
                        mv,
                        |a| a.display(),
                        Some(clock_elapsed),
                    );
                }
            }

            // #64: Home port and maintenance info
            ui.separator();
            ui.label(format!("Home Port: {}", home_port_name));
            ui.label(format!(
                "Maintenance: {} E/hd (charged to {})",
                maintenance_cost.display_compact(),
                home_port_name
            ));

            // #389: Undock button when docked at a harbour
            if let Some((_, ref harbour_name)) = docked_at_harbour {
                ui.label(
                    egui::RichText::new(format!("Docked at harbour: {}", harbour_name))
                        .color(egui::Color32::from_rgb(255, 215, 80)),
                );
                if ui.button("Undock").clicked() {
                    actions.undock = Some(ship_entity);
                }
            }

            // #389: Harbour capacity & docked ships
            if harbour_capacity > 0 {
                ui.separator();
                ui.label(
                    egui::RichText::new("Harbour")
                        .strong()
                        .color(egui::Color32::from_rgb(255, 215, 80)),
                );
                ui.label(format!(
                    "Capacity: {} / {}",
                    harbour_docked_size, harbour_capacity
                ));
                if harbour_docked_ships.is_empty() {
                    ui.label(egui::RichText::new("(no ships docked)").small().weak());
                } else {
                    for docked_name in &harbour_docked_ships {
                        ui.label(format!("  - {}", docked_name));
                    }
                }
            }

            // #57: Rules of Engagement selector
            ui.separator();
            ui.horizontal(|ui| {
                ui.label("ROE:");
                for roe_option in RulesOfEngagement::ALL {
                    let is_selected = current_roe == roe_option;
                    let label = roe_option.label();
                    if ui.selectable_label(is_selected, label).clicked() && !is_selected {
                        actions.set_roe = Some((ship_entity, roe_option, roe_command_delay));
                    }
                }
            });
            if roe_command_delay > 0 {
                ui.label(
                    egui::RichText::new(format!("ROE change delay: {} hd", roe_command_delay))
                        .small()
                        .color(egui::Color32::from_rgb(255, 200, 100)),
                );
            }

            // #99: Pending command in transit display
            if let Some(arrives_at) = pending_arrives_at {
                let remaining = (arrives_at - clock.elapsed).max(0);
                ui.label(
                    egui::RichText::new(format!(
                        "Command in transit... arrives in {} hd",
                        remaining
                    ))
                    .color(egui::Color32::from_rgb(255, 191, 0)),
                );
            }

            // #99: Cancel current action button (surveying/settling)
            if is_cancellable {
                if ui.button("Cancel Current Action").clicked() {
                    actions.cancel_current = true;
                }
            }

            // #62/#99: Command queue display with cancel buttons
            if !queued_cmds.is_empty() {
                ui.separator();
                ui.label(egui::RichText::new("Command Queue").strong());
                for (i, cmd_str) in queued_cmds.iter().enumerate() {
                    ui.horizontal(|ui| {
                        if ui.small_button("X").clicked() {
                            actions.cancel_command_index = Some(i);
                        }
                        ui.label(format!("{}. {}", i + 1, cmd_str));
                    });
                }
                if ui.button("Clear All").clicked() {
                    actions.clear_commands = true;
                }
            }

            ui.label(
                egui::RichText::new("Click a star to issue commands")
                    .weak()
                    .italics(),
            );

            // Cargo section for Courier ships docked at a colony
            if let Some(_docked_system) = docked_system {
                if design_id == "courier_mk1" {
                    if let Some((cargo_m, cargo_e)) = cargo_data {
                        ui.separator();
                        ui.label(egui::RichText::new("Cargo").strong());
                        ui.label(format!("Minerals: {}", cargo_m.display_compact()));
                        ui.label(format!("Energy: {}", cargo_e.display_compact()));

                        if system_entity_at_dock.is_some() {
                            ui.horizontal(|ui| {
                                if ui.button("Load M +100").clicked() {
                                    cargo_action.load_minerals = crate::amount::Amt::units(100);
                                }
                                if ui.button("Load E +100").clicked() {
                                    cargo_action.load_energy = crate::amount::Amt::units(100);
                                }
                            });
                            ui.horizontal(|ui| {
                                if ui.button("Unload M").clicked() {
                                    cargo_action.unload_minerals = cargo_m;
                                }
                                if ui.button("Unload E").clicked() {
                                    cargo_action.unload_energy = cargo_e;
                                }
                            });
                        }
                    }
                }
            }

            // #229: Deliverable pipeline actions — shown whenever the ship
            // either carries a deployable item or has a nearby structure
            // to interact with. Positioned directly after the Cargo section
            // per the #229 UX design.
            let has_items = !cargo_items.is_empty();
            let platforms: Vec<&NearbyStructure> =
                nearby_structures.iter().filter(|s| s.is_platform).collect();
            let scrapyards: Vec<&NearbyStructure> = nearby_structures
                .iter()
                .filter(|s| s.is_scrapyard)
                .collect();

            if has_items || !platforms.is_empty() || !scrapyards.is_empty() {
                ui.separator();
                ui.label(
                    egui::RichText::new("Deliverable Actions")
                        .strong()
                        .color(egui::Color32::from_rgb(180, 220, 180)),
                );

                // --- Cargo items with Deploy buttons ---
                if has_items {
                    ui.label(egui::RichText::new("Carried items").small());
                    for (i, item) in cargo_items.iter().enumerate() {
                        let def_id = item.definition_id();
                        ui.horizontal(|ui| {
                            ui.label(format!("  #{}: {}", i, def_id));
                            if ui.small_button("Deploy").clicked() {
                                actions.deploy_mode_request = Some((ship_entity, i));
                            }
                        });
                    }
                    ui.label(
                        egui::RichText::new(
                            "Click Deploy then click a star to place the structure.",
                        )
                        .small()
                        .italics()
                        .weak(),
                    );
                }

                // --- Transfer Resources to ConstructionPlatform ---
                if !platforms.is_empty() {
                    ui.separator();
                    ui.label(egui::RichText::new("Transfer to platform").small());
                    // Amounts applied by ALL transfer buttons this frame. Simple
                    // fixed steps — fine for V1. A slider UI is future work.
                    let step_m = Amt::units(100);
                    let step_e = Amt::units(100);
                    let (have_m, have_e) = cargo_data.unwrap_or((Amt::ZERO, Amt::ZERO));
                    for p in &platforms {
                        ui.horizontal(|ui| {
                            ui.label(format!("  {} ({:.2} ly)", p.name, p.distance_ly,));
                            let can_m = have_m > Amt::ZERO;
                            if ui.add_enabled(can_m, egui::Button::new("+100 M")).clicked() {
                                let m = step_m.min(have_m);
                                actions.transfer_request =
                                    Some((ship_entity, p.entity, m, Amt::ZERO));
                            }
                            let can_e = have_e > Amt::ZERO;
                            if ui.add_enabled(can_e, egui::Button::new("+100 E")).clicked() {
                                let e = step_e.min(have_e);
                                actions.transfer_request =
                                    Some((ship_entity, p.entity, Amt::ZERO, e));
                            }
                        });
                    }
                }

                // --- Load from Scrapyard ---
                if !scrapyards.is_empty() {
                    ui.separator();
                    ui.label(egui::RichText::new("Salvage from scrapyard").small());
                    for s in &scrapyards {
                        ui.horizontal(|ui| {
                            ui.label(format!("  {} ({:.2} ly)", s.name, s.distance_ly,));
                            if ui.button("Load").clicked() {
                                actions.load_from_scrapyard_request = Some((ship_entity, s.entity));
                            }
                        });
                    }
                }
            }

            // #117: Courier route automation panel (couriers only)
            if design_id == "courier_mk1" {
                ui.separator();
                ui.label(egui::RichText::new("Courier Route").strong());
                let route_opt = courier_routes.get(ship_entity).ok();
                let current_mode = route_opt
                    .map(|r| r.mode)
                    .unwrap_or(CourierMode::ResourceTransport);

                // Mode selector
                ui.horizontal(|ui| {
                    ui.label("Mode:");
                    for mode in [
                        CourierMode::ResourceTransport,
                        CourierMode::KnowledgeRelay,
                        CourierMode::MessageDelivery,
                    ] {
                        let selected = current_mode == mode;
                        if ui.selectable_label(selected, mode.label()).clicked() && !selected {
                            actions.courier_set_mode = Some((ship_entity, mode));
                        }
                    }
                });

                // Waypoints list
                if let Some(route) = route_opt {
                    if route.waypoints.is_empty() {
                        ui.label("(no waypoints)");
                    } else {
                        for (i, wp) in route.waypoints.iter().enumerate() {
                            let name = system_name(*wp, stars);
                            let marker = if i == route.current_index { "->" } else { "  " };
                            ui.label(format!("{} {}. {}", marker, i + 1, name));
                        }
                    }
                    ui.horizontal(|ui| {
                        let label = if route.paused {
                            "Resume Route"
                        } else {
                            "Pause Route"
                        };
                        if ui.button(label).clicked() {
                            actions.courier_toggle_pause = Some(ship_entity);
                        }
                        if ui.button("Stop Route").clicked() {
                            actions.courier_clear_route = Some(ship_entity);
                        }
                    });
                } else {
                    ui.label("(no active route)");
                }

                // Add waypoint button (uses current selection)
                if let Some(sel_sys) = selected_system {
                    let sel_name = system_name(sel_sys, stars);
                    let label = format!("Add waypoint: {}", sel_name);
                    if ui.button(label).clicked() {
                        actions.courier_add_waypoint = Some((ship_entity, sel_sys, current_mode));
                    }
                } else {
                    ui.label(
                        egui::RichText::new("Select a star to add it as a waypoint")
                            .small()
                            .weak(),
                    );
                }
            }

            // #59: Board / Disembark buttons
            if can_board {
                if ui.button("Board Ship").clicked() {
                    actions.board_ship = Some(ship_entity);
                }
            }
            if can_disembark {
                if ui.button("Disembark").clicked() {
                    actions.disembark = true;
                }
            }

            // #123: Design-based Refit UI. The ship is refit-eligible when its
            // recorded `design_revision` is behind the current registered
            // ShipDesignDefinition (i.e. somebody edited the design via the
            // Ship Designer). Module selection lives in the Ship Designer
            // exclusively — this panel only shows the cost summary and
            // dispatches the apply.
            if !is_refitting {
                if let Some(info) = refit_info.as_ref() {
                    ui.separator();
                    ui.label(egui::RichText::new("Refit Available").strong());
                    let revisions_behind =
                        info.target_revision.saturating_sub(info.current_revision);
                    ui.label(
                        egui::RichText::new(format!(
                            "Design '{}' updated ({} revision{} behind)",
                            info.design_name,
                            revisions_behind,
                            if revisions_behind == 1 { "" } else { "s" },
                        ))
                        .small()
                        .italics(),
                    );
                    ui.label(format!(
                        "Refit cost: M:{} E:{} | {} hd",
                        info.cost_minerals.display_compact(),
                        info.cost_energy.display_compact(),
                        info.refit_time
                    ));
                    if let Some(dock_system) = docked_at_colony {
                        if ui.button("Apply Refit").clicked() {
                            actions.refit = Some(ShipRefitAction {
                                ship_entity,
                                system_entity: dock_system,
                            });
                        }
                    } else {
                        ui.add_enabled(false, egui::Button::new("Apply Refit"))
                            .on_disabled_hover_text(
                                "Ship must be docked at a colony to apply refit.",
                            );
                    }
                }
            } else {
                ui.separator();
                ui.label(
                    egui::RichText::new("Refitting in progress...")
                        .color(egui::Color32::from_rgb(255, 220, 80)),
                );
            }

            // #123: Fleet-wide refit button (only when the ship belongs to a
            // fleet that has at least one refit-eligible docked member).
            if let Some(ref summary) = fleet_refit_summary {
                if summary.eligible_count > 0 {
                    ui.separator();
                    ui.label(
                        egui::RichText::new(format!("Fleet '{}'", summary.fleet_name)).strong(),
                    );
                    ui.label(format!(
                        "{} member(s) refit-eligible | M:{} E:{} | up to {} hd",
                        summary.eligible_count,
                        summary.total_cost_minerals.display_compact(),
                        summary.total_cost_energy.display_compact(),
                        summary.max_refit_time,
                    ));
                    if ui.button("Apply Refit to Fleet").clicked() {
                        actions.fleet_refit = Some(FleetRefitAction {
                            fleet_entity: summary.fleet_entity,
                        });
                    }
                }
            }

            // #64: Set Home Port button (only when docked at a colony)
            if let Some(dock_system) = docked_at_colony {
                if ui.button("Set Home Port").clicked() {
                    set_home_port = Some(dock_system);
                }
            }

            // #79: Scrap Ship button (only when docked at a colony)
            if let Some(dock_system) = docked_at_colony {
                let (refund_m, refund_e) =
                    design_registry.scrap_refund(&design_id, &ship_modules, module_registry);
                let scrap_label = format!("Scrap Ship (+{} M, +{} E)", refund_m, refund_e);
                let response = ui.button(&scrap_label).on_hover_text(
                    "Dismantle this ship and recover 50% of total value (hull + modules)",
                );
                if response.clicked() {
                    // Use system entity for stockpile refund
                    if let Some(sys_e) = system_entity_at_dock {
                        let system_name = stars
                            .get(dock_system)
                            .map(|(_, s, _, _)| s.name.clone())
                            .unwrap_or_else(|_| "Unknown".to_string());
                        actions.scrap = Some(ShipScrapAction {
                            ship_entity,
                            colony_entity: sys_e,
                            ship_name: name.clone(),
                            system_name,
                            minerals_refund: refund_m,
                            energy_refund: refund_e,
                        });
                    }
                }
            }

            if ui.button("Deselect ship").clicked() {
                deselect_ship = true;
            }
        });

    // Apply deselect
    if deselect_ship {
        selected_ship.0 = None;
    }

    // #64: Apply home port change
    if let Some(new_home_port) = set_home_port {
        if let Ok((_, mut ship, _, _, _, _)) = ships_query.get_mut(ship_entity) {
            ship.home_port = new_home_port;
        }
    }

    // Apply cargo load/unload actions
    let has_cargo_action = cargo_action.load_minerals > Amt::ZERO
        || cargo_action.load_energy > Amt::ZERO
        || cargo_action.unload_minerals > Amt::ZERO
        || cargo_action.unload_energy > Amt::ZERO;
    if has_cargo_action {
        if let Some(sys_e) = system_entity_at_dock {
            if let Ok((mut stockpile, _)) = system_stockpiles.get_mut(sys_e) {
                if let Ok((_, _, _, Some(mut cargo), _, _)) = ships_query.get_mut(ship_entity) {
                    if cargo_action.load_minerals > Amt::ZERO {
                        let transfer = cargo_action.load_minerals.min(stockpile.minerals);
                        stockpile.minerals = stockpile.minerals.sub(transfer);
                        cargo.minerals = cargo.minerals.add(transfer);
                    }
                    if cargo_action.load_energy > Amt::ZERO {
                        let transfer = cargo_action.load_energy.min(stockpile.energy);
                        stockpile.energy = stockpile.energy.sub(transfer);
                        cargo.energy = cargo.energy.add(transfer);
                    }
                    if cargo_action.unload_minerals > Amt::ZERO {
                        let transfer = cargo_action.unload_minerals.min(cargo.minerals);
                        cargo.minerals = cargo.minerals.sub(transfer);
                        stockpile.minerals = stockpile.minerals.add(transfer);
                    }
                    if cargo_action.unload_energy > Amt::ZERO {
                        let transfer = cargo_action.unload_energy.min(cargo.energy);
                        cargo.energy = cargo.energy.sub(transfer);
                        stockpile.energy = stockpile.energy.add(transfer);
                    }
                }
            }
        }
    }

    // If scrapping, clear selection (despawn handled in draw_all_ui)
    if actions.scrap.is_some() {
        selected_ship.0 = None;
    }

    actions
}

#[cfg(test)]
mod tests_229 {
    use super::*;
    use crate::amount::Amt;
    use crate::ship::QueuedCommand;

    fn placeholder_entity() -> Entity {
        // Entity::PLACEHOLDER is stable across frames and fine for string
        // formatting — the formatter never reads the entity.
        Entity::PLACEHOLDER
    }

    #[test]
    fn format_load_deliverable_shows_index() {
        let cmd = QueuedCommand::LoadDeliverable {
            system: placeholder_entity(),
            stockpile_index: 3,
        };
        assert_eq!(
            format_deliverable_command(&cmd).unwrap(),
            "Load deliverable #3"
        );
    }

    #[test]
    fn format_deploy_deliverable_shows_coords_and_item_index() {
        let cmd = QueuedCommand::DeployDeliverable {
            position: [12.3, -4.5, 0.0],
            item_index: 1,
        };
        // Matches the one-decimal formatting used by other coordinate displays.
        assert_eq!(
            format_deliverable_command(&cmd).unwrap(),
            "Deploy #1 -> (12.3, -4.5, 0.0)"
        );
    }

    #[test]
    fn format_transfer_shows_amounts() {
        let cmd = QueuedCommand::TransferToStructure {
            structure: placeholder_entity(),
            minerals: Amt::units(100),
            energy: Amt::units(25),
        };
        let s = format_deliverable_command(&cmd).unwrap();
        assert!(s.contains("100"));
        assert!(s.contains("25"));
        assert!(s.contains("structure"));
    }

    #[test]
    fn format_load_from_scrapyard_simple() {
        let cmd = QueuedCommand::LoadFromScrapyard {
            structure: placeholder_entity(),
        };
        assert_eq!(
            format_deliverable_command(&cmd).unwrap(),
            "Salvage scrapyard"
        );
    }

    #[test]
    fn format_deliverable_none_for_non_deliverable_cmds() {
        // Non-deliverable variants fall through to the stars-based branch.
        let cmd = QueuedCommand::MoveTo {
            system: placeholder_entity(),
        };
        assert!(format_deliverable_command(&cmd).is_none());
    }
}
