pub mod routing;
pub mod fleet;
pub mod exploration;
pub mod hitpoints;
pub mod modifiers;
pub mod settlement;
pub mod survey;
pub mod combat;
pub mod movement;
pub mod command;
pub mod courier_route;
pub mod pursuit;

pub use fleet::*;
pub use exploration::*;
pub use hitpoints::*;
pub use modifiers::*;
pub use settlement::*;
pub use survey::*;
pub use combat::*;
pub use movement::*;
pub use command::*;
pub use courier_route::*;
pub use pursuit::*;

use bevy::prelude::*;

use crate::amount::Amt;
use crate::components::Position;
use crate::modifier::{CachedValue, ScopedModifiers};
use crate::ship_design::ShipDesignRegistry;

// --- #34: Command queue ---

#[derive(Component, Default, Clone)]
pub struct CommandQueue {
    pub commands: Vec<QueuedCommand>,
    /// Predicted position after all queued commands execute
    pub predicted_position: [f64; 3],
    /// Predicted system after all queued commands execute
    pub predicted_system: Option<Entity>,
}

impl CommandQueue {
    /// Push a command and update predicted position
    pub fn push(&mut self, cmd: QueuedCommand, system_positions: &impl Fn(Entity) -> Option<[f64; 3]>) {
        match &cmd {
            QueuedCommand::MoveTo { system } | QueuedCommand::Survey { system } | QueuedCommand::Colonize { system, .. } => {
                if let Some(pos) = system_positions(*system) {
                    self.predicted_position = pos;
                    self.predicted_system = Some(*system);
                }
            }
            QueuedCommand::MoveToCoordinates { target } => {
                // #185: After a deep-space loiter move, the ship is no longer in any system.
                self.predicted_position = *target;
                self.predicted_system = None;
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

#[derive(Clone, Debug)]
pub enum QueuedCommand {
    MoveTo { system: Entity },
    Survey { system: Entity },
    Colonize { system: Entity, planet: Option<Entity> },
    /// #185: Travel sublight to an arbitrary point in deep space and loiter there.
    MoveToCoordinates { target: [f64; 3] },
}

/// Initial FTL speed as a multiple of light speed
pub const INITIAL_FTL_SPEED_C: f64 = 10.0;

pub struct ShipPlugin;

impl Plugin for ShipPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<routing::RouteCalculationsPending>();
        app.add_systems(Update, (
            sync_ship_module_modifiers,
            sync_ship_hitpoints.after(sync_ship_module_modifiers),
            tick_shield_regen,
            sublight_movement_system,
            process_ftl_travel,
            deliver_survey_results.after(process_ftl_travel),
            process_surveys,
            process_settling,
            process_refitting,
            process_pending_ship_commands,
            process_command_queue
                .after(sublight_movement_system)
                .after(process_ftl_travel)
                .after(process_surveys),
            resolve_combat,
            tick_ship_repair,
            // #117: Courier automation — runs before process_command_queue
            // so that any MoveTo it queues this frame is dispatched in the
            // same frame.
            tick_courier_routes
                .before(process_command_queue)
                .after(sublight_movement_system)
                .after(process_ftl_travel),
            // #186 Phase 1: Aggressive ROE detection of hostile deep-space
            // contacts. Runs after movement so ship positions are current.
            pursuit::detect_hostiles_system
                .after(sublight_movement_system)
                .after(process_ftl_travel)
                .after(process_command_queue),
        ).after(crate::time_system::advance_game_time)
         .before(crate::colony::advance_production_tick));
        // #128: Poll route tasks after Commands from process_command_queue are flushed.
        app.add_systems(Update, (
            bevy::ecs::schedule::ApplyDeferred,
            routing::poll_pending_routes,
        ).chain()
         .after(process_command_queue)
         .after(crate::time_system::advance_game_time)
         .before(crate::colony::advance_production_tick));
    }
}

// --- #57: Rules of Engagement ---

/// Controls automatic combat behavior for a ship.
#[derive(Component, Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum RulesOfEngagement {
    /// Always attack hostiles in system
    Aggressive,
    /// Only fight back when attacked (hostile initiates) — same as current behavior
    #[default]
    Defensive,
    /// Do not engage hostiles; skip combat entirely
    Retreat,
}

impl RulesOfEngagement {
    pub fn label(&self) -> &'static str {
        match self {
            Self::Aggressive => "Aggressive",
            Self::Defensive => "Defensive",
            Self::Retreat => "Retreat",
        }
    }

