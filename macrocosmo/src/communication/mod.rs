use bevy::prelude::*;

use crate::physics;
use crate::time_system::GameClock;

pub struct CommunicationPlugin;

impl Plugin for CommunicationPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<PendingColonyDispatches>();
        app.add_systems(
            Update,
            (
                process_messages,
                process_courier_ships,
                // #270: Must drain BEFORE process_pending_commands so
                // zero-delay (local) dispatches arrive the same frame.
                dispatch_pending_colony_commands,
                process_pending_commands,
            )
                .chain(),
        );
    }
}

/// #270: UI-to-dispatcher queue for colony build commands. UI code pushes
/// `PendingColonyDispatch` entries from within egui systems;
/// `dispatch_pending_colony_commands` drains them during `Update` and turns
/// each entry into a `PendingCommand` with the appropriate light-speed
/// delay (zero if the player is at the target system).
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
#[derive(Clone, Debug)]
pub enum RemoteCommand {
    BuildShip { design_id: String },
    SetProductionFocus { minerals: f64, energy: f64, research: f64 },
    /// #270: Colony or system build/demolish/upgrade/cancel command routed
    /// through light-speed delay. Cost/time/refund amounts are NOT carried in
    /// the payload — they are computed on arrival from the current
    /// `BuildingRegistry` / `ShipDesignRegistry` at the target. This keeps the
    /// payload small and matches the existing `BuildShip` convention.
    Colony(ColonyCommand),
}

/// #270: A colony-scoped remote command. `target_planet = Some(planet)`
/// addresses a planet-level `BuildingQueue`/`Buildings`; `None` addresses the
/// system-level `SystemBuildingQueue`/`SystemBuildings` on the target system.
#[derive(Clone, Debug)]
pub struct ColonyCommand {
    pub target_planet: Option<Entity>,
    pub kind: ColonyCommandKind,
}

