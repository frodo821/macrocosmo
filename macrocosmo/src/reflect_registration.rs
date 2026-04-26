//! Reflect type-registration for every `Component` / `Resource` in the
//! macrocosmo crate. Installing [`ReflectRegistrationPlugin`] makes
//! these types queryable via the Bevy Remote Protocol
//! (`bevy/bevy_remote`, feature-gated `remote`) — without it,
//! `world.query` answers will be empty even though the types derive
//! `Reflect`.
//!
//! The plugin is `App`-side rather than spread per-plugin so the type
//! registration stays trivially auditable in one place. Adding a new
//! `#[reflect(Component)]` / `#[reflect(Resource)]` definition
//! anywhere in the crate is therefore a two-step change: derive
//! `Reflect` on the type, then add a `register_type::<...>()` line
//! here. CI does not enforce this — a missing entry only manifests as
//! a missing component in BRP responses.
//!
//! Types whose modules are themselves feature-gated (currently only
//! `crate::remote`) are gated here too with matching `#[cfg(feature =
//! …)]` attributes.

use bevy::prelude::*;

/// Bevy plugin that registers every reflectable `Component` /
/// `Resource` in the macrocosmo crate with the type registry. Install
/// once from `main.rs` (and from test apps that need BRP-style
/// introspection). Idempotent — re-registering a type is a no-op in
/// Bevy 0.18.
pub struct ReflectRegistrationPlugin;

impl Plugin for ReflectRegistrationPlugin {
    fn build(&self, app: &mut App) {
        register_all_types(app);
    }
}

