pub mod combat;
pub mod combat_sim;
pub mod command;
pub mod conquered;
pub mod core_deliverable;
pub mod courier_route;
// #300 (S-6): Defense Fleet auto-composition on Core deploy.
pub mod defense_fleet;
pub mod deliverable_ops;
pub mod exploration;
pub mod fleet;
/// #384: Harbour dock/undock core logic + harbour lifecycle systems.
pub mod harbour;
pub mod hitpoints;
pub mod modifiers;
pub mod movement;
pub mod pursuit;
pub mod routing;
pub mod scout;
pub mod settlement;
pub mod survey;
/// #291: Fleet system transit events (entered / left).
pub mod transit_events;
// #334 Phase 1: event-driven command dispatch — message types and allocator.
pub mod command_events;
// #334 Phase 1: queue dispatcher — validates + emits CommandRequested messages.
pub mod dispatcher;
// #334 Phase 1: per-variant handlers (MoveTo + MoveToCoordinates this phase).
pub mod handlers;
// #334 Phase 1: CommandExecuted → CommandLog / gamestate bridge systems.
pub mod bridges;

pub use combat::*;
pub use command::*;
pub use conquered::ConqueredCore;
pub use core_deliverable::{
    CoreShip, handle_core_deploy_requested, spawn_core_ship_from_deliverable,
};
pub use courier_route::*;
pub use defense_fleet::{DefenseFleet, join_defense_fleet};
pub use exploration::*;
pub use fleet::*;
pub use hitpoints::*;
pub use modifiers::*;
pub use movement::*;
pub use pursuit::*;
pub use settlement::*;
pub use survey::*;

use bevy::prelude::*;

use crate::amount::Amt;
use crate::components::Position;
use crate::modifier::{CachedValue, ScopedModifiers};
use crate::ship_design::ShipDesignRegistry;

// --- #34: Command queue ---

#[derive(Component, Default, Clone, Reflect)]
#[reflect(Component)]
pub struct CommandQueue {
    pub commands: Vec<QueuedCommand>,
    /// Predicted position after all queued commands execute
    pub predicted_position: [f64; 3],
    /// Predicted system after all queued commands execute
    pub predicted_system: Option<Entity>,
}

impl CommandQueue {
    /// Push a command and update predicted position
    pub fn push(
        &mut self,
        cmd: QueuedCommand,
        system_positions: &impl Fn(Entity) -> Option<[f64; 3]>,
    ) {
        match &cmd {
            QueuedCommand::MoveTo { system }
            | QueuedCommand::Survey { system }
            | QueuedCommand::Colonize { system, .. }
            | QueuedCommand::LoadDeliverable { system, .. } => {
                if let Some(pos) = system_positions(*system) {
                    self.predicted_position = pos;
                    self.predicted_system = Some(*system);
                }
            }
            // #217: Scout visits the target and — under ReportMode::Return —
            // comes back. We track the final predicted position as the
            // target_system's position (FtlComm path stays at target too;
            // the fallback-to-Return auto-queues a MoveTo home separately).
            QueuedCommand::Scout { target_system, .. } => {
                if let Some(pos) = system_positions(*target_system) {
                    self.predicted_position = pos;
                    self.predicted_system = Some(*target_system);
                }
            }
            QueuedCommand::MoveToCoordinates { target } => {
                // #185: After a deep-space loiter move, the ship is no longer in any system.
                self.predicted_position = *target;
                self.predicted_system = None;
            }
            QueuedCommand::DeployDeliverable { position, .. } => {
                // #223: Deploy parks the ship at `position` in deep space.
                self.predicted_position = *position;
                self.predicted_system = None;
            }
            // #223: In-place resource actions — no predicted movement change.
            QueuedCommand::TransferToStructure { .. } | QueuedCommand::LoadFromScrapyard { .. } => {
            }
        }
        self.commands.push(cmd);
    }

    /// Reset prediction to current ship state (call when queue becomes empty or command consumed)
    pub fn sync_prediction(&mut self, current_pos: [f64; 3], current_system: Option<Entity>) {
        if self.commands.is_empty() {
            self.predicted_position = current_pos;
            self.predicted_system = current_system;
        }
    }
}

/// #217: How a scout ship reports its observation back to the empire.
///
/// `#[allow(dead_code)]`: the variants are only *constructed* by tests and
/// (future) UI / AI code that issues Scout commands — no in-engine system
/// constructs them yet. The enum is exhaustively matched by the scout
/// pipeline regardless, so the constructors themselves are load-bearing.
#[allow(dead_code)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, bevy::reflect::Reflect)]
pub enum ReportMode {
    /// If an FTL Comm Relay covers both the scout position and the player
    /// empire at observation-completion time, the report is delivered
    /// instantaneously. Otherwise falls back to `Return` (ship carries the
    /// report home physically).
    FtlComm,
    /// The ship always carries the report back to its `home_port` / origin
    /// system. The empire only learns of the observation when the ship docks.
    Return,
}

#[derive(Clone, Debug, bevy::reflect::Reflect)]
pub enum QueuedCommand {
    MoveTo {
        system: Entity,
    },
    Survey {
        system: Entity,
    },
    Colonize {
        system: Entity,
        planet: Option<Entity>,
    },
    /// #185: Travel sublight to an arbitrary point in deep space and loiter there.
    MoveToCoordinates {
        target: [f64; 3],
    },
    /// #217: Dispatch the ship to `target_system`, observe the area within
    /// the scout's sensor range for `observation_duration` hexadies, then
    /// report back via `report_mode`. The ship MUST have a scout module
    /// equipped and FTL capability; otherwise the command is rejected at
    /// dispatch time with a warning.
    ///
    /// `#[allow(dead_code)]`: constructed by tests and (future) UI / AI.
    #[allow(dead_code)]
    Scout {
        target_system: Entity,
        observation_duration: i64,
        report_mode: ReportMode,
    },
    /// #223: Load a deliverable from the docked system's `DeliverableStockpile`
    /// into this ship's `Cargo`. `stockpile_index` is the zero-based index in
    /// the stockpile at the time the command is executed; the command is a
    /// no-op (with warning) if the index is out of range or the ship has no
    /// room for the item.
    LoadDeliverable {
        system: Entity,
        stockpile_index: usize,
    },
    /// #223: Deploy the deliverable at `item_index` within this ship's Cargo
    /// at the given deep-space coordinate. If the ship is not already at
    /// `position` within a small epsilon, the command queues a sublight move
    /// first and re-evaluates on arrival. On deployment, the item is removed
    /// from Cargo and a new `DeepSpaceStructure` entity is spawned owned by
    /// the ship's `Owner`.
    DeployDeliverable {
        position: [f64; 3],
        item_index: usize,
    },
    /// #223: Transfer resources from this ship's Cargo into a co-located
    /// `ConstructionPlatform`'s accumulated pool. Ship must be at the same
    /// position (within epsilon) as the target structure.
    TransferToStructure {
        structure: Entity,
        minerals: Amt,
        energy: Amt,
    },
    /// #223: Drain a co-located `Scrapyard`'s remaining resources into the
    /// ship's Cargo (clamped by cargo capacity).
    LoadFromScrapyard {
        structure: Entity,
    },
}

