//! #334 Phase 1: per-variant command handlers.
//!
//! Each handler reads a single typed [`MessageReader<XRequested>`](bevy::prelude::MessageReader)
//! and holds only the queries / resources it needs. Together with
//! `super::dispatcher::dispatch_queued_commands`, this replaces the fat
//! pre-#334 dispatch loops (`process_command_queue` / `process_deliverable_commands`)
//! with a narrow dispatcher and focused handlers.
//!
//! Phase 1 scope: `handle_move_requested` + `handle_move_to_coordinates_requested`.
//! Phase 2 added: Load / Deploy / Transfer / Scrapyard / Survey / Colonize.
//! Phase 3 added: `handle_scout_requested` + `handle_attack_requested` skeleton.
//! Phase 4 will add the Lua bridge for `CommandExecuted`.

pub mod attack_handler;
pub mod deliverable_handler;
pub mod move_handler;
pub mod scout_handler;
pub mod settlement_handler;

pub use attack_handler::handle_attack_requested;
pub use deliverable_handler::{
    handle_deploy_deliverable_requested, handle_load_deliverable_requested,
    handle_load_from_scrapyard_requested, handle_transfer_to_structure_requested,
};
pub use move_handler::{handle_move_requested, handle_move_to_coordinates_requested};
pub use scout_handler::handle_scout_requested;
pub use settlement_handler::{handle_colonize_requested, handle_survey_requested};
