use bevy::prelude::*;

use crate::physics;
use crate::time_system::GameClock;

pub struct CommunicationPlugin;

impl Plugin for CommunicationPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<PendingColonyDispatches>();
        // Dispatcher chained before process_pending_commands so local
        // (zero-delay) commands apply within the same Update pass.
        app.add_systems(
            Update,
            (
                process_messages,
                process_courier_ships,
                dispatch_pending_colony_commands,
                process_pending_commands,
            )
                .chain(),
        );
    }
}

/// Max consecutive frames the dispatcher will retain an un-dispatchable
/// queue (no `PlayerEmpire` / unresolvable origin) before giving up. At
/// 60 fps this is ~5 seconds — long enough to paper over load/teardown/
/// observer-mode transients, short enough that observation sessions do
/// not accumulate unbounded state. See #276.
pub const MAX_DISPATCH_RETRY_FRAMES: u32 = 300;

/// UI-to-dispatcher queue for colony build commands. UI code pushes
/// `PendingColonyDispatch` entries; `dispatch_pending_colony_commands`
/// drains and turns each into a `PendingCommand` with light-speed delay.
///
/// When the dispatcher cannot resolve a `PlayerEmpire` or the player's
/// origin position (e.g. during load/teardown or observer mode), the
/// queue is retained instead of cleared so that clicks are not silently
/// lost. `retry_frames` tracks consecutive unresolved frames; once it
/// reaches `MAX_DISPATCH_RETRY_FRAMES` the queue is dropped with a
/// warning to keep observation sessions bounded.
#[derive(Resource, Default)]
pub struct PendingColonyDispatches {
    pub queue: Vec<PendingColonyDispatch>,
    /// Consecutive frames the dispatcher has seen a non-empty queue
    /// without being able to resolve empire/origin. Reset to 0 on any
    /// successful dispatch frame or when the queue is empty.
    pub retry_frames: u32,
}

pub struct PendingColonyDispatch {
    pub target_system: Entity,
    pub command: RemoteCommand,
}

/// A message in transit (light-speed or via courier)
#[derive(Component)]
pub struct Message {
    /// Source position when sent
    pub origin: [f64; 3],
    /// Destination position
    pub destination: [f64; 3],
    /// Hexadies when the message was sent
    pub sent_at: i64,
    /// Hexadies when the message will arrive
    pub arrives_at: i64,
    /// Content of the message
    pub content: MessageContent,
}

#[derive(Clone, Debug)]
pub enum MessageContent {
    /// A command from the player to a remote system
    Command(CommandPayload),
    /// An information report from a remote system
    Report(ReportPayload),
}

#[derive(Clone, Debug)]
pub struct CommandPayload {
    pub target_system: Entity,
    pub command_type: CommandType,
}

#[derive(Clone, Debug)]
pub enum CommandType {
    /// Update the autonomous AI's standing orders
    UpdateOrders,
    /// Direct a specific action
    DirectAction(String),
}

#[derive(Clone, Debug)]
pub struct ReportPayload {
    pub source_system: Entity,
    /// Hexadies when this information was current
    pub info_timestamp: i64,
}

/// A courier ship carrying messages physically
#[derive(Component)]
pub struct CourierShip {
    pub origin: [f64; 3],
    pub destination: [f64; 3],
    pub speed_fraction: f64,
    pub departed_at: i64,
    pub arrives_at: i64,
    pub carrying: Vec<MessageContent>,
}

pub fn process_messages(
    mut commands: Commands,
    clock: Res<GameClock>,
    messages: Query<(Entity, &Message)>,
) {
    for (entity, msg) in &messages {
        if clock.elapsed >= msg.arrives_at {
            match &msg.content {
                MessageContent::Command(cmd) => {
                    let delay = msg.arrives_at - msg.sent_at;
                    info!(
                        "Command arrived at destination (delay: {} sd): {:?}",
                        delay, cmd.command_type
                    );
                }
                MessageContent::Report(report) => {
                    let age = clock.elapsed - report.info_timestamp;
                    info!("Report received (information age: {} sd)", age);
                }
            }
            commands.entity(entity).despawn();
        }
    }
}

pub fn process_courier_ships(
    mut commands: Commands,
    clock: Res<GameClock>,
    couriers: Query<(Entity, &CourierShip)>,
) {
    for (entity, courier) in &couriers {
        if clock.elapsed >= courier.arrives_at {
            let travel_time = courier.arrives_at - courier.departed_at;
            info!(
                "Courier ship arrived (travel time: {} sd, carried {} messages)",
                travel_time,
                courier.carrying.len()
            );
            commands.entity(entity).despawn();
        }
    }
}

// ---------------------------------------------------------------------------
// Remote commands with delay tracking
// ---------------------------------------------------------------------------

