use bevy::prelude::*;

use super::{Ship, ShipState, CommandQueue, Cargo};

/// A fleet composed of multiple ships.
#[derive(Component)]
pub struct Fleet {
    pub name: String,
    pub members: Vec<Entity>,
    pub flagship: Entity,
}

impl Fleet {
    /// Fleet movement speed = slowest member
    pub fn speed(&self, ships: &Query<&Ship>) -> f64 {
        self.members
            .iter()
            .filter_map(|e| ships.get(*e).ok())
            .map(|s| s.sublight_speed)
            .fold(f64::MAX, f64::min)
    }

    /// Fleet FTL range = shortest range member
    pub fn ftl_range(&self, ships: &Query<&Ship>) -> f64 {
        self.members
            .iter()
            .filter_map(|e| ships.get(*e).ok())
            .map(|s| s.ftl_range)
            .fold(f64::MAX, f64::min)
    }
}

/// Marks a ship as belonging to a fleet.
#[derive(Component)]
pub struct FleetMembership {
    pub fleet: Entity,
}

/// Create a fleet from the given ships, returning the fleet entity.
pub fn create_fleet(
    commands: &mut Commands,
    name: String,
    members: Vec<Entity>,
    flagship: Entity,
) -> Entity {
    let fleet_entity = commands
        .spawn(Fleet {
            name,
            members: members.clone(),
            flagship,
        })
        .id();
    for member in &members {
        commands
            .entity(*member)
            .insert(FleetMembership { fleet: fleet_entity });
    }
    fleet_entity
}