/// Register every game-relevant `Component` / `Resource` type with
/// the Bevy type registry. Called once from
/// [`ReflectRegistrationPlugin::build`].
pub fn register_all_types(app: &mut App) {
    // crate::ai::assignments
    app.register_type::<crate::ai::assignments::PendingAssignment>();
    // crate::ai::command_consumer
    app.register_type::<crate::ai::command_consumer::PendingRulerBoarding>();
    // crate::ai::debug_log
    #[cfg(feature = "ai-log")]
    app.register_type::<crate::ai::debug_log::AiLogConfig>();
    // crate::ai::npc_decision
    app.register_type::<crate::ai::npc_decision::AiControlled>();
    app.register_type::<crate::ai::npc_decision::AiPlayerMode>();
    app.register_type::<crate::ai::npc_decision::LastAiDecisionTick>();
    // crate::ai::command_outbox
    app.register_type::<crate::ai::command_outbox::AiCommandOutbox>();
    // crate::ai::orchestrator_runtime
    app.register_type::<crate::ai::orchestrator_runtime::OrchestratorRegistry>();
    // crate::ai::plugin
    app.register_type::<crate::ai::plugin::AiBusResource>();
    app.register_type::<crate::ai::plugin::DeclaredFactionSlots>();
    // crate::casus_belli
    app.register_type::<crate::casus_belli::ActiveWars>();
    app.register_type::<crate::casus_belli::CasusBelliRegistry>();
    // crate::choice
    app.register_type::<crate::choice::PendingChoice>();
    app.register_type::<crate::choice::PendingChoiceSelection>();
    // crate::colony
    app.register_type::<crate::colony::AlertCooldowns>();
    app.register_type::<crate::colony::Colony>();
    app.register_type::<crate::colony::ConstructionParams>();
    app.register_type::<crate::colony::DeliverableStockpile>();
    app.register_type::<crate::colony::LastProductionTick>();
    app.register_type::<crate::colony::ResourceCapacity>();
    app.register_type::<crate::colony::ResourceStockpile>();
    // crate::colony::authority
    app.register_type::<crate::colony::authority::AuthorityParams>();
    app.register_type::<crate::colony::authority::PendingSovereigntyChanges>();
    // crate::colony::building_queue
    app.register_type::<crate::colony::building_queue::BuildQueue>();
    app.register_type::<crate::colony::building_queue::BuildingQueue>();
    app.register_type::<crate::colony::building_queue::Buildings>();
    // crate::colony::colonization
    app.register_type::<crate::colony::colonization::ColonizationQueue>();
    app.register_type::<crate::colony::colonization::PendingColonizationOrder>();
    // crate::colony::maintenance
    app.register_type::<crate::colony::maintenance::MaintenanceCost>();
    // crate::colony::population
    app.register_type::<crate::colony::population::FoodConsumption>();
    // crate::colony::production
    app.register_type::<crate::colony::production::ColonyJobRates>();
    app.register_type::<crate::colony::production::Production>();
    app.register_type::<crate::colony::production::ProductionFocus>();
    // crate::colony::system_buildings
    app.register_type::<crate::colony::system_buildings::SlotAssignment>();
    app.register_type::<crate::colony::system_buildings::SystemBuildingQueue>();
    app.register_type::<crate::colony::system_buildings::SystemBuildings>();
    // crate::communication
    app.register_type::<crate::communication::AppliedCommandIds>();
    app.register_type::<crate::communication::CommandLog>();
    app.register_type::<crate::communication::CourierShip>();
    app.register_type::<crate::communication::Message>();
    app.register_type::<crate::communication::NextRemoteCommandId>();
    app.register_type::<crate::communication::PendingColonyDispatches>();
    app.register_type::<crate::communication::PendingCommand>();
    // crate::components
    app.register_type::<crate::components::MovementState>();
    app.register_type::<crate::components::Position>();
    // crate::condition
    app.register_type::<crate::condition::ScopedFlags>();
    // crate::deep_space
    app.register_type::<crate::deep_space::ConstructionPlatform>();
    app.register_type::<crate::deep_space::DeepSpaceStructure>();
    app.register_type::<crate::deep_space::DeliverableRegistry>();
    app.register_type::<crate::deep_space::FTLCommRelay>();
    app.register_type::<crate::deep_space::LifetimeCost>();
    app.register_type::<crate::deep_space::Scrapyard>();
    app.register_type::<crate::deep_space::StructureHitpoints>();
    // crate::empire::comms
    app.register_type::<crate::empire::comms::CommsParams>();
    // crate::event_system
    app.register_type::<crate::event_system::EventBus>();
    app.register_type::<crate::event_system::EventSystem>();
    // crate::events
    app.register_type::<crate::events::EventLog>();
    // crate::faction
    app.register_type::<crate::faction::DiplomaticEvent>();
    app.register_type::<crate::faction::DiplomaticInbox>();
    app.register_type::<crate::faction::Extinct>();
    app.register_type::<crate::faction::FactionOwner>();
    app.register_type::<crate::faction::FactionRelations>();
    app.register_type::<crate::faction::HostileFactions>();
    app.register_type::<crate::faction::KnownFactions>();
    // crate::galaxy
    app.register_type::<crate::galaxy::Anomalies>();
    app.register_type::<crate::galaxy::AtSystem>();
    app.register_type::<crate::galaxy::GalaxyConfig>();
    app.register_type::<crate::galaxy::HomeSystem>();
    app.register_type::<crate::galaxy::HomeSystemAssignments>();
    app.register_type::<crate::galaxy::Hostile>();
    app.register_type::<crate::galaxy::HostileHitpoints>();
    app.register_type::<crate::galaxy::HostileStats>();
    app.register_type::<crate::galaxy::Planet>();
    app.register_type::<crate::galaxy::PortFacility>();
    app.register_type::<crate::galaxy::Sovereignty>();
    app.register_type::<crate::galaxy::StarSystem>();
    app.register_type::<crate::galaxy::StarTypeModifierSet>();
    app.register_type::<crate::galaxy::SystemAttributes>();
    app.register_type::<crate::galaxy::SystemModifiers>();
    // crate::galaxy::biome
    app.register_type::<crate::galaxy::biome::Biome>();
    app.register_type::<crate::galaxy::biome::BiomeRegistry>();
    // crate::galaxy::region
    app.register_type::<crate::galaxy::region::ForbiddenRegion>();
    app.register_type::<crate::galaxy::region::RegionSpecQueue>();
    app.register_type::<crate::galaxy::region::RegionTypeRegistry>();
    // crate::game_state
    app.register_type::<crate::game_state::LoadSaveRequest>();
    app.register_type::<crate::game_state::NewGameParams>();
    // crate::input
    app.register_type::<crate::input::KeybindingRegistry>();
    // crate::knowledge
    app.register_type::<crate::knowledge::DelayedCombatEventQueue>();
    app.register_type::<crate::knowledge::DestroyedShipRegistry>();
    app.register_type::<crate::knowledge::KnowledgeStore>();
    app.register_type::<crate::knowledge::SystemVisibilityMap>();
    app.register_type::<crate::knowledge::TrackedShipSystem>();
    // crate::knowledge::facts
    app.register_type::<crate::knowledge::facts::NextEventId>();
    app.register_type::<crate::knowledge::facts::NotifiedEventIds>();
    app.register_type::<crate::knowledge::facts::PendingFactQueue>();
    app.register_type::<crate::knowledge::facts::RelayNetwork>();
    // crate::knowledge::kind_registry
    app.register_type::<crate::knowledge::kind_registry::KindRegistry>();
    // crate::negotiation
    app.register_type::<crate::negotiation::NegotiationItemKindRegistry>();
    // crate::notifications
    app.register_type::<crate::notifications::NotificationQueue>();
    // crate::observer
    app.register_type::<crate::observer::ObserverMode>();
    app.register_type::<crate::observer::ObserverView>();
    app.register_type::<crate::observer::RngSeed>();
    // crate::persistence::save
    app.register_type::<crate::persistence::save::SaveId>();
    app.register_type::<crate::persistence::save::SaveableMarker>();
    // crate::player
    app.register_type::<crate::player::AboardShip>();
    app.register_type::<crate::player::Empire>();
    app.register_type::<crate::player::EmpireRuler>();
    app.register_type::<crate::player::EmpireViewerSystem>();
    app.register_type::<crate::player::Faction>();
    app.register_type::<crate::player::Player>();
    app.register_type::<crate::player::PlayerEmpire>();
    app.register_type::<crate::player::Ruler>();
    app.register_type::<crate::player::StationedAt>();
    // crate::remote
    #[cfg(feature = "remote")]
    app.register_type::<crate::remote::PendingInputReleases>();
    #[cfg(feature = "remote")]
    app.register_type::<crate::remote::ScreenshotBuffer>();
    // crate::scripting::anomaly_api
    app.register_type::<crate::scripting::anomaly_api::AnomalyRegistry>();
    // crate::scripting::building_api
    app.register_type::<crate::scripting::building_api::BuildingRegistry>();
    // crate::scripting::engine
    app.register_type::<crate::scripting::engine::ScriptEngine>();
    // crate::scripting::faction_api
    app.register_type::<crate::scripting::faction_api::DiplomaticOptionRegistry>();
    app.register_type::<crate::scripting::faction_api::FactionRegistry>();
    app.register_type::<crate::scripting::faction_api::FactionTypeRegistry>();
    // crate::scripting::galaxy_api
    app.register_type::<crate::scripting::galaxy_api::PlanetTypeRegistry>();
    app.register_type::<crate::scripting::galaxy_api::StarTypeRegistry>();
    // crate::scripting::game_rng
    app.register_type::<crate::scripting::game_rng::GameRng>();
    // crate::scripting::knowledge_dispatch
    app.register_type::<crate::scripting::knowledge_dispatch::PendingKnowledgeRecords>();
    // crate::scripting::knowledge_registry
    app.register_type::<crate::scripting::knowledge_registry::KnowledgeSubscriptionRegistry>();
    // crate::scripting::log_buffer
    app.register_type::<crate::scripting::log_buffer::LogBuffer>();
    // crate::scripting::map_api
    app.register_type::<crate::scripting::map_api::MapTypeRegistry>();
    app.register_type::<crate::scripting::map_api::PredefinedSystemRegistry>();
    // crate::ship
    app.register_type::<crate::ship::Cargo>();
    app.register_type::<crate::ship::CommandQueue>();
    app.register_type::<crate::ship::DockedAt>();
    app.register_type::<crate::ship::HarbourModifiers>();
    app.register_type::<crate::ship::PendingShipCommand>();
    app.register_type::<crate::ship::RulesOfEngagement>();
    app.register_type::<crate::ship::Ship>();
    app.register_type::<crate::ship::ShipHitpoints>();
    app.register_type::<crate::ship::ShipModifiers>();
    app.register_type::<crate::ship::ShipState>();
    app.register_type::<crate::ship::ShipStats>();
    app.register_type::<crate::ship::SurveyData>();
    app.register_type::<crate::ship::UndockedForCombat>();
    // crate::ship::command_events
    app.register_type::<crate::ship::command_events::NextCommandId>();
    // crate::ship::conquered
    app.register_type::<crate::ship::conquered::ConqueredCore>();
    // crate::ship::core_deliverable
    app.register_type::<crate::ship::core_deliverable::CoreShip>();
    // crate::ship::courier_route
    app.register_type::<crate::ship::courier_route::CarriedCommands>();
    app.register_type::<crate::ship::courier_route::CourierKnowledgeCargo>();
    app.register_type::<crate::ship::courier_route::CourierRoute>();
    // crate::ship::defense_fleet
    app.register_type::<crate::ship::defense_fleet::DefenseFleet>();
    // crate::ship::fleet
    app.register_type::<crate::ship::fleet::Fleet>();
    app.register_type::<crate::ship::fleet::FleetMembers>();
    // crate::ship::harbour
    app.register_type::<crate::ship::harbour::AppliedDockedModifiers>();
    // crate::ship::pursuit
    app.register_type::<crate::ship::pursuit::DetectedHostiles>();
    // crate::ship::scout
    app.register_type::<crate::ship::scout::ScoutReport>();
    // crate::ship::transit_events
    app.register_type::<crate::ship::transit_events::LastDockedSystem>();
    // crate::ship_design
    app.register_type::<crate::ship_design::HullRegistry>();
    app.register_type::<crate::ship_design::ModuleRegistry>();
    app.register_type::<crate::ship_design::ShipDesignRegistry>();
    app.register_type::<crate::ship_design::SlotTypeRegistry>();
    // crate::species
    app.register_type::<crate::species::ColonyJobs>();
    app.register_type::<crate::species::ColonyPopulation>();
    app.register_type::<crate::species::JobRegistry>();
    app.register_type::<crate::species::SpeciesRegistry>();
    // crate::technology
    app.register_type::<crate::technology::EmpireModifiers>();
    app.register_type::<crate::technology::GameBalance>();
    app.register_type::<crate::technology::GameFlags>();
    app.register_type::<crate::technology::GlobalParams>();
    app.register_type::<crate::technology::LastResearchTick>();
    app.register_type::<crate::technology::PendingColonyTechModifiers>();
    app.register_type::<crate::technology::PendingKnowledgePropagation>();
    app.register_type::<crate::technology::PendingResearch>();
    app.register_type::<crate::technology::RecentlyResearched>();
    app.register_type::<crate::technology::ResearchPool>();
    app.register_type::<crate::technology::ResearchQueue>();
    app.register_type::<crate::technology::TechBranchRegistry>();
    app.register_type::<crate::technology::TechEffectsLog>();
    app.register_type::<crate::technology::TechEffectsPreview>();
    app.register_type::<crate::technology::TechKnowledge>();
    app.register_type::<crate::technology::TechTree>();
    app.register_type::<crate::technology::TechUnlockIndex>();
    // crate::time_system
    app.register_type::<crate::time_system::GameClock>();
    app.register_type::<crate::time_system::GameSpeed>();
    // crate::ui
    app.register_type::<crate::ui::DiplomacyPanelOpen>();
    app.register_type::<crate::ui::ResearchPanelOpen>();
    app.register_type::<crate::ui::UiElementRegistry>();
    app.register_type::<crate::ui::UiState>();
    // crate::ui::console
    app.register_type::<crate::ui::console::ConsoleState>();
    // crate::ui::overlays
    app.register_type::<crate::ui::overlays::ShipDesignerState>();
    // crate::ui::situation_center::diplomatic_tab
    app.register_type::<crate::ui::situation_center::diplomatic_tab::DiplomaticStandingHistory>();
    // crate::ui::situation_center::notifications_tab
    app.register_type::<crate::ui::situation_center::notifications_tab::EscNotificationQueue>();
    // crate::ui::situation_center::registry
    app.register_type::<crate::ui::situation_center::registry::SituationTabRegistry>();
    // crate::ui::situation_center::resource_trends_tab
    app.register_type::<crate::ui::situation_center::resource_trends_tab::ResourceTrendHistory>();
    // crate::ui::situation_center::state
    app.register_type::<crate::ui::situation_center::state::SituationCenterState>();
    // crate::visualization
    app.register_type::<crate::visualization::ContextMenu>();
    app.register_type::<crate::visualization::CycleSelection>();
    app.register_type::<crate::visualization::DeployMode>();
    app.register_type::<crate::visualization::EguiWantsPointer>();
    app.register_type::<crate::visualization::GalaxyView>();
    app.register_type::<crate::visualization::OutlineExpandedSystems>();
    app.register_type::<crate::visualization::SelectedPlanet>();
    app.register_type::<crate::visualization::SelectedShip>();
    app.register_type::<crate::visualization::SelectedShips>();
    app.register_type::<crate::visualization::SelectedSystem>();
    // crate::visualization::stars (private types — registered via helper)
    crate::visualization::register_star_types(app);
    // crate::visualization::territory
    app.register_type::<crate::visualization::territory::TerritoryQuad>();
}