/// A command the player has issued to a remote system that hasn't arrived yet.
#[derive(Component)]
pub struct PendingCommand {
    pub target_system: Entity,
    pub command: RemoteCommand,
    pub sent_at: i64,
    pub arrives_at: i64,
    pub origin_pos: [f64; 3],
    pub destination_pos: [f64; 3],
}

/// The kinds of remote commands a player can issue.
///
/// Building slot ops (`Colony(ColonyCommand)`) are deliberately minimal —
/// cost/time/refund amounts are re-resolved on arrival from the target's
/// current `BuildingRegistry` + `ConstructionParams`. Ship / deliverable
/// builds carry their own `host_colony` entity; `ShipBuild` re-resolves
/// from `ShipDesignRegistry` at arrival while `DeliverableBuild` freezes
/// the full payload at send time (defs live in `StructureRegistry`).
#[derive(Clone, Debug)]
pub enum RemoteCommand {
    BuildShip {
        design_id: String,
    },
    SetProductionFocus {
        minerals: f64,
        energy: f64,
        research: f64,
    },
    Colony(ColonyCommand),
    ShipBuild {
        host_colony: Entity,
        design_id: String,
        build_kind: crate::colony::BuildKind,
    },
    DeliverableBuild {
        host_colony: Entity,
        def_id: String,
        display_name: String,
        cargo_size: u32,
        minerals_cost: crate::amount::Amt,
        energy_cost: crate::amount::Amt,
        build_time: i64,
    },
    /// #275: Cancel a planet- or system-level building order (construction,
    /// demolition, or upgrade) by stable `order_id`. Scope is derived at
    /// arrival by which queue holds the id — the cancel command itself
    /// doesn't carry scope, which keeps the send-side UI simple and
    /// robust against queue shifts during light-speed transit.
    CancelBuildingOrder {
        order_id: u64,
    },
    /// #275: Cancel a ship / deliverable `BuildOrder` by stable
    /// `order_id`. Ship orders live on a specific `host_colony`'s
    /// `BuildQueue`, so we carry that entity explicitly — the id alone
    /// isn't scoped across colonies.
    CancelShipOrder {
        host_colony: Entity,
        order_id: u64,
    },
}

/// Building-slot op against the target system. `scope` picks planet-level
/// vs system-level; `kind` is the action.
#[derive(Clone, Debug)]
pub struct ColonyCommand {
    pub scope: BuildingScope,
    pub kind: BuildingKind,
}

#[derive(Clone, Copy, Debug)]
pub enum BuildingScope {
    /// Acts on the planet-level `BuildingQueue` of the colony on this planet.
    Planet(Entity),
    /// Acts on the system-level `SystemBuildingQueue` of the target system.
    System,
}

#[derive(Clone, Debug)]
pub enum BuildingKind {
    Queue {
        building_id: String,
        target_slot: usize,
    },
    Demolish {
        target_slot: usize,
    },
    Upgrade {
        slot_index: usize,
        target_id: String,
    },
}

impl RemoteCommand {
    /// Short player-facing label. Used in CommandLog entries and the
    /// in-flight list so players don't see raw `{:?}` output with entity
    /// indices.
    pub fn describe(&self) -> String {
        match self {
            RemoteCommand::BuildShip { design_id } => format!("Build ship: {}", design_id),
            RemoteCommand::SetProductionFocus { .. } => "Set production focus".to_string(),
            RemoteCommand::Colony(cc) => match &cc.kind {
                BuildingKind::Queue {
                    building_id,
                    target_slot,
                } => {
                    format!("Build {} → slot {}", building_id, target_slot)
                }
                BuildingKind::Demolish { target_slot } => {
                    format!("Demolish slot {}", target_slot)
                }
                BuildingKind::Upgrade {
                    slot_index,
                    target_id,
                } => {
                    format!("Upgrade slot {} → {}", slot_index, target_id)
                }
            },
            RemoteCommand::ShipBuild { design_id, .. } => format!("Build ship: {}", design_id),
            RemoteCommand::DeliverableBuild { display_name, .. } => {
                format!("Build deliverable: {}", display_name)
            }
            RemoteCommand::CancelBuildingOrder { order_id } => {
                format!("Cancel building order #{}", order_id)
            }
            RemoteCommand::CancelShipOrder { order_id, .. } => {
                format!("Cancel ship order #{}", order_id)
            }
        }
    }
}

/// Tracks command status for UI display.
#[derive(Resource, Component, Default)]
pub struct CommandLog {
    pub entries: Vec<CommandLogEntry>,
}