/// Dissolve a fleet, removing FleetMembership from all members and despawning the fleet entity.
pub fn dissolve_fleet(commands: &mut Commands, fleet_entity: Entity, fleet: &Fleet) {
    for member in &fleet.members {
        commands.entity(*member).remove::<FleetMembership>();
    }
    commands.entity(fleet_entity).despawn();
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::ecs::world::World;
    use crate::amount::Amt;
    use crate::components::Position;
    use crate::ship::{Owner, ShipHitpoints, ShipModifiers, ShipStats, RulesOfEngagement};
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
        }
    }

    #[test]
    fn fleet_speed_is_min_of_members() {
        let mut world = World::new();
        let ship_a = world.spawn(Ship {
            name: "Fast".to_string(),
            design_id: "courier_mk1".to_string(),
            hull_id: "corvette".to_string(),
            modules: Vec::new(),
            owner: Owner::Neutral,
            sublight_speed: 0.85,
            ftl_range: 0.0,
            player_aboard: false,
            home_port: Entity::PLACEHOLDER,
        }).id();
        let ship_b = world.spawn(Ship {
            name: "Slow".to_string(),
            design_id: "colony_ship_mk1".to_string(),
            hull_id: "freighter".to_string(),
            modules: Vec::new(),
            owner: Owner::Neutral,
            sublight_speed: 0.5,
            ftl_range: 30.0,
            player_aboard: false,
            home_port: Entity::PLACEHOLDER,
        }).id();

        let fleet = Fleet {
            name: "Test Fleet".to_string(),
            members: vec![ship_a, ship_b],
            flagship: ship_a,
        };

        let mut system_state = bevy::ecs::system::SystemState::<Query<&Ship>>::new(&mut world);
        let ships = system_state.get(&world);
        let speed = fleet.speed(&ships);
        assert!((speed - 0.5).abs() < f64::EPSILON);
    }

    #[test]
    fn fleet_ftl_range_is_min_of_members() {
        let mut world = World::new();
        let ship_a = world.spawn(Ship {
            name: "Short Range".to_string(),
            design_id: "colony_ship_mk1".to_string(),
            hull_id: "freighter".to_string(),
            modules: Vec::new(),
            owner: Owner::Neutral,
            sublight_speed: 0.5,
            ftl_range: 10.0,
            player_aboard: false,
            home_port: Entity::PLACEHOLDER,
        }).id();
        let ship_b = world.spawn(Ship {
            name: "Long Range".to_string(),
            design_id: "colony_ship_mk1".to_string(),
            hull_id: "freighter".to_string(),
            modules: Vec::new(),
            owner: Owner::Neutral,
            sublight_speed: 0.5,
            ftl_range: 30.0,
            player_aboard: false,
            home_port: Entity::PLACEHOLDER,
        }).id();

        let fleet = Fleet {
            name: "Test Fleet".to_string(),
            members: vec![ship_a, ship_b],
            flagship: ship_a,
        };

        let mut system_state = bevy::ecs::system::SystemState::<Query<&Ship>>::new(&mut world);
        let ships = system_state.get(&world);
        let range = fleet.ftl_range(&ships);
        assert!((range - 10.0).abs() < f64::EPSILON);
    }

    #[test]
    fn fleet_creation_adds_membership() {
        let mut world = World::new();
        let system = world.spawn_empty().id();
        let pos = Position { x: 0.0, y: 0.0, z: 0.0 };
        let ship_a = world.spawn((
            make_ship("explorer_mk1"),
            ShipState::Docked { system },
            pos,
            CommandQueue::default(),
            Cargo::default(),
        )).id();
        let ship_b = world.spawn((
            make_ship("colony_ship_mk1"),
            ShipState::Docked { system },
            pos,
            CommandQueue::default(),
            Cargo::default(),
        )).id();

        let members = vec![ship_a, ship_b];
        let fleet_entity = {
            let mut commands = world.commands();
            let e = create_fleet(&mut commands, "Alpha Fleet".to_string(), members, ship_a);
            e
        };
        world.flush();

        let fleet = world.get::<Fleet>(fleet_entity).expect("Fleet should exist");
        assert_eq!(fleet.name, "Alpha Fleet");
        assert_eq!(fleet.members.len(), 2);
        assert_eq!(fleet.flagship, ship_a);

        let membership_a = world.get::<FleetMembership>(ship_a).expect("Ship A should have FleetMembership");
        assert_eq!(membership_a.fleet, fleet_entity);

        let membership_b = world.get::<FleetMembership>(ship_b).expect("Ship B should have FleetMembership");
        assert_eq!(membership_b.fleet, fleet_entity);
    }

    #[test]
    fn fleet_dissolution_removes_membership() {
        let mut world = World::new();
        let system = world.spawn_empty().id();
        let pos = Position { x: 0.0, y: 0.0, z: 0.0 };
        let ship_a = world.spawn((
            make_ship("explorer_mk1"),
            ShipState::Docked { system },
            pos,
            CommandQueue::default(),
            Cargo::default(),
        )).id();
        let ship_b = world.spawn((
            make_ship("colony_ship_mk1"),
            ShipState::Docked { system },
            pos,
            CommandQueue::default(),
            Cargo::default(),
        )).id();

        // Create fleet
        let members = vec![ship_a, ship_b];
        let fleet_entity = {
            let mut commands = world.commands();
            create_fleet(&mut commands, "Alpha Fleet".to_string(), members, ship_a)
        };
        world.flush();

        // Verify membership exists
        assert!(world.get::<FleetMembership>(ship_a).is_some());
        assert!(world.get::<FleetMembership>(ship_b).is_some());

        // Dissolve fleet
        let fleet_members = world.get::<Fleet>(fleet_entity).unwrap().members.clone();
        let fleet_flagship = world.get::<Fleet>(fleet_entity).unwrap().flagship;
        let fleet_data = Fleet {
            name: "Alpha Fleet".to_string(),
            members: fleet_members,
            flagship: fleet_flagship,
        };
        {
            let mut commands = world.commands();
            dissolve_fleet(&mut commands, fleet_entity, &fleet_data);
        }
        world.flush();

        // Verify membership removed
        assert!(world.get::<FleetMembership>(ship_a).is_none());
        assert!(world.get::<FleetMembership>(ship_b).is_none());

        // Fleet entity should be despawned
        assert!(world.get_entity(fleet_entity).is_err());
    }
}
