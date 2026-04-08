use bevy::prelude::*;

use crate::physics;
use crate::time_system::GameClock;

pub struct CommunicationPlugin;

impl Plugin for CommunicationPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Update, (process_messages, process_courier_ships));
    }
}

/// A message in transit (light-speed or via courier)
#[derive(Component)]
pub struct Message {
    /// Source position when sent
    pub origin: [f64; 3],
    /// Destination position
    pub destination: [f64; 3],
    /// Sexadie when the message was sent
    pub sent_at: i64,
    /// Sexadie when the message will arrive
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
    /// Sexadie when this information was current
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

fn process_messages(
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

fn process_courier_ships(
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

/// Helper: send a light-speed message between two points
pub fn send_light_message(
    commands: &mut Commands,
    origin: [f64; 3],
    destination: [f64; 3],
    sent_at: i64,
    content: MessageContent,
) {
    let distance = physics::distance_ly_arr(origin, destination);
    let delay = physics::light_delay_sexadies(distance);

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
    let travel_time = physics::sublight_travel_sexadies(distance, speed_fraction);

    commands.spawn(CourierShip {
        origin,
        destination,
        speed_fraction,
        departed_at,
        arrives_at: departed_at + travel_time,
        carrying: messages,
    });
}