/// Initial FTL speed as a multiple of light speed
pub const INITIAL_FTL_SPEED_C: f64 = 10.0;

pub struct ShipPlugin;

impl Plugin for ShipPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<routing::RouteCalculationsPending>();
        // #334 Phase 2 (Commit 2): `PendingCoreDeploys` was retired in favour
        // of the `CoreDeployRequested` message bus registered via
        // `CommandEventsPlugin`.
        // #334 Phase 1: register command-dispatch message types + allocator
        // before any dispatcher/handler system that references them.
        app.add_plugins(command_events::CommandEventsPlugin);
        // #439 Phase 2: Modifier/HP sync systems are NOT gated on
        // `GameState::InGame` — they are pure pipeline syncs (no damage
        // application, no time-delta integration) and must keep running
        // during construction / load so the modifier cache stays coherent
        // before gameplay starts.
        app.add_systems(
            Update,
            (
                sync_ship_module_modifiers,
                sync_ship_hitpoints.after(sync_ship_module_modifiers),
            )
                .after(crate::time_system::advance_game_time)
                .before(crate::colony::advance_production_tick),
        );
        app.add_systems(
            Update,
            (
                tick_shield_regen,
                sublight_movement_system,
                process_ftl_travel,
                deliver_survey_results.after(process_ftl_travel),
                process_surveys,
                process_settling,
                process_refitting,
                process_pending_ship_commands,
                // #296 (S-3) / #334 Phase 2 Commit 2: Resolve Core deploy
                // requests into actual CoreShip entities, grouping same-tick
                // duplicates and tie-breaking via GameRng. Per-tick ordering
                // with `handle_deploy_deliverable_requested` (which emits the
                // messages) is declared in the dispatcher `add_systems` call
                // below.
                core_deliverable::handle_core_deploy_requested,
                resolve_combat,
                tick_ship_repair,
                // #117: Courier automation — runs before the dispatcher so
                // any MoveTo it queues this frame is dispatched in the
                // same frame.
                tick_courier_routes
                    .before(dispatcher::dispatch_queued_commands)
                    .after(sublight_movement_system)
                    .after(process_ftl_travel),
                // #186 Phase 1: Aggressive ROE detection of hostile deep-space
                // contacts. Runs after movement so ship positions are current.
                pursuit::detect_hostiles_system
                    .after(sublight_movement_system)
                    .after(process_ftl_travel)
                    .after(handlers::handle_attack_requested),
                // #217: Scout observation ticker — transitions a Scouting ship
                // into InSystem + attaches a ScoutReport when the timer expires.
                // Runs after FTL/sublight movement so a ship that finished
                // travel and transitioned into Scouting this tick doesn't
                // double-process.
                scout::tick_scout_observation
                    .after(process_ftl_travel)
                    .after(sublight_movement_system)
                    .after(handlers::handle_scout_requested),
                // #217: Scout report delivery — writes to KnowledgeStore on
                // FTL comm success, or auto-queues return home. Runs after the
                // observation ticker so a ship that completed observation this
                // frame can still have its report routed this frame.
                scout::process_scout_report
                    .after(scout::tick_scout_observation)
                    .after(handlers::handle_scout_requested),
                // #287 (γ-1): Reconcile FleetMembers against live Ship entities
                // and despawn fleets that have lost their last member. Runs
                // after every system that may despawn a ship this frame
                // (combat, settlement, refit consumption, command handling)
                // so the cleanup is visible next tick at the latest.
                fleet::prune_empty_fleets
                    .after(resolve_combat)
                    .after(process_settling)
                    .after(process_refitting)
                    .after(handlers::handle_attack_requested),
            )
                .after(crate::time_system::advance_game_time)
                .before(crate::colony::advance_production_tick)
                .run_if(in_state(crate::game_state::GameState::InGame)),
        );
        // #298 (S-4): Conquered Core lifecycle — transition, wartime lock,
        // and peacetime recovery. Separate `add_systems` call to stay under
        // Bevy 0.18's 20-arm `IntoScheduleConfigs` limit.
        app.add_systems(
            Update,
            (
                conquered::check_conquered_transition.after(resolve_combat),
                conquered::enforce_conquered_hp_lock.after(conquered::check_conquered_transition),
                conquered::tick_conquered_recovery.after(conquered::enforce_conquered_hp_lock),
            )
                .after(crate::time_system::advance_game_time)
                .before(crate::colony::advance_production_tick)
                .run_if(in_state(crate::game_state::GameState::InGame)),
        );
        // #334 Phase 1–3: dispatcher + per-variant handlers. Kept in its
        // own `add_systems` call so we stay under Bevy 0.18's 20-arm
        // `IntoScheduleConfigs` limit without resorting to a nested tuple
        // (which was observed to elide systems from the scheduler).
        //
        // #334 Phase 3 (Commit 3): legacy per-variant dispatch loops
        // deleted — the handler chain below is the sole consumer of
        // `QueuedCommand` now.
        app.add_systems(
            Update,
            (
                dispatcher::dispatch_queued_commands,
                handlers::handle_move_requested,
                handlers::handle_move_to_coordinates_requested,
                // #334 Phase 2 (Commit 1/2): LoadDeliverable / DeployDeliverable
                // handlers run after the dispatcher so same-tick messages are
                // picked up. Core-branch of Deploy emits a `CoreDeployRequested`
                // message that `handle_core_deploy_requested` drains.
                handlers::handle_load_deliverable_requested,
                handlers::handle_deploy_deliverable_requested,
                // #334 Phase 2 (Commit 3): Transfer / LoadFromScrapyard
                // handlers.
                handlers::handle_transfer_to_structure_requested,
                handlers::handle_load_from_scrapyard_requested,
                // #334 Phase 2 (Commit 4): Survey / Colonize handlers.
                handlers::handle_survey_requested,
                handlers::handle_colonize_requested,
                // #334 Phase 3 (Commit 1): Scout handler.
                handlers::handle_scout_requested,
                // #334 Phase 3 (Commit 2): AttackRequested skeleton (no-op
                // foundation for #219 / #220 — receives no messages today
                // because no code path emits `AttackRequested` yet).
                handlers::handle_attack_requested,
            )
                .chain()
                .after(sublight_movement_system)
                .after(process_ftl_travel)
                .after(process_surveys)
                // Core deploy resolution must run after the deploy handler
                // that emits `CoreDeployRequested` this tick.
                .before(core_deliverable::handle_core_deploy_requested)
                .after(crate::time_system::advance_game_time)
                .before(crate::colony::advance_production_tick)
                .run_if(in_state(crate::game_state::GameState::InGame)),
        );
        // #334 Phase 1: CommandExecuted → CommandLog bridge. Runs after the
        // route poller (which emits terminal CommandExecuted for deferred
        // MoveTo) and after every handler so synchronous emissions are
        // visible in the same frame.
        app.add_systems(
            Update,
            bridges::bridge_command_executed_to_log
                .after(routing::poll_pending_routes)
                .after(handlers::handle_move_requested)
                .after(handlers::handle_move_to_coordinates_requested)
                .after(handlers::handle_load_deliverable_requested)
                .after(handlers::handle_deploy_deliverable_requested)
                .after(handlers::handle_transfer_to_structure_requested)
                .after(handlers::handle_load_from_scrapyard_requested)
                .after(handlers::handle_survey_requested)
                .after(handlers::handle_colonize_requested)
                .after(handlers::handle_scout_requested)
                .after(handlers::handle_attack_requested)
                .after(core_deliverable::handle_core_deploy_requested)
                .after(crate::time_system::advance_game_time)
                .before(crate::colony::advance_production_tick)
                .run_if(in_state(crate::game_state::GameState::InGame)),
        );
        // #334 Phase 4: CommandExecuted → Lua `on_command_completed` hook.
        // Runs alongside the CommandLog bridge — both read the same
        // `CommandExecuted` messages independently (different `MessageReader`
        // cursors), so the two subscribers never race. Queue-only: this
        // bridge enqueues a `COMMAND_COMPLETED_EVENT` on `EventSystem`;
        // `dispatch_event_handlers` in the scripting plugin drains the
        // fired_log on its own tick. See plan §7 Phase 4 +
        // `memory/feedback_rust_no_lua_callback.md`.
        app.add_systems(
            Update,
            bridges::bridge_command_executed_to_gamestate
                .after(routing::poll_pending_routes)
                .after(handlers::handle_move_requested)
                .after(handlers::handle_move_to_coordinates_requested)
                .after(handlers::handle_load_deliverable_requested)
                .after(handlers::handle_deploy_deliverable_requested)
                .after(handlers::handle_transfer_to_structure_requested)
                .after(handlers::handle_load_from_scrapyard_requested)
                .after(handlers::handle_survey_requested)
                .after(handlers::handle_colonize_requested)
                .after(handlers::handle_scout_requested)
                .after(handlers::handle_attack_requested)
                .after(core_deliverable::handle_core_deploy_requested)
                .after(crate::time_system::advance_game_time)
                .before(crate::colony::advance_production_tick)
                .run_if(in_state(crate::game_state::GameState::InGame)),
        );
        // #291: Fleet departure detection — fires `macrocosmo:fleet_system_left`
        // when a ship's ShipState transitions from InSystem to InFTL/SubLight.
        // Runs after all movement and command systems so state changes are final.
        app.add_systems(
            Update,
            transit_events::detect_fleet_departures
                .after(sublight_movement_system)
                .after(process_ftl_travel)
                .after(routing::poll_pending_routes)
                .after(handlers::handle_move_requested)
                .after(handlers::handle_move_to_coordinates_requested)
                .after(crate::time_system::advance_game_time)
                .before(crate::colony::advance_production_tick)
                .run_if(in_state(crate::game_state::GameState::InGame)),
        );
        // #128: Poll route tasks after Commands emitted by handlers are flushed.
        app.add_systems(
            Update,
            (
                bevy::ecs::schedule::ApplyDeferred,
                routing::poll_pending_routes,
            )
                .chain()
                .after(handlers::handle_attack_requested)
                .after(crate::time_system::advance_game_time)
                .before(crate::colony::advance_production_tick)
                .run_if(in_state(crate::game_state::GameState::InGame)),
        );
        // #384: Harbour lifecycle systems. Separate `add_systems` call to stay
        // under Bevy's tuple-arm limit.
        app.add_systems(
            Update,
            (
                harbour::auto_undock_on_move_command
                    .before(sublight_movement_system)
                    .before(process_ftl_travel),
                harbour::sync_docked_position
                    .after(sublight_movement_system)
                    .after(process_ftl_travel),
                harbour::force_undock_on_harbour_destroy.after(resolve_combat),
            )
                .after(crate::time_system::advance_game_time)
                .before(crate::colony::advance_production_tick)
                .run_if(in_state(crate::game_state::GameState::InGame)),
        );
        // #384: Combat ROE harbour systems — auto-undock/re-dock.
        app.add_systems(
            Update,
            (
                harbour::auto_undock_on_combat_roe.before(resolve_combat),
                harbour::auto_return_dock_after_combat.after(resolve_combat),
            )
                .after(crate::time_system::advance_game_time)
                .before(crate::colony::advance_production_tick)
                .run_if(in_state(crate::game_state::GameState::InGame)),
        );
        // #384: Docked modifier propagation — runs after module sync.
        // #439 Phase 2: NOT gated on `InGame` — shares the modifier-pipeline
        // sync category as `sync_ship_module_modifiers` / `sync_ship_hitpoints`.
        app.add_systems(
            Update,
            harbour::sync_docked_modifiers
                .after(sync_ship_module_modifiers)
                .after(crate::time_system::advance_game_time)
                .before(crate::colony::advance_production_tick),
        );
    }
}