/// Terminal disposition of a locally-dispatched command (#334). The legacy
/// `arrived` boolean is preserved for remote (`PendingCommand`) entries;
/// `status` carries richer information for dispatcher/handler-driven
/// commands. Defaults to `Pending` so save fixtures that predate this
/// field decode unchanged.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum CommandLogStatus {
    /// Legacy remote command awaiting light-speed arrival (kept as the
    /// default so existing entries continue to display unchanged).
    #[default]
    Pending,
    /// Dispatcher emitted a `CommandRequested`; handler has not yet
    /// produced a terminal `CommandExecuted`.
    Dispatched,
    /// Handler emitted `CommandExecuted { result: Ok }`.
    Executed,
    /// Handler (or dispatcher-side validation) rejected the command.
    Rejected { reason: String },
    /// Handler emitted `CommandExecuted { result: Deferred }` — an async
    /// follow-up will finalize via another `CommandExecuted`.
    Deferred,
}

pub struct CommandLogEntry {
    pub description: String,
    pub sent_at: i64,
    pub arrives_at: i64,
    /// Legacy UI flag — flipped to `true` when a remote `PendingCommand`
    /// arrives at its target. For dispatcher/handler-driven commands
    /// (#334) this is also set to `true` whenever `status` becomes
    /// `Executed` / `Rejected` so bottom-bar rendering stays consistent
    /// with the pre-refactor UX.
    pub arrived: bool,
    /// #334: Stable identifier for dispatcher-allocated commands. `None`
    /// for legacy remote-pending entries (pre-dispatcher code path).
    pub command_id: Option<crate::ship::command_events::CommandId>,
    /// #334: Per-entry lifecycle state.
    pub status: CommandLogStatus,
    /// #334: Timestamp the handler emitted its terminal `CommandExecuted`.
    /// `None` for entries still in flight / legacy remote entries.
    pub executed_at: Option<i64>,
}

impl CommandLogEntry {
    /// Convenience constructor for legacy remote-pending entries.
    pub fn new_pending(description: String, sent_at: i64, arrives_at: i64) -> Self {
        Self {
            description,
            sent_at,
            arrives_at,
            arrived: false,
            command_id: None,
            status: CommandLogStatus::Pending,
            executed_at: None,
        }
    }

    /// #334: Dispatcher-side constructor (local command, zero light-speed
    /// delay). `arrives_at == sent_at` so the UI "eta" renders as arrived
    /// immediately while `status` tracks the true lifecycle.
    pub fn new_dispatched(
        description: String,
        sent_at: i64,
        command_id: crate::ship::command_events::CommandId,
    ) -> Self {
        Self {
            description,
            sent_at,
            arrives_at: sent_at,
            arrived: false,
            command_id: Some(command_id),
            status: CommandLogStatus::Dispatched,
            executed_at: None,
        }
    }
}

pub fn dispatch_pending_colony_commands(
    mut commands: Commands,
    clock: Res<GameClock>,
    mut queue: ResMut<PendingColonyDispatches>,
    stars: Query<&crate::components::Position, With<crate::galaxy::StarSystem>>,
    ship_positions: Query<&crate::components::Position, With<crate::ship::Ship>>,
    player_q: Query<
        (
            &crate::player::StationedAt,
            Option<&crate::player::AboardShip>,
        ),
        With<crate::player::Player>,
    >,
    mut empire_q: Query<&mut CommandLog, With<crate::player::PlayerEmpire>>,
) {
    if queue.queue.is_empty() {
        queue.retry_frames = 0;
        return;
    }
    let Ok(mut command_log) = empire_q.single_mut() else {
        // No PlayerEmpire: load/teardown/observer mode. Retain the queue
        // for a bounded number of frames so transient unavailability
        // does not silently eat player clicks (#276).
        queue.retry_frames = queue.retry_frames.saturating_add(1);
        if queue.retry_frames >= MAX_DISPATCH_RETRY_FRAMES {
            warn!(
                "dispatch_pending_colony_commands: no PlayerEmpire for {} frames, dropping {} queued command(s)",
                queue.retry_frames,
                queue.queue.len()
            );
            queue.queue.clear();
            queue.retry_frames = 0;
        }
        return;
    };

    // Resolve player origin position.
    let origin = player_q
        .iter()
        .next()
        .and_then(|(stationed, aboard)| match aboard {
            Some(ab) => ship_positions.get(ab.ship).ok().map(|p| p.as_array()),
            None => stars.get(stationed.system).ok().map(|p| p.as_array()),
        });
    let Some(origin) = origin else {
        // Empire exists but the player's position is indeterminate
        // (e.g. stationed-at entity despawned mid-frame). Same bounded
        // retry policy as the no-empire case above.
        queue.retry_frames = queue.retry_frames.saturating_add(1);
        if queue.retry_frames >= MAX_DISPATCH_RETRY_FRAMES {
            warn!(
                "dispatch_pending_colony_commands: cannot resolve player origin for {} frames, dropping {} queued command(s)",
                queue.retry_frames,
                queue.queue.len()
            );
            queue.queue.clear();
            queue.retry_frames = 0;
        }
        return;
    };

    // Successful dispatch path — clear the retry counter.
    queue.retry_frames = 0;
    for dispatch in queue.queue.drain(..) {
        let Ok(target_pos) = stars.get(dispatch.target_system) else {
            warn!(
                "dispatch_pending_colony_commands: target_system {:?} has no Position",
                dispatch.target_system
            );
            continue;
        };
        let destination = target_pos.as_array();
        send_remote_command(
            &mut commands,
            origin,
            destination,
            clock.elapsed,
            dispatch.command,
            dispatch.target_system,
            &mut command_log,
        );
    }
}

