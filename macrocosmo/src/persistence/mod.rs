//! Save / load infrastructure (#247).
//!
//! The public API is a pair of postcard-backed functions:
//!
//! ```no_run
//! use bevy::prelude::World;
//! use std::path::Path;
//! use macrocosmo::persistence::{save_game_to, load_game_from};
//!
//! # let mut world = World::new();
//! save_game_to(&mut world, Path::new("savegame.bin")).expect("save failed");
//! load_game_from(&mut world, Path::new("savegame.bin")).expect("load failed");
//! ```
//!
//! The save wire format is `postcard` (v1) encoding of a [`GameSave`] struct
//! tagged with [`SAVE_VERSION`]. Entity references are translated to `u64`
//! save ids via [`EntityMap`]; [`RemapEntities`] marks types whose saved form
//! carries such ids.
//!
//! ## What is persisted
//!
//! **Resources**: [`GameClock`](crate::time_system::GameClock),
//! [`GameSpeed`](crate::time_system::GameSpeed),
//! [`LastProductionTick`](crate::colony::LastProductionTick),
//! [`GalaxyConfig`](crate::galaxy::GalaxyConfig),
//! [`GameRng`](crate::scripting::game_rng::GameRng) (full Xoshiro256++ state
//! for deterministic stream continuation),
//! [`FactionRelations`](crate::faction::FactionRelations),
//! [`PendingFactQueue`](crate::knowledge::PendingFactQueue),
//! [`EventLog`](crate::events::EventLog),
//! [`NotificationQueue`](crate::notifications::NotificationQueue).
//!
//! **Components** (see [`savebag::SavedComponentBag`] for the full 62-field
//! list): galaxy (StarSystem/Planet/SystemAttributes/Sovereignty/Hostile
//! family/Anomalies/ForbiddenRegion/PortFacility), colony full stack
//! (Colony/Buildings/BuildQueue/BuildingQueue/SystemBuildings/
//! SystemBuildingQueue/Production/ProductionFocus/ColonyJobs/
//! ColonyPopulation/MaintenanceCost/FoodConsumption/DeliverableStockpile/
//! ColonizationQueue/PendingColonizationOrder/AuthorityParams/
//! ConstructionParams), ship full stack (Ship/ShipState/ShipHitpoints/Cargo/
//! CommandQueue/ShipModifiers/CourierRoute/SurveyData/ScoutReport/Fleet/
//! FleetMembers/DetectedHostiles/RulesOfEngagement/PendingShipCommand/
//! PendingDiplomaticAction/PendingCommand/PendingResearch/
//! PendingKnowledgePropagation), deep-space structures, KnowledgeStore,
//! CommandLog, TechTree / ResearchQueue, Empire modifiers, GameFlags /
//! ScopedFlags, FactionOwner / Faction, Player / StationedAt / AboardShip /
//! Empire / PlayerEmpire.
//!
//! ## What is **not** persisted (re-derived on load)
//!
//! Lua-loaded registries (`BuildingRegistry`, `HullRegistry`, `ModuleRegistry`,
//! `ShipDesignRegistry`, `StructureRegistry`, `SpeciesRegistry`, `JobRegistry`,
//! `TechRegistry`, `EventDefinitionRegistry`, `PlanetTypeRegistry`,
//! `StarTypeRegistry`, `ScriptEngine`) are reloaded from `scripts/` rather
//! than serialized — this is the canonical decoupling between data content
//! and saved state. `scripts_version` mismatches warn but do not hard-fail
//! (see [`SCRIPTS_VERSION`]).

pub mod load;
pub mod remap;
pub mod rng_serde;
pub mod save;
pub mod savebag;

pub use load::{LoadError, load_game_from, load_game_from_reader};
pub use remap::{EntityMap, RemapEntities};
pub use rng_serde::SavedGameRng;
pub use save::{
    GameSave, SAVE_VERSION, SCRIPTS_VERSION, SaveError, SaveId, SaveableMarker, SavedEntity,
    SavedResources, capture_save, save_game_to, save_game_to_writer,
};
pub use savebag::SavedComponentBag;
