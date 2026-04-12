use bevy::prelude::*;
use bevy::ecs::system::SystemParam;

use crate::colony::{
    ColonizationQueue, ResourceCapacity, ResourceStockpile, SystemBuildingQueue, SystemBuildings,
};
use crate::components::Position;
use crate::galaxy::{Anomalies, HostilePresence, Planet, StarSystem, SystemAttributes};
use crate::ship::{CourierRoute, Fleet, FleetMembership, PendingShipCommand, RulesOfEngagement};
use crate::ship_design::{HullRegistry, ModuleRegistry, ShipDesignRegistry};
use crate::visualization::{ContextMenu, SelectedPlanet, SelectedShip, SelectedSystem};

#[derive(SystemParam)]
pub struct MainPanelWorldQueries<'w, 's> {
    pub positions: Query<'w, 's, &'static Position>,
    pub planets: Query<'w, 's, &'static Planet>,
    pub planet_entities: Query<'w, 's, (Entity, &'static Planet, Option<&'static SystemAttributes>)>,
    pub stockpiles: Query<'w, 's, (&'static mut ResourceStockpile, Option<&'static ResourceCapacity>), With<StarSystem>>,
    pub system_buildings: Query<'w, 's, (Option<&'static mut SystemBuildings>, Option<&'static mut SystemBuildingQueue>)>,
    pub colonization_queues: Query<'w, 's, &'static ColonizationQueue>,
    pub roe: Query<'w, 's, &'static RulesOfEngagement>,
    pub hostile_presence: Query<'w, 's, &'static HostilePresence>,
    pub pending_commands: Query<'w, 's, &'static PendingShipCommand>,
    pub anomalies: Query<'w, 's, &'static Anomalies>,
    /// #117: Courier route data for ship panel display.
    pub courier_routes: Query<'w, 's, &'static CourierRoute>,
    /// #123: Fleets and their memberships, used by the design-based refit
    /// panel to compute fleet-wide refit summaries.
    pub fleets: Query<'w, 's, &'static Fleet>,
    pub fleet_memberships: Query<'w, 's, &'static FleetMembership>,
}

#[derive(SystemParam)]
pub struct MainPanelSelection<'w> {
    pub selected_system: ResMut<'w, SelectedSystem>,
    pub selected_ship: ResMut<'w, SelectedShip>,
    pub selected_planet: ResMut<'w, SelectedPlanet>,
    pub context_menu: ResMut<'w, ContextMenu>,
}

#[derive(SystemParam)]
pub struct MainPanelRegistries<'w> {
    pub hull_registry: Res<'w, HullRegistry>,
    pub module_registry: Res<'w, ModuleRegistry>,
    pub design_registry: Res<'w, ShipDesignRegistry>,
}
