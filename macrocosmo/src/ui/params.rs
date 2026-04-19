use bevy::ecs::system::SystemParam;
use bevy::prelude::*;

use crate::colony::{
    ColonizationQueue, ColonyJobRates, DeliverableStockpile, ResourceCapacity, ResourceStockpile,
    SystemBuildingQueue, SystemBuildings,
};
use crate::components::Position;
use crate::deep_space::{ConstructionPlatform, DeepSpaceStructure, Scrapyard};
use crate::faction::FactionOwner;
use crate::galaxy::{Anomalies, AtSystem, Hostile, Planet, StarSystem, SystemAttributes};
use crate::ship::{
    CoreShip, CourierRoute, DockedAt, Fleet, FleetMembers, PendingShipCommand, RulesOfEngagement,
    Ship, ShipModifiers, ShipStats,
};
use crate::ship_design::{HullRegistry, ModuleRegistry, ShipDesignRegistry};
use crate::species::{ColonyJobs, ColonyPopulation, JobRegistry};
use crate::visualization::{ContextMenu, SelectedPlanet, SelectedShip, SelectedSystem};

#[derive(SystemParam)]
pub struct MainPanelWorldQueries<'w, 's> {
    pub positions: Query<'w, 's, &'static Position>,
    pub planets: Query<'w, 's, &'static Planet>,
    pub planet_entities:
        Query<'w, 's, (Entity, &'static Planet, Option<&'static SystemAttributes>)>,
    pub stockpiles: Query<
        'w,
        's,
        (
            &'static mut ResourceStockpile,
            Option<&'static ResourceCapacity>,
        ),
        With<StarSystem>,
    >,
    pub system_buildings: Query<
        'w,
        's,
        (
            Option<&'static mut SystemBuildings>,
            Option<&'static mut SystemBuildingQueue>,
        ),
    >,
    pub colonization_queues: Query<'w, 's, &'static ColonizationQueue>,
    pub roe: Query<'w, 's, &'static RulesOfEngagement>,
    /// #293: Hostile entities — `(AtSystem, Option<FactionOwner>)` tuple
    /// keyed off the `Hostile` marker. UI readers build a hostile-systems
    /// HashSet filtered by FactionRelations at the call site. `FactionOwner`
    /// is optional because tests may spawn legacy hostiles before the
    /// backfill system runs.
    pub hostile_presence:
        Query<'w, 's, (&'static AtSystem, Option<&'static FactionOwner>), With<Hostile>>,
    pub pending_commands: Query<'w, 's, &'static PendingShipCommand>,
    pub anomalies: Query<'w, 's, &'static Anomalies>,
    /// #117: Courier route data for ship panel display.
    pub courier_routes: Query<'w, 's, &'static CourierRoute>,
    /// #123: Fleets and their members, used by the design-based refit
    /// panel to compute fleet-wide refit summaries. #287 (γ-1):
    /// membership is now expressed via `Ship.fleet` + the sibling
    /// `FleetMembers` component.
    pub fleets: Query<'w, 's, &'static Fleet>,
    pub fleet_members: Query<'w, 's, &'static FleetMembers>,
    /// #229: Deliverable stockpiles live on StarSystem entities. Populated once
    /// the system builds at least one deliverable via a shipyard.
    pub deliverable_stockpiles: Query<'w, 's, &'static DeliverableStockpile, With<StarSystem>>,
    /// #229: All deep-space structures plus their construction/scrapyard
    /// state. Used by the system panel structure list and the
    /// visualization overlay markers.
    pub deep_space_structures: Query<
        'w,
        's,
        (
            Entity,
            &'static DeepSpaceStructure,
            &'static Position,
            Option<&'static ConstructionPlatform>,
            Option<&'static Scrapyard>,
        ),
    >,
    /// #252: Read-only colony pop/job view for the Pop management tab on
    /// the colony detail panel. Separated from the main mutable `colonies`
    /// query in `draw_main_panels_system` to avoid B0001 conflicts and to
    /// keep that query's tuple arity stable.
    pub colony_pop_view: Query<
        'w,
        's,
        (
            Entity,
            Option<&'static ColonyPopulation>,
            Option<&'static ColonyJobs>,
            Option<&'static ColonyJobRates>,
        ),
    >,
    pub remote_commands: Query<'w, 's, &'static crate::communication::PendingCommand>,
    /// #370 + #299: Core ships — `(AtSystem, FactionOwner)` keyed off the
    /// `CoreShip` marker. Used for sovereignty / system building gate checks
    /// and the colonize-gate UI.
    pub core_ships:
        Query<'w, 's, (&'static AtSystem, &'static FactionOwner), With<crate::ship::CoreShip>>,
    /// #389: Ship stats — used for harbour capacity display.
    pub ship_stats: Query<'w, 's, &'static ShipStats>,
    /// #389: Docked-at relationships — used for harbour occupancy.
    /// NOTE: does NOT include `&Ship` — that would conflict (B0001) with
    /// the mutable `ships_query` in `draw_main_panels_system`. Look up
    /// ship names via `ships_query.get(entity)` instead.
    pub docked_at: Query<'w, 's, (Entity, &'static DockedAt)>,
    /// #389: Docked-at check — entity-indexed lookup for single ship.
    pub docked_check: Query<'w, 's, &'static DockedAt>,
    /// #391: Ship modifiers — used for modifier breakdown tooltips.
    pub ship_modifiers: Query<'w, 's, &'static ShipModifiers>,
}

#[derive(SystemParam)]
pub struct MainPanelSelection<'w> {
    pub selected_system: ResMut<'w, SelectedSystem>,
    pub selected_ship: ResMut<'w, SelectedShip>,
    pub selected_planet: ResMut<'w, SelectedPlanet>,
    pub context_menu: ResMut<'w, ContextMenu>,
    /// #390-T5: UI element registry for BRP introspection. `None` when the
    /// `remote` feature is not enabled (resource not inserted).
    pub ui_registry: Option<ResMut<'w, super::UiElementRegistry>>,
    /// #398: Observer mode read-only flag. When `read_only` is true, context
    /// menu and ship panel commands are suppressed.
    pub observer_mode: Res<'w, crate::observer::ObserverMode>,
}

/// #229: Deliverable-pipeline resources used by the main panels: the Lua-
/// loaded structure/deliverable registry and the cross-panel `DeployMode`
/// signal written by the ship panel's Deploy button and consumed by
/// `click_select_system` on the next star click. Flag queries use `Query`
/// (not `Single`) so the system still runs before the player empire
/// entity exists (e.g. very early startup frames).
#[derive(SystemParam)]
pub struct MainPanelDeliverableRes<'w, 's> {
    pub structure_registry: Res<'w, crate::deep_space::StructureRegistry>,
    pub deploy_mode: ResMut<'w, crate::visualization::DeployMode>,
    pub empire_flags: Query<
        'w,
        's,
        (
            &'static crate::technology::GameFlags,
            &'static crate::condition::ScopedFlags,
        ),
        With<crate::player::PlayerEmpire>,
    >,
    pub colony_dispatches: ResMut<'w, crate::communication::PendingColonyDispatches>,
}

#[derive(SystemParam)]
pub struct MainPanelRegistries<'w> {
    pub hull_registry: Res<'w, HullRegistry>,
    pub module_registry: Res<'w, ModuleRegistry>,
    pub design_registry: Res<'w, ShipDesignRegistry>,
    /// #252: Job registry — consumed by the colony panel Pop management tab.
    pub job_registry: Res<'w, JobRegistry>,
}