/// #270: Payload-free colony command variants. Arrival handler looks up the
/// current cost/time/refund from registries when applying the command.
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
    /// Cancel the (head of the) pending build order targeting `target_slot`.
    CancelBuildingOrder { target_slot: usize },
    /// Enqueue a ship (or deliverable) build on `host_colony`'s `BuildQueue`.
    QueueShipBuild {
        host_colony: Entity,
        design_id: String,
        build_kind: crate::colony::BuildKind,
    },
    /// Cancel a ship build order on `host_colony`'s `BuildQueue` at `queue_index`.
    /// NOTE: by-index cancel is best-effort — queues can shift between
    /// dispatch and arrival. Stable order-ids are a follow-up (see #270 plan).
    CancelShipOrder {
        host_colony: Entity,
        queue_index: usize,
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

/// #270: Drain `PendingColonyDispatches` and turn each entry into a
/// `PendingCommand` with the appropriate light-speed delay. Runs in
/// `Update` before `process_pending_commands` so local (zero-delay)
/// commands are applied the same frame.
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
        description: format!("{:?}", command),
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
    mut colonies: Query<(
        &crate::colony::Colony,
        &crate::colony::Buildings,
        &mut crate::colony::BuildingQueue,
        &mut crate::colony::BuildQueue,
    )>,
    mut sys_buildings_q: Query<(
        &crate::colony::SystemBuildings,
        &mut crate::colony::SystemBuildingQueue,
    )>,
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
                "Remote command arrived at target (delay: {} sd): {:?}",
                delay, cmd.command
            );

            // #270: Dispatch colony-scoped commands on arrival.
            if let RemoteCommand::Colony(cc) = &cmd.command {
                apply_colony_command(
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
            // NOTE: BuildShip / SetProductionFocus remain as pre-#270 "orphan
            // API" — no UI wires them today. They'll be wired when the
            // SetProductionFocus UI lands, tracked separately.

            // Mark the matching log entry as arrived.
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

/// #270: Apply a `ColonyCommand` to the target queues. Cost / time / refund
/// amounts are resolved here from the current registry state — the payload
/// only carries ids and slot indices.
fn apply_colony_command(
    cc: &ColonyCommand,
    target_system: Entity,
    br: &crate::scripting::building_api::BuildingRegistry,
    sdr: &crate::ship_design::ShipDesignRegistry,
    bldg_cost_mod: crate::amount::Amt,
    bldg_time_mod: crate::amount::Amt,
    colonies: &mut Query<(
        &crate::colony::Colony,
        &crate::colony::Buildings,
        &mut crate::colony::BuildingQueue,
        &mut crate::colony::BuildQueue,
    )>,
    sys_buildings_q: &mut Query<(
        &crate::colony::SystemBuildings,
        &mut crate::colony::SystemBuildingQueue,
    )>,
) {
    use crate::amount::Amt;
    use crate::colony::{BuildOrder, BuildingOrder, DemolitionOrder, UpgradeOrder};
    use crate::scripting::building_api::BuildingId;

    match &cc.kind {
        ColonyCommandKind::QueueBuilding {
            building_id,
            target_slot,
        } => {
            let Some(def) = br.get(building_id) else {
                warn!("QueueBuilding: unknown building_id '{}'", building_id);
                return;
            };
            let (base_m, base_e) = def.build_cost();
            let eff_m = base_m.mul_amt(bldg_cost_mod);
            let eff_e = base_e.mul_amt(bldg_cost_mod);
            let eff_time = (def.build_time as f64 * bldg_time_mod.to_f64()).ceil() as i64;
            let order = BuildingOrder {
                building_id: BuildingId::new(building_id),
                target_slot: *target_slot,
                minerals_remaining: eff_m,
                energy_remaining: eff_e,
                build_time_remaining: eff_time,
            };
            match cc.target_planet {
                Some(planet) => {
                    push_planet_building_order(planet, order, colonies);
                }
                None => {
                    if let Ok((_, mut sbq)) = sys_buildings_q.get_mut(target_system) {
                        sbq.queue.push(order);
                    } else {
                        warn!(
                            "QueueBuilding (system): target_system {:?} has no SystemBuildingQueue",
                            target_system
                        );
                    }
                }
            }
        }
        ColonyCommandKind::DemolishBuilding { target_slot } => match cc.target_planet {
            Some(planet) => {
                let mut found = false;
                for (colony, buildings, mut bq, _) in colonies.iter_mut() {
                    if colony.planet != planet {
                        continue;
                    }
                    found = true;
                    let Some(Some(bid)) = buildings.slots.get(*target_slot).cloned() else {
                        warn!(
                            "DemolishBuilding (planet): slot {} is empty or out of bounds",
                            target_slot
                        );
                        break;
                    };
                    let def = br.get(bid.as_str());
                    let (m_ref, e_ref) =
                        def.map(|d| d.demolition_refund()).unwrap_or((Amt::ZERO, Amt::ZERO));
                    let demo_time = def.map(|d| d.demolition_time()).unwrap_or(0);
                    bq.demolition_queue.push(DemolitionOrder {
                        target_slot: *target_slot,
                        building_id: bid,
                        time_remaining: demo_time,
                        minerals_refund: m_ref,
                        energy_refund: e_ref,
                    });
                    break;
                }
                if !found {
                    warn!(
                        "DemolishBuilding (planet): no colony found on planet {:?}",
                        planet
                    );
                }
            }
            None => {
                let Ok((sys_buildings, mut sbq)) = sys_buildings_q.get_mut(target_system) else {
                    warn!(
                        "DemolishBuilding (system): target_system {:?} has no SystemBuildings/Queue",
                        target_system
                    );
                    return;
                };
                let Some(Some(bid)) = sys_buildings.slots.get(*target_slot).cloned() else {
                    warn!(
                        "DemolishBuilding (system): slot {} is empty or out of bounds",
                        target_slot
                    );
                    return;
                };
                let def = br.get(bid.as_str());
                let (m_ref, e_ref) =
                    def.map(|d| d.demolition_refund()).unwrap_or((Amt::ZERO, Amt::ZERO));
                let demo_time = def.map(|d| d.demolition_time()).unwrap_or(0);
                sbq.demolition_queue.push(DemolitionOrder {
                    target_slot: *target_slot,
                    building_id: bid,
                    time_remaining: demo_time,
                    minerals_refund: m_ref,
                    energy_refund: e_ref,
                });
            }
        },
        ColonyCommandKind::UpgradeBuilding {
            slot_index,
            target_id,
        } => {
            let upgrade_order = |source_def: &crate::scripting::building_api::BuildingDefinition,
                                 target_id: &str|
             -> Option<UpgradeOrder> {
                let up = source_def
                    .upgrade_to
                    .iter()
                    .find(|u| u.target_id == target_id)?;
                let eff_m = up.cost_minerals.mul_amt(bldg_cost_mod);
                let eff_e = up.cost_energy.mul_amt(bldg_cost_mod);
                let base_time = up
                    .build_time
                    .unwrap_or_else(|| br.get(target_id).map(|d| d.build_time / 2).unwrap_or(5));
                let eff_time = (base_time as f64 * bldg_time_mod.to_f64()).ceil() as i64;
                Some(UpgradeOrder {
                    slot_index: *slot_index,
                    target_id: BuildingId::new(target_id),
                    minerals_remaining: eff_m,
                    energy_remaining: eff_e,
                    build_time_remaining: eff_time,
                })
            };
            match cc.target_planet {
                Some(planet) => {
                    let mut handled = false;
                    for (colony, buildings, mut bq, _) in colonies.iter_mut() {
                        if colony.planet != planet {
                            continue;
                        }
                        handled = true;
                        let Some(Some(source_bid)) = buildings.slots.get(*slot_index).cloned()
                        else {
                            warn!(
                                "UpgradeBuilding (planet): slot {} empty or OOB",
                                slot_index
                            );
                            break;
                        };
                        let Some(source_def) = br.get(source_bid.as_str()) else {
                            warn!(
                                "UpgradeBuilding (planet): unknown source building '{}'",
                                source_bid
                            );
                            break;
                        };
                        if let Some(order) = upgrade_order(source_def, target_id) {
                            bq.upgrade_queue.push(order);
                        } else {
                            warn!(
                                "UpgradeBuilding (planet): no upgrade path '{}' -> '{}'",
                                source_bid, target_id
                            );
                        }
                        break;
                    }
                    if !handled {
                        warn!("UpgradeBuilding (planet): no colony on planet {:?}", planet);
                    }
                }
                None => {
                    let Ok((sys_buildings, mut sbq)) = sys_buildings_q.get_mut(target_system)
                    else {
                        warn!(
                            "UpgradeBuilding (system): target_system {:?} missing components",
                            target_system
                        );
                        return;
                    };
                    let Some(Some(source_bid)) = sys_buildings.slots.get(*slot_index).cloned()
                    else {
                        warn!(
                            "UpgradeBuilding (system): slot {} empty or OOB",
                            slot_index
                        );
                        return;
                    };
                    let Some(source_def) = br.get(source_bid.as_str()) else {
                        warn!(
                            "UpgradeBuilding (system): unknown source building '{}'",
                            source_bid
                        );
                        return;
                    };
                    if let Some(order) = upgrade_order(source_def, target_id) {
                        sbq.upgrade_queue.push(order);
                    } else {
                        warn!(
                            "UpgradeBuilding (system): no upgrade path '{}' -> '{}'",
                            source_bid, target_id
                        );
                    }
                }
            }
        }
        ColonyCommandKind::CancelBuildingOrder { target_slot } => match cc.target_planet {
            Some(planet) => {
                for (colony, _, mut bq, _) in colonies.iter_mut() {
                    if colony.planet == planet {
                        if let Some(pos) = bq.queue.iter().position(|o| o.target_slot == *target_slot) {
                            bq.queue.remove(pos);
                        } else {
                            warn!(
                                "CancelBuildingOrder (planet): no queued order for slot {}",
                                target_slot
                            );
                        }
                        return;
                    }
                }
                warn!(
                    "CancelBuildingOrder (planet): no colony on planet {:?}",
                    planet
                );
            }
            None => {
                if let Ok((_, mut sbq)) = sys_buildings_q.get_mut(target_system) {
                    if let Some(pos) = sbq.queue.iter().position(|o| o.target_slot == *target_slot) {
                        sbq.queue.remove(pos);
                    } else {
                        warn!(
                            "CancelBuildingOrder (system): no queued order for slot {}",
                            target_slot
                        );
                    }
                } else {
                    warn!(
                        "CancelBuildingOrder (system): target_system {:?} missing",
                        target_system
                    );
                }
            }
        },
        ColonyCommandKind::QueueShipBuild {
            host_colony,
            design_id,
            build_kind,
        } => {
            let Ok((_, _, _, mut build_q)) = colonies.get_mut(*host_colony) else {
                warn!(
                    "QueueShipBuild: host_colony {:?} has no BuildQueue",
                    host_colony
                );
                return;
            };
            let Some(design) = sdr.get(design_id) else {
                warn!("QueueShipBuild: unknown design_id '{}'", design_id);
                return;
            };
            // NOTE: Ship cost/time don't yet run through an empire modifier
            // the way buildings do (no ship_cost_modifier), so take the
            // registry values as-is. If such a modifier lands later, apply
            // it here at arrival time.
            let minerals_cost = design.build_cost_minerals;
            let energy_cost = design.build_cost_energy;
            let build_time_total = sdr.build_time(design_id);
            let display_name = design.name.clone();
            build_q.queue.push(BuildOrder {
                kind: build_kind.clone(),
                design_id: design_id.clone(),
                display_name,
                minerals_cost,
                minerals_invested: Amt::ZERO,
                energy_cost,
                energy_invested: Amt::ZERO,
                build_time_total,
                build_time_remaining: build_time_total,
            });
        }
        ColonyCommandKind::CancelShipOrder {
            host_colony,
            queue_index,
        } => {
            let Ok((_, _, _, mut build_q)) = colonies.get_mut(*host_colony) else {
                warn!(
                    "CancelShipOrder: host_colony {:?} has no BuildQueue",
                    host_colony
                );
                return;
            };
            if *queue_index < build_q.queue.len() {
                build_q.queue.remove(*queue_index);
            } else {
                warn!(
                    "CancelShipOrder: queue_index {} out of bounds (len={})",
                    queue_index,
                    build_q.queue.len()
                );
            }
        }
    }
}

/// Helper: push a `BuildingOrder` onto the BuildingQueue of the colony whose
/// `colony.planet == planet`. Warns if no matching colony is found.
fn push_planet_building_order(
    planet: Entity,
    order: crate::colony::BuildingOrder,
    colonies: &mut Query<(
        &crate::colony::Colony,
        &crate::colony::Buildings,
        &mut crate::colony::BuildingQueue,
        &mut crate::colony::BuildQueue,
    )>,
) {
    for (colony, _, mut bq, _) in colonies.iter_mut() {
        if colony.planet == planet {
            bq.queue.push(order);
            return;
        }
    }
    warn!(
        "QueueBuilding (planet): no colony found on planet {:?}",
        planet
    );
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
