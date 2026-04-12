//! AI integration layer — bridges the pure `macrocosmo-ai` core into the
//! Bevy-based game engine (`macrocosmo`).
//!
//! This module is the **only** place where `macrocosmo` (Bevy / ECS / Lua)
//! and `macrocosmo-ai` (engine-agnostic pure Rust) meet. The dependency
//! direction is strictly `macrocosmo → macrocosmo-ai`; the AI crate has no
//! knowledge of Bevy, `Entity`, `GameClock`, etc., and CI
//! (`ai-core-isolation.yml`) enforces this invariant.
//!
//! The public surface consists of:
//!
//! - [`AiPlugin`] — the Bevy plugin that registers the [`AiBusResource`]
//!   (wrapping `macrocosmo_ai::AiBus`), runs a one-time schema declaration
//!   at `Startup`, and configures ordered system sets under `Update`.
//! - [`AiBusResource`] — thin `Resource` newtype around `AiBus` with
//!   `Deref`/`DerefMut` to the underlying bus for ergonomic access.
//! - [`AiTickSet`] — the three ordered system sets each AI-related system
//!   hangs under: `MetricProduce → Reason → CommandDrain`, all scheduled
//!   `.after(crate::time_system::advance_game_time)`.
//! - [`emit::AiBusWriter`] / [`emit::AiBusReader`] / [`emit::AiBusDrainer`]
//!   — `SystemParam` helpers wrapping write / read / drain access to the
//!   bus with automatic tick stamping from `GameClock`.
//! - [`convert`] — `Entity`/`GameClock` ↔ `macrocosmo-ai` type helpers.
//!
//! Content (metrics/commands/evidence) is declared by downstream issues via
//! [`schema::declare_all`]; this integration issue (#203) establishes the
//! infrastructure only.
//!
//! For convenience, the entire `macrocosmo-ai` crate is re-exported as
//! [`core`] so callers can refer to AI types via `crate::ai::core::…`.

pub mod convert;
pub mod emit;
pub mod plugin;
pub mod schema;

pub use plugin::{AiBusResource, AiPlugin, AiTickSet};

/// Re-export of the `macrocosmo-ai` crate. Callers should prefer
/// `crate::ai::core::…` over `macrocosmo_ai::…` so that swapping the AI
/// core (e.g. for a mock) is a single edit in this module.
pub use macrocosmo_ai as core;