/// Send a remote command from `origin` to `destination`. The command will
/// travel at light-speed and arrive after the corresponding delay.
pub fn send_remote_command(
    commands: &mut Commands,
    origin: [f64; 3],
    destination: [f64; 3],
    sent_at: i64,
    command: RemoteCommand,
    target_system: Entity,
    command_log: &mut CommandLog,
) {
    let distance = physics::distance_ly_arr(origin, destination);
    let delay = physics::light_delay_hexadies(distance);
    let arrives_at = sent_at + delay;

    command_log.entries.push(CommandLogEntry::new_pending(
        command.describe(),
        sent_at,
        arrives_at,
    ));

    commands.spawn(PendingCommand {
        target_system,
        command,
        sent_at,
        arrives_at,
        origin_pos: origin,
        destination_pos: destination,
    });
}

pub fn process_pending_commands(
    mut commands: Commands,
    clock: Res<GameClock>,
    pending: Query<(Entity, &PendingCommand)>,
    mut empire_q: Query<
        (&mut CommandLog, &crate::colony::ConstructionParams),
        With<crate::player::PlayerEmpire>,
    >,
    building_registry: Res<crate::scripting::building_api::BuildingRegistry>,
    ship_design_registry: Res<crate::ship_design::ShipDesignRegistry>,
    mut colonies: crate::colony::remote::ApplyColoniesQuery,
    mut sys_buildings_q: crate::colony::remote::ApplySystemBuildingsQuery,
    planets: crate::colony::remote::ApplyPlanetsQuery,
    core_ships: Query<&crate::galaxy::AtSystem, With<crate::ship::CoreShip>>,
    station_ships: crate::colony::remote::ApplyStationShipQuery,
) {
    let Ok((mut command_log, construction_params)) = empire_q.single_mut() else {
        return;
    };
    let bldg_cost_mod = construction_params.building_cost_modifier.final_value();
    let bldg_time_mod = construction_params
        .building_build_time_modifier
        .final_value();

    for (entity, cmd) in &pending {
        if clock.elapsed >= cmd.arrives_at {
            let delay = cmd.arrives_at - cmd.sent_at;
            info!(
                "Remote command arrived at target (delay: {} sd): {}",
                delay,
                cmd.command.describe()
            );

            let system_has_core = core_ships.iter().any(|at| at.0 == cmd.target_system);
            crate::colony::apply_remote_command(
                &cmd.command,
                cmd.target_system,
                &building_registry,
                &ship_design_registry,
                bldg_cost_mod,
                bldg_time_mod,
                &mut colonies,
                &mut sys_buildings_q,
                &planets,
                system_has_core,
                &station_ships,
            );

            for entry in command_log.entries.iter_mut() {
                if entry.sent_at == cmd.sent_at
                    && entry.arrives_at == cmd.arrives_at
                    && !entry.arrived
                {
                    entry.arrived = true;
                    break;
                }
            }

            commands.entity(entity).despawn();
        }
    }
}

// ---------------------------------------------------------------------------
// Existing helpers
// ---------------------------------------------------------------------------

/// Helper: send a light-speed message between two points
pub fn send_light_message(
    commands: &mut Commands,
    origin: [f64; 3],
    destination: [f64; 3],
    sent_at: i64,
    content: MessageContent,
) {
    let distance = physics::distance_ly_arr(origin, destination);
    let delay = physics::light_delay_hexadies(distance);

    commands.spawn(Message {
        origin,
        destination,
        sent_at,
        arrives_at: sent_at + delay,
        content,
    });
}

/// Helper: dispatch a courier ship
pub fn dispatch_courier(
    commands: &mut Commands,
    origin: [f64; 3],
    destination: [f64; 3],
    speed_fraction: f64,
    departed_at: i64,
    messages: Vec<MessageContent>,
) {
    let distance = physics::distance_ly_arr(origin, destination);
    let travel_time = physics::sublight_travel_hexadies(distance, speed_fraction);

    commands.spawn(CourierShip {
        origin,
        destination,
        speed_fraction,
        departed_at,
        arrives_at: departed_at + travel_time,
        carrying: messages,
    });
}
