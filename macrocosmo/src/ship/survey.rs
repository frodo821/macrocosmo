use bevy::prelude::*;

use crate::events::{GameEvent, GameEventKind};
use crate::galaxy::{Anomalies, AtSystem, Hostile, StarSystem, SystemAttributes};
use crate::knowledge::{
    FactSysParam, FactionVantageQueries, KnowledgeFact, KnowledgeStore, ObservationSource,
    SystemKnowledge, SystemSnapshot,
};
use crate::physics::{distance_ly_arr, light_delay_hexadies};
use crate::player::{Empire, EmpireRuler, Player, PlayerEmpire, Ruler, StationedAt};
use crate::ship_design::ShipDesignRegistry;
use crate::time_system::{GameClock, HEXADIES_PER_YEAR};

use super::exploration::roll_and_apply_anomaly;
use super::{CommandQueue, QueuedCommand, Ship, ShipHitpoints, ShipState, SurveyData};

/// Default duration of a survey operation in hexadies (30 hexadies = half a year) (#32).
///
/// #160: Canonical value lives in `GameBalance.survey_duration` (Lua-defined).
/// This constant is retained as the fallback used by helper-function callers
/// that don't have access to the `GameBalance` resource (notably tests).
pub const SURVEY_DURATION_HEXADIES: i64 = 30;

/// Default maximum survey range (LY). Canonical value: `GameBalance.survey_range_ly`.
pub const SURVEY_RANGE_LY: f64 = 5.0;

/// Attempt to start a survey operation on a target star system.
/// #45: Accepts optional survey_range_bonus from GlobalParams
pub fn start_survey(
    ship_state: &mut ShipState,
    ship: &Ship,
    target_system: Entity,
    ship_pos: &crate::components::Position,
    system_pos: &crate::components::Position,
    current_time: i64,
    design_registry: &ShipDesignRegistry,
) -> Result<(), &'static str> {
    start_survey_with_bonus(
        ship_state,
        ship,
        target_system,
        ship_pos,
        system_pos,
        current_time,
        0.0,
        design_registry,
        SURVEY_RANGE_LY,
        SURVEY_DURATION_HEXADIES,
    )
}

/// #160: `base_range` / `base_duration` are read from `GameBalance` by callers;
/// fallback constants are `SURVEY_RANGE_LY` / `SURVEY_DURATION_HEXADIES`.
#[allow(clippy::too_many_arguments)]
pub fn start_survey_with_bonus(
    ship_state: &mut ShipState,
    ship: &Ship,
    target_system: Entity,
    ship_pos: &crate::components::Position,
    system_pos: &crate::components::Position,
    current_time: i64,
    survey_range_bonus: f64,
    design_registry: &ShipDesignRegistry,
    base_range: f64,
    base_duration: i64,
) -> Result<(), &'static str> {
    if !design_registry.can_survey(&ship.design_id) {
        return Err("Only Explorer ships can perform surveys");
    }

    let docked_system = match ship_state {
        ShipState::InSystem { system } => *system,
        _ => return Err("Ship must be docked to begin a survey"),
    };

    // #102: Ship must be docked at the target system to survey it
    if docked_system != target_system {
        return Err("Ship must be docked at the target system to survey it");
    }

    let effective_range = base_range + survey_range_bonus;
    let distance = ship_pos.distance_to(system_pos);
    if distance > effective_range {
        return Err("Target system is beyond survey range");
    }

    *ship_state = ShipState::Surveying {
        target_system,
        started_at: current_time,
        completes_at: current_time + base_duration,
    };

    Ok(())
}