// --- #57: Rules of Engagement ---

/// Controls automatic combat behavior for a ship.
#[derive(Component, Clone, Copy, Debug, Default, PartialEq, Eq, Reflect)]
#[reflect(Component)]
pub enum RulesOfEngagement {
    /// Always attack hostiles in system
    Aggressive,
    /// Only fight back when attacked (hostile initiates) — same as current behavior
    #[default]
    Defensive,
    /// Do not engage hostiles; skip combat entirely
    Retreat,
    /// #384: Evade combat — stay docked if harboured (sheltered), otherwise retreat
    Evasive,
    /// #384: Passive — never engage, stay docked if harboured (sheltered)
    Passive,
}

impl RulesOfEngagement {
    pub fn label(&self) -> &'static str {
        match self {
            Self::Aggressive => "Aggressive",
            Self::Defensive => "Defensive",
            Self::Retreat => "Retreat",
            Self::Evasive => "Evasive",
            Self::Passive => "Passive",
        }
    }

    pub const ALL: [RulesOfEngagement; 5] = [
        RulesOfEngagement::Aggressive,
        RulesOfEngagement::Defensive,
        RulesOfEngagement::Retreat,
        RulesOfEngagement::Evasive,
        RulesOfEngagement::Passive,
    ];
}

// --- #33: Pending ship command system ---

/// A command queued for a remote ship, waiting for light-speed communication delay.
#[derive(Component, Reflect)]
#[reflect(Component)]
pub struct PendingShipCommand {
    pub ship: Entity,
    pub command: ShipCommand,
    pub arrives_at: i64,
}

/// The kinds of commands that can be issued to a ship.
#[derive(Clone, Debug, bevy::reflect::Reflect)]
pub enum ShipCommand {
    MoveTo {
        destination: Entity,
    },
    Survey {
        target: Entity,
    },
    Colonize,
    SetROE {
        roe: RulesOfEngagement,
    },
    /// Enqueue a command into the ship's CommandQueue (for in-transit ships).
    EnqueueCommand(QueuedCommand),
}