    pub const ALL: [RulesOfEngagement; 3] = [
        RulesOfEngagement::Aggressive,
        RulesOfEngagement::Defensive,
        RulesOfEngagement::Retreat,
    ];
}

// --- #33: Pending ship command system ---

/// A command queued for a remote ship, waiting for light-speed communication delay.
#[derive(Component)]
pub struct PendingShipCommand {
    pub ship: Entity,
    pub command: ShipCommand,
    pub arrives_at: i64,
}

/// The kinds of commands that can be issued to a ship.
#[derive(Clone, Debug)]
pub enum ShipCommand {
    MoveTo { destination: Entity },
    Survey { target: Entity },
    Colonize,
    SetROE { roe: RulesOfEngagement },
    /// Enqueue a command into the ship's CommandQueue (for in-transit ships).
    EnqueueCommand(QueuedCommand),
}

/// A module equipped in a specific slot on a ship.
#[derive(Clone, Debug)]
pub struct EquippedModule {
    pub slot_type: String,
    pub module_id: String,
}

/// Per-ship modifier scopes, driven by equipped modules and tech effects.
#[derive(Component, Default)]
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
}

/// Cached computed stats for a ship, derived from ShipModifiers.
#[derive(Component, Default)]
pub struct ShipStats {
    pub speed: CachedValue,
    pub ftl_range: CachedValue,
    pub survey_speed: CachedValue,
    pub colonize_speed: CachedValue,
    pub evasion: CachedValue,
    pub cargo_capacity: CachedValue,
    pub maintenance: Amt,
}

