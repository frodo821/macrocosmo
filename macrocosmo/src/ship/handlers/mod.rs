//! #334 Phase 1: per-variant command handlers.
//!
//! Each handler reads a single typed [`MessageReader<XRequested>`](bevy::prelude::MessageReader)
//! and holds only the queries / resources it needs. Together with
//! `super::dispatcher::dispatch_queued_commands`, this replaces the fat
//! `process_command_queue` + `process_deliverable_commands` loops with a
//! narrow dispatcher and focused handlers.
//!
//! Phase 1 scope: `handle_move_requested` + `handle_move_to_coordinates_requested`.
//! Phases 2/3/4 migrate the remaining variants into this module.

pub mod move_handler;

pub use move_handler::{handle_move_requested, handle_move_to_coordinates_requested};