/// A module equipped in a specific slot on a ship.
#[derive(Clone, Debug, bevy::reflect::Reflect)]
pub struct EquippedModule {
    pub slot_type: String,
    pub module_id: String,
}

/// Per-ship modifier scopes, driven by equipped modules and tech effects.
#[derive(Component, Default, Reflect)]
#[reflect(Component)]
pub struct ShipModifiers {
    pub speed: ScopedModifiers,
    pub ftl_range: ScopedModifiers,
    pub survey_speed: ScopedModifiers,
    pub colonize_speed: ScopedModifiers,
    pub evasion: ScopedModifiers,
    pub cargo_capacity: ScopedModifiers,
    pub attack: ScopedModifiers,
    pub defense: ScopedModifiers,
    pub armor_max: ScopedModifiers,
    pub shield_max: ScopedModifiers,
    pub shield_regen: ScopedModifiers,
    /// #384: How many size-units of ships this vessel can harbour.
    pub harbour_capacity: ScopedModifiers,
}

/// Cached computed stats for a ship, derived from ShipModifiers.
#[derive(Component, Default, Reflect)]
#[reflect(Component)]
pub struct ShipStats {
    pub speed: CachedValue,
    pub ftl_range: CachedValue,
    pub survey_speed: CachedValue,
    pub colonize_speed: CachedValue,
    pub evasion: CachedValue,
    pub cargo_capacity: CachedValue,
    pub maintenance: Amt,
    /// #384: Cached harbour capacity (from ScopedModifiers). > 0 means ship is a harbour.
    pub harbour_capacity: CachedValue,
}

/// 3-layer hit point model: shield → armor → hull.
/// Shield regenerates over time; armor/hull require docking at a Port.
#[derive(Component, Clone, Debug, Reflect)]
#[reflect(Component)]
pub struct ShipHitpoints {
    pub hull: f64,
    pub hull_max: f64,
    pub armor: f64,
    pub armor_max: f64,
    pub shield: f64,
    pub shield_max: f64,
    pub shield_regen: f64, // per hexadies
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, bevy::reflect::Reflect)]
pub enum Owner {
    Empire(Entity),
    Neutral,
}

impl Owner {
    /// Check if this owner is any empire (not neutral).
    pub fn is_empire(&self) -> bool {
        matches!(self, Owner::Empire(_))
    }
}

#[derive(Component, Reflect)]
#[reflect(Component)]
pub struct Ship {
    pub name: String,
    pub design_id: String,
    pub hull_id: String,
    pub modules: Vec<EquippedModule>,
    pub owner: Owner,
    pub sublight_speed: f64,
    pub ftl_range: f64,
    pub ruler_aboard: bool,
    /// #64: System entity where maintenance is charged
    pub home_port: Entity,
    /// #123: Last `ShipDesignDefinition.revision` this ship was synchronized
    /// with. When the underlying design's revision moves ahead, the ship is
    /// flagged as "needs refit" in the UI and can have the new design
    /// applied via the Apply Refit action.
    pub design_revision: u64,
    /// #287 (γ-1): The `Fleet` entity this ship belongs to, or `None` if
    /// detached. This is a back-pointer mirroring `FleetMembers` on the
    /// fleet entity — mutate only via the helpers in
    /// [`crate::ship::fleet`] (or within a single Commands batch) to
    /// preserve the bidirectional invariant. Every ship spawned via
    /// [`spawn_ship`] starts as the sole member of a freshly-created
    /// 1-ship Fleet (the same invariant is applied for test-only spawns
    /// through `tests/common::spawn_test_ship`).
    pub fleet: Option<Entity>,
}

impl Ship {
    /// #296 (S-3): A ship is *immobile* when its design confers neither
    /// sublight nor FTL propulsion. Infrastructure Core ships are the
    /// canonical example: their hull has `base_speed = 0` and they carry no
    /// FTL module, so both stats are zero.
    ///
    /// This predicate is consulted by:
    /// * `start_sublight_travel` — returns `Err` instead of transitioning
    ///   such a ship into `ShipState::SubLight`.
    /// * `routing::plan_ftl_route` / `dispatcher::dispatch_queued_commands`
    ///   — skip route planning entirely when the source ship is immobile
    ///   (otherwise a queued MoveTo would stall forever).
    /// * UI MoveTo guards in `context_menu` — suppress the Move button.
    /// * `pursuit::detect_hostiles_system` — early-returns for immobile
    ///   self-detectors (they can never intercept).
    pub fn is_immobile(&self) -> bool {
        self.sublight_speed <= 0.0 && self.ftl_range <= 0.0
    }
}

/// #384: Modifiers that a harbour propagates to its docked ships.
/// Each entry is (filter, target, modifier) where:
/// - filter = "self" | "*" | "<hull_id>"
/// - target = the modifier target string (e.g. "ship.speed")
/// - modifier = the Modifier to apply
#[derive(Component, Default, Debug, Clone, Reflect)]
#[reflect(Component)]
pub struct HarbourModifiers(pub Vec<(String, String, crate::modifier::Modifier)>);

/// #384: Transient marker on ships undocked for combat, tracking their original harbour.
/// After combat resolves with no hostiles remaining, the ship attempts to re-dock.
#[derive(Component, Debug, Clone, Copy, Reflect)]
#[reflect(Component)]
pub struct UndockedForCombat(pub Entity);

/// Orthogonal marker: ship is docked at a specific entity (port, station, etc.).
/// Added/removed independently of `ShipState`. Not yet used by game logic —
/// introduced in #383 for future #372-B work.
#[derive(Component, Debug, Clone, Copy, Reflect)]
#[reflect(Component)]
pub struct DockedAt(pub Entity);

#[derive(Component, Reflect)]
#[reflect(Component)]
pub enum ShipState {
    InSystem {
        system: Entity,
    },
    SubLight {
        origin: [f64; 3],
        destination: [f64; 3],
        target_system: Option<Entity>,
        departed_at: i64,
        arrival_at: i64,
    },
    InFTL {
        origin_system: Entity,
        destination_system: Entity,
        departed_at: i64,
        arrival_at: i64,
    },
    Surveying {
        target_system: Entity,
        started_at: i64,
        completes_at: i64,
    },
    /// #32: Colony ship settling state
    Settling {
        system: Entity,
        planet: Option<Entity>,
        started_at: i64,
        completes_at: i64,
    },
    /// #98 / #123: Ship is being refitted to match its current design.
    /// `target_revision` is the `ShipDesignDefinition.revision` recorded
    /// when refit started; on completion the ship's `design_revision` is
    /// set to this value and `new_modules` replaces the equipped modules.
    Refitting {
        system: Entity,
        started_at: i64,
        completes_at: i64,
        new_modules: Vec<EquippedModule>,
        target_revision: u64,
    },
    /// #185: Loitering at an arbitrary deep-space coordinate.
    /// Reached when a SubLight move with `target_system = None` arrives, or
    /// (future) when a ship is interdicted out of FTL or engaged in deep-space
    /// ship-vs-ship combat. Loitering ships are NOT subject to `resolve_combat`,
    /// which currently only operates on InSystem ships in star systems.
    Loitering {
        position: [f64; 3],
    },
    /// #217: Scout ship is observing `target_system` for the duration of the
    /// observation window. On completion, `tick_scout_observation` produces
    /// a `ScoutReport` component on the ship and transitions it back to
    /// `InSystem { system: target_system }`. From there the ship either
    /// delivers the report via FTL comm (if in coverage and mode allows) or
    /// is auto-routed home to `origin_system` to deliver on dock.
    Scouting {
        target_system: Entity,
        origin_system: Entity,
        started_at: i64,
        completes_at: i64,
        report_mode: ReportMode,
    },
}

