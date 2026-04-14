//! Save / load infrastructure (#247).
//!
//! # Phase A (this module)
//!
//! Foundations + core game state. The public API is a pair of postcard-backed
//! functions:
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
//! The save wire format is `postcard` (v1) encoding of a [`GameSave`] struct.
//! Entity references are translated to `u64` save ids via [`EntityMap`];
//! [`RemapEntities`] marks types whose saved form carries such ids.
//!
//! ## What is persisted
//!
//! - Resources: [`GameClock`](crate::time_system::GameClock),
//!   [`GameSpeed`](crate::time_system::GameSpeed),
//!   [`LastProductionTick`](crate::colony::LastProductionTick),
//!   [`GalaxyConfig`](crate::galaxy::GalaxyConfig),
//!   [`GameRng`](crate::scripting::game_rng::GameRng) (deterministic stream
//!   continuation), and [`FactionRelations`](crate::faction::FactionRelations).
//! - Components: Position, MovementState, StarSystem, Planet, SystemAttributes,
//!   Sovereignty, Hostile/AtSystem/HostileHitpoints/HostileStats/HostileKind,
//!   ObscuredByGas, PortFacility, Colony,
//!   ResourceStockpile, ResourceCapacity, Ship, ShipState, ShipHitpoints, Cargo,
//!   FactionOwner, Faction, Player, StationedAt, AboardShip, Empire, PlayerEmpire.
//!
//! ## What is deferred to Phase B/C
//!
//! Ship extension state (ShipModifiers, ShipStats, CommandQueue,
//! CourierRoute, SurveyData, ScoutReport, Fleet), colony extension state
//! (BuildQueue, BuildingQueue, SystemBuildings, ColonyJobs), deep-space
//! structures, knowledge store, pending command queues, tech tree,
//! research queue, event/notification logs, Lua registries
//! (re-derived from scripts on load).

pub mod load;
pub mod remap;
pub mod rng_serde;
pub mod save;
pub mod savebag;

pub use load::{load_game_from, load_game_from_reader, LoadError};
pub use remap::{EntityMap, RemapEntities};
pub use rng_serde::SavedGameRng;
pub use save::{
    capture_save, save_game_to, save_game_to_writer, GameSave, SaveError, SaveId, SaveableMarker,
    SavedEntity, SavedResources, SAVE_VERSION, SCRIPTS_VERSION,
};
pub use savebag::SavedComponentBag;