/// System that processes ongoing surveys and marks star systems as surveyed
/// when the survey duration has elapsed. Rolls an exploration event on completion.
///
/// #103: FTL-capable ships store survey data internally instead of publishing
/// immediately. They auto-queue an FTL return to the player's StationedAt system
/// if no commands are pending. Non-FTL ships publish results immediately via
/// light-speed propagation (existing behavior).
#[allow(clippy::too_many_arguments)]
pub fn process_surveys(
    mut commands: Commands,
    clock: Res<GameClock>,
    mut ships: Query<(
        Entity,
        &Ship,
        &mut ShipState,
        &mut ShipHitpoints,
        &crate::components::Position,
        Option<&mut CommandQueue>,
    )>,
    mut systems: Query<
        (
            &mut StarSystem,
            Option<&mut SystemAttributes>,
            &crate::components::Position,
            Option<&mut Anomalies>,
        ),
        Without<Ship>,
    >,
    hostiles: Query<&AtSystem, With<Hostile>>,
    player_q: Query<&StationedAt, With<Player>>,
    empire_params_q: Query<&crate::technology::GlobalParams, With<PlayerEmpire>>,
    balance: Res<crate::technology::GameBalance>,
    anomaly_registry: Option<Res<crate::scripting::anomaly_api::AnomalyRegistry>>,
    mut events: MessageWriter<GameEvent>,
    mut fact_sys: FactSysParam,
    // Round 9 PR #1 Step 3: per-faction fact routing.
    vantage_q: FactionVantageQueries,
    // Round 9: per-ship-owner auto-return target lookup. NPC ships
    // (#428) need to return to their own empire's home system, not the
    // player's. Resolved via Empire → EmpireRuler → Ruler.StationedAt.
    empire_rulers: Query<&EmpireRuler, With<Empire>>,
    rulers_stationed: Query<&StationedAt, With<Ruler>>,
) {
    let initial_ftl_speed_c = balance.initial_ftl_speed_c();
    let mut rng = rand::rng();

    // Collect player's stationed-at system for auto-return
    let player_system = player_q.iter().next().map(|s| s.system);

    // #110: Pre-compute player system position and FTL speed for light-vs-FTL comparison
    let player_system_pos: Option<[f64; 3]> =
        player_system.and_then(|sys| systems.get(sys).ok().map(|(_, _, pos, _)| pos.as_array()));
    let ftl_speed_multiplier = empire_params_q
        .iter()
        .next()
        .map(|p| p.ftl_speed_multiplier)
        .unwrap_or(1.0);

    // Round 9 PR #1 Step 3: collect every empire's vantage so the
    // dual-write below routes a survey discovery into ALL empires'
    // PendingFactQueues, not just the player's.
    let vantages = vantage_q.collect();

    for (ship_entity, ship, mut state, mut ship_hp, ship_pos, mut cmd_queue) in ships.iter_mut() {
        let (target_system, completes_at) = match *state {
            ShipState::Surveying {
                target_system,
                completes_at,
                ..
            } => (target_system, completes_at),
            _ => continue,
        };

        if clock.elapsed >= completes_at {
            let has_ftl = ship.ftl_range > 0.0;

            if has_ftl {
                // #110: Compare light-speed propagation vs FTL return time
                let use_light_speed = player_system_pos
                    .map(|player_pos| {
                        let distance = distance_ly_arr(ship_pos.as_array(), player_pos);
                        let light_delay = light_delay_hexadies(distance);
                        let effective_ftl_speed = initial_ftl_speed_c * ftl_speed_multiplier;
                        let ftl_return_time = (distance * HEXADIES_PER_YEAR as f64
                            / effective_ftl_speed)
                            .ceil() as i64;
                        light_delay <= ftl_return_time
                    })
                    .unwrap_or(false);

                if use_light_speed {
                    // #110: Light-speed is faster — mark surveyed immediately
                    let sys_pos_arr: Option<[f64; 3]> = systems
                        .get(target_system)
                        .ok()
                        .map(|(_, _, p, _)| p.as_array());
                    if let Ok((mut star_system, attrs, _sys_pos, anomalies)) =
                        systems.get_mut(target_system)
                    {
                        star_system.surveyed = true;
                        let system_name = star_system.name.clone();
                        info!(
                            "Survey complete (FTL ship, light-speed propagation): {} surveyed {}",
                            ship.name, system_name
                        );

                        // #249: Dual-write GameEvent + KnowledgeFact with shared EventId
                        let event_id = fact_sys.allocate_event_id();
                        let desc = format!("{} completed survey of {}", ship.name, system_name);
                        events.write(GameEvent {
                            id: event_id,
                            timestamp: clock.elapsed,
                            kind: GameEventKind::SurveyComplete,
                            description: desc.clone(),
                            related_system: Some(target_system),
                        });
                        if let Some(origin_pos) = sys_pos_arr {
                            let fact = KnowledgeFact::SurveyComplete {
                                event_id: Some(event_id),
                                system: target_system,
                                system_name: system_name.clone(),
                                detail: desc,
                            };
                            fact_sys.record_for(fact, &vantages, origin_pos, clock.elapsed);
                        }

                        let has_hostile = hostiles.iter().any(|at| at.0 == target_system);
                        if has_hostile {
                            let event_id = fact_sys.allocate_event_id();
                            let desc =
                                format!("Warning: Hostile presence detected at {}!", system_name);
                            events.write(GameEvent {
                                id: event_id,
                                timestamp: clock.elapsed,
                                kind: GameEventKind::HostileDetected,
                                description: desc.clone(),
                                related_system: Some(target_system),
                            });
                            if let Some(origin_pos) = sys_pos_arr {
                                let fact = KnowledgeFact::HostileDetected {
                                    event_id: Some(event_id),
                                    target: Entity::PLACEHOLDER,
                                    detector: ship_entity,
                                    target_pos: origin_pos,
                                    description: desc,
                                };
                                fact_sys.record_for(fact, &vantages, origin_pos, clock.elapsed);
                            }
                        }

                        // #127: Roll anomaly discovery (with fallback to legacy exploration events)
                        let anomaly_id = roll_and_apply_anomaly(
                            &anomaly_registry,
                            &mut rng,
                            &system_name,
                            &ship,
                            &mut ship_hp,
                            attrs,
                            anomalies,
                            clock.elapsed,
                            target_system,
                            &mut events,
                        );
                        // #249 / E: For light-speed propagation we also dual-write
                        // the AnomalyDiscovered banner so the player gets a
                        // single notification once the news arrives. The
                        // per-effect events the helper wrote stay EventLog-only
                        // (they're descriptive sub-detail, not banner-worthy on
                        // their own).
                        if let Some(aid) = anomaly_id {
                            let event_id = fact_sys.allocate_event_id();
                            let desc = format!("Anomaly '{}' discovered at {}", aid, system_name);
                            events.write(GameEvent {
                                id: event_id,
                                timestamp: clock.elapsed,
                                kind: GameEventKind::AnomalyDiscovered,
                                description: desc.clone(),
                                related_system: Some(target_system),
                            });
                            if let Some(origin_pos) = sys_pos_arr {
                                let fact = KnowledgeFact::AnomalyDiscovered {
                                    event_id: Some(event_id),
                                    system: target_system,
                                    anomaly_id: aid,
                                    detail: desc,
                                };
                                fact_sys.record_for(fact, &vantages, origin_pos, clock.elapsed);
                            }
                        }
                    }
                } else {
                    let sys_pos_arr: Option<[f64; 3]> = systems
                        .get(target_system)
                        .ok()
                        .map(|(_, _, p, _)| p.as_array());
                    if let Ok((star_system, attrs, _sys_pos, anomalies)) =
                        systems.get_mut(target_system)
                    {
                        // #103: FTL return is faster — carry back
                        let system_name = star_system.name.clone();
                        info!(
                            "Survey complete (FTL ship): {} surveyed {} — data stored on ship",
                            ship.name, system_name
                        );

                        let has_hostile = hostiles.iter().any(|at| at.0 == target_system);
                        if has_hostile {
                            // #249: Dual-write — hostile visible via light-speed/relay even
                            // though the ship is FTL-returning the survey data itself.
                            let event_id = fact_sys.allocate_event_id();
                            let desc =
                                format!("Warning: Hostile presence detected at {}!", system_name);
                            events.write(GameEvent {
                                id: event_id,
                                timestamp: clock.elapsed,
                                kind: GameEventKind::HostileDetected,
                                description: desc.clone(),
                                related_system: Some(target_system),
                            });
                            if let Some(origin_pos) = sys_pos_arr {
                                let fact = KnowledgeFact::HostileDetected {
                                    event_id: Some(event_id),
                                    target: Entity::PLACEHOLDER,
                                    detector: ship_entity,
                                    target_pos: origin_pos,
                                    description: desc,
                                };
                                fact_sys.record_for(fact, &vantages, origin_pos, clock.elapsed);
                            }
                        }

                        // #127: Roll anomaly discovery; effects applied immediately, event deferred
                        let anomaly_id = roll_and_apply_anomaly(
                            &anomaly_registry,
                            &mut rng,
                            &system_name,
                            &ship,
                            &mut ship_hp,
                            attrs,
                            anomalies,
                            clock.elapsed,
                            target_system,
                            &mut events,
                        );

                        // Use try_insert: ship may have been despawned by combat in the same frame
                        commands.entity(ship_entity).try_insert(SurveyData {
                            target_system,
                            surveyed_at: clock.elapsed,
                            system_name: system_name.clone(),
                            anomaly_id,
                        });

                        let queue_empty = cmd_queue
                            .as_ref()
                            .map(|q| q.commands.is_empty())
                            .unwrap_or(true);
                        if queue_empty {
                            // Round 9: per-ship-owner auto-return. NPC ships
                            // (#428) return to their own empire's Ruler
                            // stationed system, not the player's. Player
                            // ships still resolve via the same chain
                            // (player Empire → EmpireRuler → Ruler).
                            let owner_home = match ship.owner {
                                crate::ship::Owner::Empire(e) => empire_rulers
                                    .get(e)
                                    .ok()
                                    .and_then(|er| rulers_stationed.get(er.0).ok())
                                    .map(|s| s.system),
                                crate::ship::Owner::Neutral => None,
                            };
                            // Legacy fallback: tests / old saves that attach
                            // Player+StationedAt directly without the
                            // Empire→EmpireRuler→Ruler chain still get player
                            // auto-return. Production NPC ships resolve via
                            // owner_home (the chain is always populated for
                            // spawned empires).
                            let return_target = owner_home.or(player_system);
                            if let Some(home) = return_target
                                && home != target_system
                                && let Some(ref mut queue) = cmd_queue
                            {
                                queue
                                    .commands
                                    .push(QueuedCommand::MoveTo { system: home });
                                info!(
                                    "Auto-queued FTL return to home system for {}",
                                    ship.name
                                );
                            }
                        }
                    }
                }
            } else {
                // Non-FTL ship — existing behavior: mark surveyed immediately
                let sys_pos_arr: Option<[f64; 3]> = systems
                    .get(target_system)
                    .ok()
                    .map(|(_, _, p, _)| p.as_array());
                if let Ok((mut star_system, attrs, _sys_pos, anomalies)) =
                    systems.get_mut(target_system)
                {
                    star_system.surveyed = true;
                    let system_name = star_system.name.clone();
                    info!("Survey complete: {} has been surveyed", system_name);

                    // #249: Dual-write SurveyComplete
                    let event_id = fact_sys.allocate_event_id();
                    let desc = format!("{} completed survey of {}", ship.name, system_name);
                    events.write(GameEvent {
                        id: event_id,
                        timestamp: clock.elapsed,
                        kind: GameEventKind::SurveyComplete,
                        description: desc.clone(),
                        related_system: Some(target_system),
                    });
                    if let Some(origin_pos) = sys_pos_arr {
                        let fact = KnowledgeFact::SurveyComplete {
                            event_id: Some(event_id),
                            system: target_system,
                            system_name: system_name.clone(),
                            detail: desc,
                        };
                        fact_sys.record_for(fact, &vantages, origin_pos, clock.elapsed);
                    }

                    // Check for hostile presence at this system
                    let has_hostile = hostiles.iter().any(|at| at.0 == target_system);
                    if has_hostile {
                        let event_id = fact_sys.allocate_event_id();
                        let desc =
                            format!("Warning: Hostile presence detected at {}!", system_name);
                        events.write(GameEvent {
                            id: event_id,
                            timestamp: clock.elapsed,
                            kind: GameEventKind::HostileDetected,
                            description: desc.clone(),
                            related_system: Some(target_system),
                        });
                        if let Some(origin_pos) = sys_pos_arr {
                            let fact = KnowledgeFact::HostileDetected {
                                event_id: Some(event_id),
                                target: Entity::PLACEHOLDER,
                                detector: ship_entity,
                                target_pos: origin_pos,
                                description: desc,
                            };
                            fact_sys.record_for(fact, &vantages, origin_pos, clock.elapsed);
                        }
                    }

                    // #127: Roll anomaly discovery (with fallback to legacy exploration events)
                    roll_and_apply_anomaly(
                        &anomaly_registry,
                        &mut rng,
                        &system_name,
                        &ship,
                        &mut ship_hp,
                        attrs,
                        anomalies,
                        clock.elapsed,
                        target_system,
                        &mut events,
                    );
                }
            }

            *state = ShipState::InSystem {
                system: target_system,
            };
        }
    }
}