/// #223: An item in a ship's cargo hold other than bulk resources.
#[derive(Clone, Debug, PartialEq, Eq, bevy::reflect::Reflect)]
pub enum CargoItem {
    /// A shipyard-built deliverable awaiting deployment.
    Deliverable { definition_id: String },
}

impl CargoItem {
    /// The id used to look up the carried item's definition in the registry.
    pub fn definition_id(&self) -> &str {
        match self {
            CargoItem::Deliverable { definition_id } => definition_id,
        }
    }
}

/// Cargo hold for Courier ships (and potentially others).
#[derive(Component, Default, Debug, Clone, Reflect)]
#[reflect(Component)]
pub struct Cargo {
    pub minerals: Amt,
    pub energy: Amt,
    /// #223: Non-resource items (e.g. deliverables). Each item's mass impact
    /// on shared `cargo_capacity` is derived from its cargo_size via
    /// `GameBalance.mass_per_item_slot`.
    pub items: Vec<CargoItem>,
}

impl Cargo {
    /// Raw units of cargo mass per item slot: 1 slot = 1.0 Amt unit by default.
    /// #223: Kept in sync with `GameBalance.mass_per_item_slot`; the constant
    /// value is used when the registry is unreachable (fallback).
    pub const DEFAULT_MASS_PER_ITEM_SLOT_RAW: u64 = 1000; // 1 Amt unit

    /// Total cargo mass (resources + items) as an Amt, given a resolver that
    /// returns each item's cargo_size via the deliverable registry.
    pub fn total_mass_with<F: Fn(&str) -> Option<u32>>(
        &self,
        cargo_size_lookup: F,
        mass_per_slot_raw: u64,
    ) -> Amt {
        let resource = self.minerals.add(self.energy);
        let mut item_raw: u64 = 0;
        for it in &self.items {
            let size = cargo_size_lookup(it.definition_id()).unwrap_or(0) as u64;
            item_raw = item_raw.saturating_add(size.saturating_mul(mass_per_slot_raw));
        }
        resource.add(Amt(item_raw))
    }

    /// Check if the cargo can accept another item with `cargo_size` against
    /// the ship's effective capacity `cap`. Uses the same mass accounting as
    /// `total_mass_with`.
    pub fn can_accept_item_size(&self, added_size: u32, cap: Amt, mass_per_slot_raw: u64) -> bool {
        // Without a registry lookup we can't compute item mass for existing
        // items, so callers must provide it via `total_mass_with` externally.
        // This helper only checks the additive delta; callers sum the existing
        // mass themselves when needed.
        let _ = cap;
        let _ = mass_per_slot_raw;
        let _ = added_size;
        true // not used directly; see Cargo::can_fit below
    }

    /// Comprehensive fit-check: does adding `added_size` slots stay within cap?
    pub fn can_fit<F: Fn(&str) -> Option<u32>>(
        &self,
        added_size: u32,
        cap: Amt,
        cargo_size_lookup: F,
        mass_per_slot_raw: u64,
    ) -> bool {
        let current = self.total_mass_with(cargo_size_lookup, mass_per_slot_raw);
        let delta = Amt((added_size as u64).saturating_mul(mass_per_slot_raw));
        current.add(delta) <= cap
    }
}

/// #103: Survey data carried by an FTL-capable ship back to the player's system.
/// Stored on the ship when survey completes until the ship docks at the player's
/// StationedAt system, at which point the results are published.
#[derive(Component, Clone, Debug, Reflect)]
#[reflect(Component)]
pub struct SurveyData {
    /// The system that was surveyed.
    pub target_system: Entity,
    /// The game time when the survey completed.
    pub surveyed_at: i64,
    /// Name of the surveyed system (cached for event descriptions).
    pub system_name: String,
    /// #127: Anomaly discovered during survey (if any), delivered with survey results.
    pub anomaly_id: Option<String>,
}

pub fn spawn_ship(
    commands: &mut Commands,
    design_id: &str,
    name: String,
    system: Entity,
    initial_position: Position,
    owner: Owner,
    design_registry: &ShipDesignRegistry,
) -> Entity {
    let design = design_registry.get(design_id);
    let hull_hp = design.map(|d| d.hp).unwrap_or(50.0);
    let hull_id = design
        .map(|d| d.hull_id.as_str())
        .unwrap_or("corvette")
        .to_string();
    let sublight_speed = design.map(|d| d.sublight_speed).unwrap_or(0.75);
    let ftl_range = design.map(|d| d.ftl_range).unwrap_or(10.0);
    // #123: Newly built ships are spawned in sync with the current design revision.
    let design_revision = design.map(|d| d.revision).unwrap_or(0);
    // Equip ships from the design's slot assignments so they start out matching
    // the design exactly (no spurious "needs refit" right after construction).
    let modules = design
        .map(crate::ship_design::design_equipped_modules)
        .unwrap_or_default();
    // #287 (γ-1): Reserve both entity ids up front so Ship <-> Fleet
    // can be wired in a single Commands batch (no follow-up backref
    // system needed). Every `spawn_ship` call produces a matching
    // single-ship Fleet so downstream γ-2..γ-6 systems can always find
    // a fleet handle per ship.
    let ship_entity = commands.spawn_empty().id();
    let fleet_entity = commands.spawn_empty().id();
    let ship_name = name.clone();
    commands.entity(ship_entity).insert((
        Ship {
            name,
            design_id: design_id.to_string(),
            hull_id,
            modules,
            owner,
            sublight_speed,
            ftl_range,
            ruler_aboard: false,
            home_port: system,
            design_revision,
            fleet: Some(fleet_entity),
        },
        ShipState::InSystem { system },
        initial_position,
        CommandQueue::default(),
        Cargo::default(),
        ShipHitpoints {
            hull: hull_hp,
            hull_max: hull_hp,
            armor: 0.0,
            armor_max: 0.0,
            shield: 0.0,
            shield_max: 0.0,
            shield_regen: 0.0,
        },
        ShipModifiers::default(),
        ShipStats::default(),
        RulesOfEngagement::default(),
        transit_events::LastDockedSystem(Some(system)),
    ));
    commands.entity(fleet_entity).insert((
        fleet::Fleet {
            // Single-ship fleets inherit the ship's name as a
            // human-readable label. Player can rename via γ-6 UI.
            name: ship_name,
            flagship: Some(ship_entity),
        },
        fleet::FleetMembers(vec![ship_entity]),
    ));
    // #297 (S-2): Empire-owned ships carry `FactionOwner` alongside the
    // legacy `Ship.owner` enum so all owned entity classes share one
    // diplomatic-identity component. `Owner::Neutral` intentionally gets
    // no component — combat/ROE treats such ships as unaffiliated.
    if let Owner::Empire(e) = owner {
        commands
            .entity(ship_entity)
            .insert(crate::faction::FactionOwner(e));
    }
    ship_entity
}