/// 3-layer hit point model: shield → armor → hull.
/// Shield regenerates over time; armor/hull require docking at a Port.
#[derive(Component, Clone, Debug)]
pub struct ShipHitpoints {
    pub hull: f64,
    pub hull_max: f64,
    pub armor: f64,
    pub armor_max: f64,
    pub shield: f64,
    pub shield_max: f64,
    pub shield_regen: f64, // per hexadies
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
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

#[derive(Component)]
pub struct Ship {
    pub name: String,
    pub design_id: String,
    pub hull_id: String,
    pub modules: Vec<EquippedModule>,
    pub owner: Owner,
    pub sublight_speed: f64,
    pub ftl_range: f64,
    pub player_aboard: bool,
    /// #64: System entity where maintenance is charged
    pub home_port: Entity,
    /// #123: Last `ShipDesignDefinition.revision` this ship was synchronized
    /// with. When the underlying design's revision moves ahead, the ship is
    /// flagged as "needs refit" in the UI and can have the new design
    /// applied via the Apply Refit action.
    pub design_revision: u64,
}

#[derive(Component)]
pub enum ShipState {
    Docked { system: Entity },
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
    /// which currently only operates on Docked ships in star systems.
    Loitering {
        position: [f64; 3],
    },
}

/// Cargo hold for Courier ships (and potentially others).
#[derive(Component, Default, Debug, Clone)]
pub struct Cargo {
    pub minerals: Amt,
    pub energy: Amt,
}

/// #103: Survey data carried by an FTL-capable ship back to the player's system.
/// Stored on the ship when survey completes until the ship docks at the player's
/// StationedAt system, at which point the results are published.
#[derive(Component, Clone, Debug)]
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
    let hull_id = design.map(|d| d.hull_id.as_str()).unwrap_or("corvette").to_string();
    let sublight_speed = design.map(|d| d.sublight_speed).unwrap_or(0.75);
    let ftl_range = design.map(|d| d.ftl_range).unwrap_or(10.0);
    // #123: Newly built ships are spawned in sync with the current design revision.
    let design_revision = design.map(|d| d.revision).unwrap_or(0);
    // Equip ships from the design's slot assignments so they start out matching
    // the design exactly (no spurious "needs refit" right after construction).
    let modules = design.map(crate::ship_design::design_equipped_modules).unwrap_or_default();
    commands
        .spawn((
            Ship {
                name,
                design_id: design_id.to_string(),
                hull_id,
                modules,
                owner,
                sublight_speed,
                ftl_range,
                player_aboard: false,
                home_port: system,
                design_revision,
            },
            ShipState::Docked { system },
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
        ))
        .id()
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::ecs::world::World;
    use crate::ship_design::{ShipDesignDefinition, ShipDesignRegistry};

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
        });
        registry.insert(ShipDesignDefinition {
            id: "courier_mk1".to_string(),
            name: "Courier Mk.I".to_string(),
            description: String::new(),
            hull_id: "courier_hull".to_string(),
            modules: Vec::new(),
            can_survey: false,
            can_colonize: false,
            maintenance: Amt::new(0, 300),
            build_cost_minerals: Amt::units(100),
            build_cost_energy: Amt::units(50),
            build_time: 30,
            hp: 35.0,
            sublight_speed: 0.80,
            ftl_range: 0.0,
            revision: 0,
        });
        registry.insert(ShipDesignDefinition {
            id: "scout_mk1".to_string(),
            name: "Scout Mk.I".to_string(),
            description: String::new(),
            hull_id: "scout_hull".to_string(),
            modules: Vec::new(),
            can_survey: true,
            can_colonize: false,
            maintenance: Amt::new(0, 400),
            build_cost_minerals: Amt::units(150),
            build_cost_energy: Amt::units(80),
            build_time: 45,
            hp: 40.0,
            sublight_speed: 0.85,
            ftl_range: 10.0,
            revision: 0,
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
            player_aboard: false,
            home_port: Entity::PLACEHOLDER,
            design_revision: 0,
        }
    }

    #[test]
    fn start_sublight_sets_correct_arrival_time() {
        let mut world = World::new();
        let system = world.spawn_empty().id();
        let ship = make_ship("colony_ship_mk1"); // 0.5c
        let origin = Position { x: 0.0, y: 0.0, z: 0.0 };
        let dest = Position { x: 1.0, y: 0.0, z: 0.0 }; // 1 LY away
        let mut state = ShipState::Docked { system };
        start_sublight_travel(&mut state, &origin, &ship, dest, Some(system), 100);
        match state {
            ShipState::SubLight { arrival_at, departed_at, .. } => {
                assert_eq!(departed_at, 100);
                assert_eq!(arrival_at, 220);
            }
            _ => panic!("Expected SubLight state"),
        }
    }

    #[test]
    fn start_ftl_rejects_no_ftl_ship() {
        let mut world = World::new();
        let origin = world.spawn_empty().id();
        let dest = world.spawn_empty().id();
        let ship = make_ship("courier_mk1");
        let mut state = ShipState::Docked { system: origin };
        let origin_pos = Position { x: 0.0, y: 0.0, z: 0.0 };
        let dest_pos = Position { x: 1.0, y: 0.0, z: 0.0 };
        let result = start_ftl_travel(&mut state, &ship, origin, dest, &origin_pos, &dest_pos, 0);
        assert_eq!(result, Err("Ship has no FTL capability"));
    }

    #[test]
    fn start_ftl_rejects_out_of_range() {
        let mut world = World::new();
        let origin = world.spawn_empty().id();
        let dest = world.spawn_empty().id();
        let ship = make_ship("colony_ship_mk1");
        let mut state = ShipState::Docked { system: origin };
        let origin_pos = Position { x: 0.0, y: 0.0, z: 0.0 };
        let dest_pos = Position { x: 50.0, y: 0.0, z: 0.0 };
        let result = start_ftl_travel(&mut state, &ship, origin, dest, &origin_pos, &dest_pos, 0);
        assert_eq!(result, Err("Destination is beyond FTL range"));
    }

    #[test]
    fn start_ftl_correct_travel_time() {
        let mut world = World::new();
        let origin = world.spawn_empty().id();
        let dest = world.spawn_empty().id();
        let ship = make_ship("colony_ship_mk1");
        let mut state = ShipState::Docked { system: origin };
        let origin_pos = Position { x: 0.0, y: 0.0, z: 0.0 };
        let dest_pos = Position { x: 10.0, y: 0.0, z: 0.0 };
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
        let origin_pos = Position { x: 0.0, y: 0.0, z: 0.0 };
        let dest_pos = Position { x: 10.0, y: 0.0, z: 0.0 };

        // Without port
        let mut state_no_port = ShipState::Docked { system: origin };
        let _ = start_ftl_travel_with_bonus(&mut state_no_port, &ship, origin, dest, &origin_pos, &dest_pos, 0, 0.0, 1.0, PortParams::NONE);
        let time_no_port = match state_no_port {
            ShipState::InFTL { arrival_at, .. } => arrival_at,
            _ => panic!("Expected InFTL state"),
        };

        // With port (using Lua-defined values)
        let mut state_port = ShipState::Docked { system: origin };
        let port_params = PortParams { has_port: true, ftl_range_bonus: 10.0, travel_time_factor: 0.8 };
        let _ = start_ftl_travel_with_bonus(&mut state_port, &ship, origin, dest, &origin_pos, &dest_pos, 0, 0.0, 1.0, port_params);
        let time_port = match state_port {
            ShipState::InFTL { arrival_at, .. } => arrival_at,
            _ => panic!("Expected InFTL state"),
        };

        // Port should reduce travel time by 20%
        assert!(time_port < time_no_port, "Port should reduce FTL travel time");
        let expected = (time_no_port as f64 * 0.8).ceil() as i64;
        assert_eq!(time_port, expected);
    }

    #[test]
    fn start_ftl_with_port_extends_range() {
        let mut world = World::new();
        let origin = world.spawn_empty().id();
        let dest = world.spawn_empty().id();
        let ship = make_ship("colony_ship_mk1"); // ftl_range = 15.0

        let origin_pos = Position { x: 0.0, y: 0.0, z: 0.0 };
        let dest_pos = Position { x: 20.0, y: 0.0, z: 0.0 }; // 20 ly, beyond base 15 ly range

        // Without port: should fail
        let mut state = ShipState::Docked { system: origin };
        let result = start_ftl_travel_with_bonus(&mut state, &ship, origin, dest, &origin_pos, &dest_pos, 0, 0.0, 1.0, PortParams::NONE);
        assert_eq!(result, Err("Destination is beyond FTL range"));

        // With port: +10 ly range, so 25 ly total, should succeed
        let mut state = ShipState::Docked { system: origin };
        let port_params = PortParams { has_port: true, ftl_range_bonus: 10.0, travel_time_factor: 0.8 };
        let result = start_ftl_travel_with_bonus(&mut state, &ship, origin, dest, &origin_pos, &dest_pos, 0, 0.0, 1.0, port_params);
        assert!(result.is_ok(), "Port should extend FTL range by 10 ly");
    }

    // --- #51: Ship maintenance cost tests ---

    #[test]
    fn ship_maintenance_costs() {
        let registry = test_design_registry();
        assert_eq!(registry.maintenance("explorer_mk1"), Amt::new(0, 500));
        assert_eq!(registry.maintenance("colony_ship_mk1"), Amt::units(1));
        assert_eq!(registry.maintenance("courier_mk1"), Amt::new(0, 300));
    }

    #[test]
    fn build_cost_returns_expected_values() {
        let registry = test_design_registry();
        assert_eq!(registry.build_cost("explorer_mk1"), (Amt::units(200), Amt::units(100)));
        assert_eq!(registry.build_cost("colony_ship_mk1"), (Amt::units(500), Amt::units(300)));
        assert_eq!(registry.build_cost("courier_mk1"), (Amt::units(100), Amt::units(50)));
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
            prerequisite_tech: None,
            upgrade_to: Vec::new(),
        });
        let modules = vec![
            EquippedModule { slot_type: "weapon".into(), module_id: "test_weapon".into() },
        ];
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
            commands: vec![QueuedCommand::Survey {
                system: system_b,
            }],
            ..Default::default()
        };
        let state = ShipState::Docked { system: system_a };

        // Simulate what process_command_queue does:
        // It checks if docked_system != target, and if so, inserts move + re-queues survey
        let docked_system = match &state {
            ShipState::Docked { system } => *system,
            _ => panic!("Expected Docked"),
        };
        let next = queue.commands.remove(0);
        match next {
            QueuedCommand::Survey { system: target } => {
                assert_ne!(docked_system, target);
                // Auto-insert: move to target, then re-queue survey
                queue.commands.insert(0, QueuedCommand::Survey { system: target });
                queue.commands.insert(0, QueuedCommand::MoveTo { system: target });
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
        let state = ShipState::Docked { system: system_a };

        let docked_system = match &state {
            ShipState::Docked { system } => *system,
            _ => panic!("Expected Docked"),
        };
        let next = queue.commands.remove(0);
        match next {
            QueuedCommand::Colonize { system: target, planet } => {
                assert_ne!(docked_system, target);
                // Auto-insert: move to target, then re-queue colonize
                queue.commands.insert(0, QueuedCommand::Colonize { system: target, planet });
                queue.commands.insert(0, QueuedCommand::MoveTo { system: target });
            }
            _ => panic!("Expected Colonize command"),
        }

        // Should be [MoveTo, Colonize] — route planning (FTL vs sublight) is handled by process_command_queue
        assert_eq!(queue.commands.len(), 2);
        assert!(matches!(queue.commands[0], QueuedCommand::MoveTo { .. }));
        assert!(matches!(queue.commands[1], QueuedCommand::Colonize { .. }));
    }
}
