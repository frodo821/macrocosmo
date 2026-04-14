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

/// UI-to-dispatcher queue for colony build commands. UI code pushes
/// `PendingColonyDispatch` entries; `dispatch_pending_colony_commands`
/// drains and turns each into a `PendingCommand` with light-speed delay.
#[derive(Resource, Default)]
pub struct PendingColonyDispatches {
    pub queue: Vec<PendingColonyDispatch>,
}

pub struct PendingColonyDispatch {
    pub target_system: Entity,
    pub command: ColonyCommand,
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
                    info!(
                        "Report received (information age: {} sd)",
                        age
                    );
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
/// Build-related payloads (`Colony(ColonyCommand)`) are deliberately minimal —
/// cost/time/refund amounts are re-resolved on arrival from the target's
/// current `BuildingRegistry` + `ConstructionParams`. `QueueDeliverableBuild`
/// is the exception (defs live in a separate registry; see variant doc).
#[derive(Clone, Debug)]
pub enum RemoteCommand {
    BuildShip { design_id: String },
    SetProductionFocus { minerals: f64, energy: f64, research: f64 },
    Colony(ColonyCommand),
}

/// A colony-scoped remote command. `target_planet = Some(p)` addresses a
/// planet-level `BuildingQueue`/`Buildings`; `None` addresses the
/// system-level `SystemBuildingQueue`/`SystemBuildings` on `target_system`.
/// Some variants (`QueueShipBuild`, `QueueDeliverableBuild`) carry their
/// own `host_colony` and ignore `target_planet`.
#[derive(Clone, Debug)]
pub struct ColonyCommand {
    pub target_planet: Option<Entity>,
    pub kind: ColonyCommandKind,
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
                ColonyCommandKind::QueueBuilding { building_id, target_slot } => {
                    format!("Build {} → slot {}", building_id, target_slot)
                }
                ColonyCommandKind::DemolishBuilding { target_slot } => {
                    format!("Demolish slot {}", target_slot)
                }
                ColonyCommandKind::UpgradeBuilding { slot_index, target_id } => {
                    format!("Upgrade slot {} → {}", slot_index, target_id)
                }
                ColonyCommandKind::QueueShipBuild { design_id, .. } => {
                    format!("Build ship: {}", design_id)
                }
                ColonyCommandKind::QueueDeliverableBuild { display_name, .. } => {
                    format!("Build deliverable: {}", display_name)
                }
            },
        }
    }
}

#[derive(Clone, Debug)]
pub enum ColonyCommandKind {
    /// Enqueue construction of `building_id` into `target_slot`.
    QueueBuilding {
        building_id: String,
        target_slot: usize,
    },
    /// Enqueue demolition of whatever occupies `target_slot`.
    DemolishBuilding { target_slot: usize },
    /// Enqueue an upgrade of the building in `slot_index` to `target_id`.
    UpgradeBuilding {
        slot_index: usize,
        target_id: String,
    },
    /// Enqueue a ship (or deliverable) build on `host_colony`'s `BuildQueue`.
    QueueShipBuild {
        host_colony: Entity,
        design_id: String,
        build_kind: crate::colony::BuildKind,
    },
    /// Enqueue a deliverable build on `host_colony`'s `BuildQueue`. Full
    /// payload (def_id/display_name/cargo_size/cost/time) is frozen at
    /// send time because deliverable defs live in `StructureRegistry`,
    /// not `ShipDesignRegistry`, so arrival-time re-resolution would
    /// require a second registry lookup path.
    QueueDeliverableBuild {
        host_colony: Entity,
        def_id: String,
        display_name: String,
        cargo_size: u32,
        minerals_cost: crate::amount::Amt,
        energy_cost: crate::amount::Amt,
        build_time: i64,
    },
}

/// Tracks command status for UI display.
#[derive(Resource, Component, Default)]
pub struct CommandLog {
    pub entries: Vec<CommandLogEntry>,
}

pub struct CommandLogEntry {
    pub description: String,
    pub sent_at: i64,
    pub arrives_at: i64,
    pub arrived: bool,
}

pub fn dispatch_pending_colony_commands(
    mut commands: Commands,
    clock: Res<GameClock>,
    mut queue: ResMut<PendingColonyDispatches>,
    stars: Query<&crate::components::Position, With<crate::galaxy::StarSystem>>,
    ship_positions: Query<&crate::components::Position, With<crate::ship::Ship>>,
    player_q: Query<
        (&crate::player::StationedAt, Option<&crate::player::AboardShip>),
        With<crate::player::Player>,
    >,
    mut empire_q: Query<&mut CommandLog, With<crate::player::PlayerEmpire>>,
) {
    if queue.queue.is_empty() {
        return;
    }
    let Ok(mut command_log) = empire_q.single_mut() else {
        queue.queue.clear();
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
        warn!("dispatch_pending_colony_commands: cannot resolve player origin, dropping commands");
        queue.queue.clear();
        return;
    };

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
            RemoteCommand::Colony(dispatch.command),
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

    command_log.entries.push(CommandLogEntry {
        description: command.describe(),
        sent_at,
        arrives_at,
        arrived: false,
    });

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

            if let RemoteCommand::Colony(cc) = &cmd.command {
                crate::colony::apply_colony_command(
                    cc,
                    cmd.target_system,
                    &building_registry,
                    &ship_design_registry,
                    bldg_cost_mod,
                    bldg_time_mod,
                    &mut colonies,
                    &mut sys_buildings_q,
                );
            }

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