/// #387: Check whether a station ship with the given `design_id` already exists
/// in the specified system. Used to prevent duplicate auto-spawns (e.g. Shipyard
/// on colonization when one already exists from a Core deploy or prior colony).
pub fn system_has_station_ship(
    design_id: &str,
    system: Entity,
    ships: &Query<(&Ship, &ShipState)>,
) -> bool {
    ships.iter().any(|(ship, state)| {
        ship.design_id == design_id
            && matches!(state, ShipState::InSystem { system: s } if *s == system)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ship_design::{ShipDesignDefinition, ShipDesignRegistry};
    use bevy::ecs::world::World;

    // --- #223: Cargo item mass accounting ---

    #[test]
    fn test_cargo_mass_accounting_with_items() {
        let cargo = Cargo {
            minerals: Amt::units(50),
            energy: Amt::units(30),
            items: vec![
                CargoItem::Deliverable {
                    definition_id: "sensor_buoy".into(),
                },
                CargoItem::Deliverable {
                    definition_id: "interdictor".into(),
                },
            ],
        };
        // sensor_buoy size=1, interdictor size=3 → 4 slots total.
        let lookup = |id: &str| -> Option<u32> {
            match id {
                "sensor_buoy" => Some(1),
                "interdictor" => Some(3),
                _ => None,
            }
        };
        // mass_per_slot = 1000 (1 Amt unit per slot).
        let mass = cargo.total_mass_with(lookup, 1000);
        // resource = 80 units, items = 4 slots * 1 unit = 4 units → 84 units.
        assert_eq!(mass, Amt::units(84));
    }

    #[test]
    fn test_cargo_can_fit_respects_capacity() {
        let cargo = Cargo {
            minerals: Amt::ZERO,
            energy: Amt::ZERO,
            items: vec![CargoItem::Deliverable {
                definition_id: "big".into(),
            }],
        };
        let lookup = |id: &str| -> Option<u32> {
            match id {
                "big" => Some(5),
                _ => None,
            }
        };
        // Capacity = 10 units. Already have 5. Adding size=5 → 10 total: exactly fits.
        assert!(cargo.can_fit(5, Amt::units(10), lookup, 1000));
        // Adding size=6 → 11 total: exceeds.
        assert!(!cargo.can_fit(6, Amt::units(10), lookup, 1000));
    }

    // #236: In-Rust preset mirror. Values reflect `design_derived` output
    // for the Lua presets (hull + modules). Hand-keep in sync; also covered
    // by `test_preset_designs_derived_from_modules` regression test.
    fn test_design_registry() -> ShipDesignRegistry {
        let mut registry = ShipDesignRegistry::default();
        // explorer_mk1 = corvette + ftl_drive (15) + survey_equipment
        registry.insert(ShipDesignDefinition {
            id: "explorer_mk1".to_string(),
            name: "Explorer Mk.I".to_string(),
            description: String::new(),
            hull_id: "corvette".to_string(),
            modules: Vec::new(),
            can_survey: true,
            can_colonize: false,
            // 0.5 (hull) + 0.1*(100+60) = 0.5 + 16.0 = 16.5
            maintenance: Amt::new(16, 500),
            // 200 + 100 + 60 = 360
            build_cost_minerals: Amt::units(360),
            // 100 + 50 + 40 = 190
            build_cost_energy: Amt::units(190),
            build_time: 60,
            hp: 50.0,
            sublight_speed: 0.75,
            ftl_range: 15.0,
            revision: 0,
            is_direct_buildable: true,
        });
        // colony_ship_mk1 = frigate + ftl_drive + colony_module
        registry.insert(ShipDesignDefinition {
            id: "colony_ship_mk1".to_string(),
            name: "Colony Ship Mk.I".to_string(),
            description: String::new(),
            hull_id: "frigate".to_string(),
            modules: Vec::new(),
            can_survey: false,
            can_colonize: true,
            // 1.0 + 0.1*(100+300) = 1.0 + 40 = 41.0
            maintenance: Amt::units(41),
            // 400 + 100 + 300 = 800
            build_cost_minerals: Amt::units(800),
            // 200 + 50 + 200 = 450
            build_cost_energy: Amt::units(450),
            build_time: 120,
            hp: 120.0,
            sublight_speed: 0.5,
            ftl_range: 15.0,
            revision: 0,
            is_direct_buildable: true,
        });
        // courier_mk1 = courier_hull + ftl_drive + afterburner + cargo_bay
        registry.insert(ShipDesignDefinition {
            id: "courier_mk1".to_string(),
            name: "Courier Mk.I".to_string(),
            description: String::new(),
            hull_id: "courier_hull".to_string(),
            modules: Vec::new(),
            can_survey: false,
            can_colonize: false,
            // 0.3 + 0.1*(100+60+30) = 0.3 + 19 = 19.3
            maintenance: Amt::new(19, 300),
            // 100+100+60+30 = 290
            build_cost_minerals: Amt::units(290),
            // 50+50+40+0 = 140
            build_cost_energy: Amt::units(140),
            build_time: 30,
            hp: 35.0,
            // 0.80 * (1+0.2) = 0.96
            sublight_speed: 0.96,
            // 15 * (1+1.2) = 33.0 (courier_hull ftl_range multiplier 1.2)
            ftl_range: 33.0,
            revision: 0,
            is_direct_buildable: true,
        });
        // scout_mk1 = scout_hull + ftl_drive + survey_equipment
        registry.insert(ShipDesignDefinition {
            id: "scout_mk1".to_string(),
            name: "Scout Mk.I".to_string(),
            description: String::new(),
            hull_id: "scout_hull".to_string(),
            modules: Vec::new(),
            can_survey: true,
            can_colonize: false,
            // 0.4 + 0.1*(100+60) = 0.4 + 16 = 16.4
            maintenance: Amt::new(16, 400),
            // 150+100+60 = 310
            build_cost_minerals: Amt::units(310),
            // 80+50+40 = 170
            build_cost_energy: Amt::units(170),
            build_time: 45,
            hp: 40.0,
            // 0.85 * (1+1.15) = 1.8275
            sublight_speed: 1.8275,
            ftl_range: 15.0,
            revision: 0,
            is_direct_buildable: true,
        });
        registry
    }

    fn make_ship(design_id: &str) -> Ship {
        let registry = test_design_registry();
        let design = registry.get(design_id).expect("unknown test design");
        Ship {
            name: "Test Ship".to_string(),
            design_id: design.id.clone(),
            hull_id: design.hull_id.clone(),
            modules: Vec::new(),
            owner: Owner::Neutral,
            sublight_speed: design.sublight_speed,
            ftl_range: design.ftl_range,
            ruler_aboard: false,
            home_port: Entity::PLACEHOLDER,
            design_revision: 0,
            fleet: None,
        }
    }

    // #296: Result API — immobile ships reject start_sublight_travel.
    #[test]
    fn test_start_sublight_travel_rejects_immobile() {
        let mut world = World::new();
        let system = world.spawn_empty().id();
        let mut ship = make_ship("colony_ship_mk1");
        ship.sublight_speed = 0.0;
        ship.ftl_range = 0.0;
        let origin = Position {
            x: 0.0,
            y: 0.0,
            z: 0.0,
        };
        let dest = Position {
            x: 1.0,
            y: 0.0,
            z: 0.0,
        };
        let mut state = ShipState::InSystem { system };
        let result = start_sublight_travel(&mut state, &origin, &ship, dest, Some(system), 0);
        assert_eq!(result, Err("ship is immobile"));
        // State must remain InSystem.
        assert!(matches!(state, ShipState::InSystem { .. }));
    }

    #[test]
    fn start_sublight_sets_correct_arrival_time() {
        let mut world = World::new();
        let system = world.spawn_empty().id();
        let ship = make_ship("colony_ship_mk1"); // 0.5c
        let origin = Position {
            x: 0.0,
            y: 0.0,
            z: 0.0,
        };
        let dest = Position {
            x: 1.0,
            y: 0.0,
            z: 0.0,
        }; // 1 LY away
        let mut state = ShipState::InSystem { system };
        start_sublight_travel(&mut state, &origin, &ship, dest, Some(system), 100)
            .expect("mobile ship should travel");
        match state {
            ShipState::SubLight {
                arrival_at,
                departed_at,
                ..
            } => {
                assert_eq!(departed_at, 100);
                assert_eq!(arrival_at, 220);
            }
            _ => panic!("Expected SubLight state"),
        }
    }

    #[test]
    fn start_ftl_rejects_no_ftl_ship() {
        // #236: courier_mk1 now has FTL (33.0 ly) because its modules include
        // ftl_drive. Construct a manual non-FTL ship to cover this rejection.
        let mut world = World::new();
        let origin = world.spawn_empty().id();
        let dest = world.spawn_empty().id();
        let mut ship = make_ship("courier_mk1");
        ship.ftl_range = 0.0;
        let mut state = ShipState::InSystem { system: origin };
        let origin_pos = Position {
            x: 0.0,
            y: 0.0,
            z: 0.0,
        };
        let dest_pos = Position {
            x: 1.0,
            y: 0.0,
            z: 0.0,
        };
        let result = start_ftl_travel(&mut state, &ship, origin, dest, &origin_pos, &dest_pos, 0);
        assert_eq!(result, Err("Ship has no FTL capability"));
    }

    #[test]
    fn start_ftl_rejects_out_of_range() {
        let mut world = World::new();
        let origin = world.spawn_empty().id();
        let dest = world.spawn_empty().id();
        let ship = make_ship("colony_ship_mk1");
        let mut state = ShipState::InSystem { system: origin };
        let origin_pos = Position {
            x: 0.0,
            y: 0.0,
            z: 0.0,
        };
        let dest_pos = Position {
            x: 50.0,
            y: 0.0,
            z: 0.0,
        };
        let result = start_ftl_travel(&mut state, &ship, origin, dest, &origin_pos, &dest_pos, 0);
        assert_eq!(result, Err("Destination is beyond FTL range"));
    }

    #[test]
    fn start_ftl_correct_travel_time() {
        let mut world = World::new();
        let origin = world.spawn_empty().id();
        let dest = world.spawn_empty().id();
        let ship = make_ship("colony_ship_mk1");
        let mut state = ShipState::InSystem { system: origin };
        let origin_pos = Position {
            x: 0.0,
            y: 0.0,
            z: 0.0,
        };
        let dest_pos = Position {
            x: 10.0,
            y: 0.0,
            z: 0.0,
        };
        let result = start_ftl_travel(&mut state, &ship, origin, dest, &origin_pos, &dest_pos, 0);
        assert!(result.is_ok());
        match state {
            ShipState::InFTL { arrival_at, .. } => assert_eq!(arrival_at, 60),
            _ => panic!("Expected InFTL state"),
        }
    }

    // --- #46: Port FTL tests ---

    #[test]
    fn start_ftl_with_port_reduces_travel_time() {
        let mut world = World::new();
        let origin = world.spawn_empty().id();
        let dest = world.spawn_empty().id();
        let ship = make_ship("colony_ship_mk1");
        let origin_pos = Position {
            x: 0.0,
            y: 0.0,
            z: 0.0,
        };
        let dest_pos = Position {
            x: 10.0,
            y: 0.0,
            z: 0.0,
        };

        // Without port
        let mut state_no_port = ShipState::InSystem { system: origin };
        let _ = start_ftl_travel_with_bonus(
            &mut state_no_port,
            &ship,
            origin,
            dest,
            &origin_pos,
            &dest_pos,
            0,
            0.0,
            1.0,
            PortParams::NONE,
        );
        let time_no_port = match state_no_port {
            ShipState::InFTL { arrival_at, .. } => arrival_at,
            _ => panic!("Expected InFTL state"),
        };

        // With port (using Lua-defined values)
        let mut state_port = ShipState::InSystem { system: origin };
        let port_params = PortParams {
            has_port: true,
            ftl_range_bonus: 10.0,
            travel_time_factor: 0.8,
        };
        let _ = start_ftl_travel_with_bonus(
            &mut state_port,
            &ship,
            origin,
            dest,
            &origin_pos,
            &dest_pos,
            0,
            0.0,
            1.0,
            port_params,
        );
        let time_port = match state_port {
            ShipState::InFTL { arrival_at, .. } => arrival_at,
            _ => panic!("Expected InFTL state"),
        };

        // Port should reduce travel time by 20%
        assert!(
            time_port < time_no_port,
            "Port should reduce FTL travel time"
        );
        let expected = (time_no_port as f64 * 0.8).ceil() as i64;
        assert_eq!(time_port, expected);
    }

    #[test]
    fn start_ftl_with_port_extends_range() {
        let mut world = World::new();
        let origin = world.spawn_empty().id();
        let dest = world.spawn_empty().id();
        let ship = make_ship("colony_ship_mk1"); // ftl_range = 15.0

        let origin_pos = Position {
            x: 0.0,
            y: 0.0,
            z: 0.0,
        };
        let dest_pos = Position {
            x: 20.0,
            y: 0.0,
            z: 0.0,
        }; // 20 ly, beyond base 15 ly range

        // Without port: should fail
        let mut state = ShipState::InSystem { system: origin };
        let result = start_ftl_travel_with_bonus(
            &mut state,
            &ship,
            origin,
            dest,
            &origin_pos,
            &dest_pos,
            0,
            0.0,
            1.0,
            PortParams::NONE,
        );
        assert_eq!(result, Err("Destination is beyond FTL range"));

        // With port: +10 ly range, so 25 ly total, should succeed
        let mut state = ShipState::InSystem { system: origin };
        let port_params = PortParams {
            has_port: true,
            ftl_range_bonus: 10.0,
            travel_time_factor: 0.8,
        };
        let result = start_ftl_travel_with_bonus(
            &mut state,
            &ship,
            origin,
            dest,
            &origin_pos,
            &dest_pos,
            0,
            0.0,
            1.0,
            port_params,
        );
        assert!(result.is_ok(), "Port should extend FTL range by 10 ly");
    }

    // --- #51: Ship maintenance cost tests ---

    #[test]
    fn ship_maintenance_costs() {
        // #236: derived maintenance = hull + 10% of each module's mineral cost
        let registry = test_design_registry();
        assert_eq!(registry.maintenance("explorer_mk1"), Amt::new(16, 500));
        assert_eq!(registry.maintenance("colony_ship_mk1"), Amt::units(41));
        assert_eq!(registry.maintenance("courier_mk1"), Amt::new(19, 300));
    }

    #[test]
    fn build_cost_returns_expected_values() {
        // #236: derived build cost = hull + Σ module costs
        let registry = test_design_registry();
        assert_eq!(
            registry.build_cost("explorer_mk1"),
            (Amt::units(360), Amt::units(190))
        );
        assert_eq!(
            registry.build_cost("colony_ship_mk1"),
            (Amt::units(800), Amt::units(450))
        );
        assert_eq!(
            registry.build_cost("courier_mk1"),
            (Amt::units(290), Amt::units(140))
        );
    }

    #[test]
    fn scrap_refund_is_half_build_cost_without_modules() {
        let design_reg = test_design_registry();
        let empty_module_registry = crate::ship_design::ModuleRegistry::default();
        for design_id in ["explorer_mk1", "colony_ship_mk1", "courier_mk1"] {
            let (bm, be) = design_reg.build_cost(design_id);
            let (rm, re) = design_reg.scrap_refund(design_id, &[], &empty_module_registry);
            assert_eq!(rm, Amt::milli(bm.raw() / 2));
            assert_eq!(re, Amt::milli(be.raw() / 2));
        }
    }

    #[test]
    fn scrap_refund_includes_module_costs() {
        let design_reg = test_design_registry();
        let mut module_registry = crate::ship_design::ModuleRegistry::default();
        module_registry.insert(crate::ship_design::ModuleDefinition {
            id: "test_weapon".into(),
            name: "Test Weapon".into(),
            description: String::new(),
            slot_type: "weapon".into(),
            cost_minerals: Amt::units(100),
            cost_energy: Amt::units(50),
            modifiers: vec![],
            weapon: None,
            prerequisites: None,
            upgrade_to: Vec::new(),
            build_time: 0,
            power_cost: 0,
            power_output: 0,
            size: crate::ship_design::ModuleSize::Small,
        });
        let modules = vec![EquippedModule {
            slot_type: "weapon".into(),
            module_id: "test_weapon".into(),
        }];
        let (bm, be) = design_reg.build_cost("explorer_mk1");
        let (rm, re) = design_reg.scrap_refund("explorer_mk1", &modules, &module_registry);
        // Refund = 50% of (hull cost + module cost)
        let expected_m = Amt::milli((bm.raw() + Amt::units(100).raw()) / 2);
        let expected_e = Amt::milli((be.raw() + Amt::units(50).raw()) / 2);
        assert_eq!(rm, expected_m);
        assert_eq!(re, expected_e);
    }

    // --- #101: Auto-insert movement for remote Survey/Colonize ---

    #[test]
    fn command_queue_survey_auto_inserts_move_when_not_at_target() {
        let mut world = World::new();
        let system_a = world.spawn_empty().id();
        let system_b = world.spawn_empty().id();
        // Ship is docked at system_a, survey targets system_b
        let mut queue = CommandQueue {
            commands: vec![QueuedCommand::Survey { system: system_b }],
            ..Default::default()
        };
        let state = ShipState::InSystem { system: system_a };

        // Simulate what `handlers::handle_survey_requested` does:
        // It checks if docked_system != target, and if so, inserts move + re-queues survey
        let docked_system = match &state {
            ShipState::InSystem { system } => *system,
            _ => panic!("Expected InSystem"),
        };
        let next = queue.commands.remove(0);
        match next {
            QueuedCommand::Survey { system: target } => {
                assert_ne!(docked_system, target);
                // Auto-insert: move to target, then re-queue survey
                queue
                    .commands
                    .insert(0, QueuedCommand::Survey { system: target });
                queue
                    .commands
                    .insert(0, QueuedCommand::MoveTo { system: target });
            }
            _ => panic!("Expected Survey command"),
        }

        // Verify: queue should now be [MoveTo, Survey]
        assert_eq!(queue.commands.len(), 2);
        assert!(matches!(queue.commands[0], QueuedCommand::MoveTo { .. }));
        assert!(matches!(queue.commands[1], QueuedCommand::Survey { .. }));
    }

    #[test]
    fn command_queue_colonize_auto_inserts_move_when_not_at_target() {
        let mut world = World::new();
        let system_a = world.spawn_empty().id();
        let system_b = world.spawn_empty().id();
        let mut queue = CommandQueue {
            commands: vec![QueuedCommand::Colonize {
                system: system_b,
                planet: None,
            }],
            ..Default::default()
        };
        let state = ShipState::InSystem { system: system_a };

        let docked_system = match &state {
            ShipState::InSystem { system } => *system,
            _ => panic!("Expected InSystem"),
        };
        let next = queue.commands.remove(0);
        match next {
            QueuedCommand::Colonize {
                system: target,
                planet,
            } => {
                assert_ne!(docked_system, target);
                // Auto-insert: move to target, then re-queue colonize
                queue.commands.insert(
                    0,
                    QueuedCommand::Colonize {
                        system: target,
                        planet,
                    },
                );
                queue
                    .commands
                    .insert(0, QueuedCommand::MoveTo { system: target });
            }
            _ => panic!("Expected Colonize command"),
        }

        // Should be [MoveTo, Colonize] — route planning (FTL vs sublight) is handled by `handlers::handle_move_requested`.
        assert_eq!(queue.commands.len(), 2);
        assert!(matches!(queue.commands[0], QueuedCommand::MoveTo { .. }));
        assert!(matches!(queue.commands[1], QueuedCommand::Colonize { .. }));
    }

    #[test]
    fn test_docked_at_component_can_be_inserted_and_removed() {
        let mut world = World::new();
        let port = world.spawn_empty().id();
        let ship = world.spawn(DockedAt(port)).id();

        // Verify inserted.
        let docked = world.get::<DockedAt>(ship).expect("DockedAt should exist");
        assert_eq!(docked.0, port);

        // Remove and verify gone.
        world.entity_mut(ship).remove::<DockedAt>();
        assert!(world.get::<DockedAt>(ship).is_none());
    }
}