/// #103: Deliver survey results when an FTL ship carrying survey data docks
/// at the player's StationedAt system.
#[allow(clippy::too_many_arguments)]
pub fn deliver_survey_results(
    mut commands: Commands,
    clock: Res<GameClock>,
    ships: Query<(Entity, &Ship, &ShipState, &SurveyData)>,
    mut systems: Query<(&mut StarSystem, &crate::components::Position), Without<Ship>>,
    player_q: Query<&StationedAt, With<Player>>,
    mut empire_q: Query<&mut KnowledgeStore, With<PlayerEmpire>>,
    mut events: MessageWriter<GameEvent>,
    mut fact_sys: FactSysParam,
    // Round 9 PR #1 Step 3: per-faction routing.
    vantage_q: FactionVantageQueries,
) {
    let player_system = match player_q.iter().next() {
        Some(s) => s.system,
        None => return,
    };

    // #249: Player vantage — delivered at player's docked system, so origin
    // matches player_pos → local path in `record_fact_or_local`.
    // Round 9 PR #1 Step 3: also collect every other empire's vantage so
    // the FTL-delivery banner does not skip NPC observers entirely. NPC
    // empires already see the surveyed flag via the light-speed path in
    // `process_surveys`, but routing through `record_for` keeps every
    // empire's `PendingFactQueue` aware of the formal `SurveyComplete`
    // fact (with its own arrival time per vantage).
    let player_pos: Option<[f64; 3]> = systems.get(player_system).ok().map(|(_, p)| p.as_array());
    let vantages = vantage_q.collect();

    for (ship_entity, ship, state, survey_data) in &ships {
        let ShipState::InSystem { system: docked_at } = state else {
            continue;
        };

        if *docked_at != player_system {
            continue;
        }

        // Ship is docked at the player's system — deliver results
        let target = survey_data.target_system;

        // Mark the target system as surveyed and update KnowledgeStore
        if let Ok((mut star_system, pos)) = systems.get_mut(target) {
            star_system.surveyed = true;
            info!(
                "Survey data delivered: {} marked as surveyed (delivered by {})",
                survey_data.system_name, ship.name
            );

            // Update KnowledgeStore
            if let Ok(mut store) = empire_q.single_mut() {
                store.update(SystemKnowledge {
                    system: target,
                    observed_at: survey_data.surveyed_at,
                    received_at: clock.elapsed,
                    data: SystemSnapshot {
                        name: star_system.name.clone(),
                        position: pos.as_array(),
                        surveyed: true,
                        ..default()
                    },
                    source: ObservationSource::Direct,
                });
            }
        }

        // #249: Dual-write SurveyComplete for delivered data.
        let event_id = fact_sys.allocate_event_id();
        let desc = format!(
            "{} delivered survey data for {} (surveyed at t={})",
            ship.name, survey_data.system_name, survey_data.surveyed_at
        );
        events.write(GameEvent {
            id: event_id,
            timestamp: clock.elapsed,
            kind: GameEventKind::SurveyComplete,
            description: desc.clone(),
            related_system: Some(target),
        });
        if let Some(pp) = player_pos {
            let fact = KnowledgeFact::SurveyComplete {
                event_id: Some(event_id),
                system: target,
                system_name: survey_data.system_name.clone(),
                detail: desc,
            };
            fact_sys.record_for(fact, &vantages, pp, clock.elapsed);
        }

        // #127: If anomaly was discovered, fire AnomalyDiscovered event on delivery
        if let Some(ref anomaly_id) = survey_data.anomaly_id {
            let event_id = fact_sys.allocate_event_id();
            let desc = format!(
                "{} reports anomaly '{}' discovered at {} (surveyed at t={})",
                ship.name, anomaly_id, survey_data.system_name, survey_data.surveyed_at
            );
            events.write(GameEvent {
                id: event_id,
                timestamp: clock.elapsed,
                kind: GameEventKind::AnomalyDiscovered,
                description: desc.clone(),
                related_system: Some(target),
            });
            if let Some(pp) = player_pos {
                let fact = KnowledgeFact::AnomalyDiscovered {
                    event_id: Some(event_id),
                    system: target,
                    anomaly_id: anomaly_id.clone(),
                    detail: desc,
                };
                fact_sys.record_for(fact, &vantages, pp, clock.elapsed);
            }
        }

        // Clear survey data from the ship
        commands.entity(ship_entity).remove::<SurveyData>();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::amount::Amt;
    use crate::components::Position;
    use crate::ship::Owner;
    use crate::ship_design::{ShipDesignDefinition, ShipDesignRegistry};
    use bevy::ecs::world::World;

    fn test_design_registry() -> ShipDesignRegistry {
        let mut registry = ShipDesignRegistry::default();
        registry.insert(ShipDesignDefinition {
            id: "explorer_mk1".to_string(),
            name: "Explorer Mk.I".to_string(),
            description: String::new(),
            hull_id: "corvette".to_string(),
            modules: Vec::new(),
            can_survey: true,
            can_colonize: false,
            maintenance: Amt::new(0, 500),
            build_cost_minerals: Amt::units(200),
            build_cost_energy: Amt::units(100),
            build_time: 60,
            hp: 50.0,
            sublight_speed: 0.75,
            ftl_range: 10.0,
            revision: 0,
            is_direct_buildable: true,
        });
        registry.insert(ShipDesignDefinition {
            id: "colony_ship_mk1".to_string(),
            name: "Colony Ship Mk.I".to_string(),
            description: String::new(),
            hull_id: "frigate".to_string(),
            modules: Vec::new(),
            can_survey: false,
            can_colonize: true,
            maintenance: Amt::units(1),
            build_cost_minerals: Amt::units(500),
            build_cost_energy: Amt::units(300),
            build_time: 120,
            hp: 100.0,
            sublight_speed: 0.5,
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

    #[test]
    fn start_survey_rejects_non_explorer() {
        let mut world = World::new();
        let system = world.spawn_empty().id();
        let ship = make_ship("colony_ship_mk1");
        let mut state = ShipState::InSystem { system };
        let pos = Position {
            x: 0.0,
            y: 0.0,
            z: 0.0,
        };
        let registry = test_design_registry();
        let result = start_survey(&mut state, &ship, system, &pos, &pos, 0, &registry);
        assert_eq!(result, Err("Only Explorer ships can perform surveys"));
    }

    #[test]
    fn start_survey_rejects_non_docked() {
        let mut world = World::new();
        let system = world.spawn_empty().id();
        let ship = make_ship("explorer_mk1");
        let mut state = ShipState::SubLight {
            origin: [0.0; 3],
            destination: [1.0, 0.0, 0.0],
            target_system: Some(system),
            departed_at: 0,
            arrival_at: 100,
        };
        let pos = Position {
            x: 0.0,
            y: 0.0,
            z: 0.0,
        };
        let registry = test_design_registry();
        let result = start_survey(&mut state, &ship, system, &pos, &pos, 0, &registry);
        assert_eq!(result, Err("Ship must be docked to begin a survey"));
    }

    #[test]
    fn start_survey_rejects_out_of_range() {
        let mut world = World::new();
        let system = world.spawn_empty().id();
        let ship = make_ship("explorer_mk1");
        let mut state = ShipState::InSystem { system };
        let ship_pos = Position {
            x: 0.0,
            y: 0.0,
            z: 0.0,
        };
        let target_pos = Position {
            x: 10.0,
            y: 0.0,
            z: 0.0,
        };
        let registry = test_design_registry();
        let result = start_survey(
            &mut state,
            &ship,
            system,
            &ship_pos,
            &target_pos,
            0,
            &registry,
        );
        assert_eq!(result, Err("Target system is beyond survey range"));
    }

    #[test]
    fn start_survey_sets_correct_completion_time() {
        let mut world = World::new();
        let system = world.spawn_empty().id();
        let ship = make_ship("explorer_mk1");
        let mut state = ShipState::InSystem { system };
        let pos = Position {
            x: 0.0,
            y: 0.0,
            z: 0.0,
        };
        let registry = test_design_registry();
        let result = start_survey(&mut state, &ship, system, &pos, &pos, 50, &registry);
        assert!(result.is_ok());
        match state {
            ShipState::Surveying {
                completes_at,
                started_at,
                ..
            } => {
                assert_eq!(started_at, 50);
                assert_eq!(completes_at, 80); // 50 + SURVEY_DURATION_HEXADIES (30)
            }
            _ => panic!("Expected Surveying state"),
        }
    }

    // --- #102: Survey requires docked at target system ---

    #[test]
    fn start_survey_rejects_wrong_system() {
        let mut world = World::new();
        let system_a = world.spawn_empty().id();
        let system_b = world.spawn_empty().id();
        let ship = make_ship("explorer_mk1");
        let mut state = ShipState::InSystem { system: system_a };
        let pos = Position {
            x: 0.0,
            y: 0.0,
            z: 0.0,
        };
        let registry = test_design_registry();
        let result = start_survey(&mut state, &ship, system_b, &pos, &pos, 0, &registry);
        assert_eq!(
            result,
            Err("Ship must be docked at the target system to survey it")
        );
    }

    #[test]
    fn start_survey_same_system_succeeds() {
        let mut world = World::new();
        let system = world.spawn_empty().id();
        let ship = make_ship("explorer_mk1");
        let mut state = ShipState::InSystem { system };
        let pos = Position {
            x: 0.0,
            y: 0.0,
            z: 0.0,
        };
        let registry = test_design_registry();
        let result = start_survey(&mut state, &ship, system, &pos, &pos, 0, &registry);
        assert!(result.is_ok());
        assert!(matches!(state, ShipState::Surveying { .. }));
    }
}
