//! #300 (S-6): Defense Fleet auto-composition.
//!
//! When an Infrastructure Core deploys to a star system, a dedicated
//! **Defense Fleet** is created to host the Core ship (and, in the future,
//! defense platforms built at the system). The [`DefenseFleet`] marker
//! component on the fleet entity ties the fleet to a specific star system
//! and distinguishes it from ad-hoc player-created fleets.
//!
//! # Invariants
//!
//! * Each star system has at most one Defense Fleet (one-to-one with the
//!   Infrastructure Core).
//! * The Core ship is always a member of the Defense Fleet and is always
//!   the flagship.
//! * The Defense Fleet is pruned by [`super::prune_empty_fleets`] when its
//!   last member is destroyed (same lifecycle as any other fleet).

use bevy::prelude::*;

/// Marker component on a [`Fleet`](super::Fleet) entity that designates it
/// as the system-level Defense Fleet created by an Infrastructure Core
/// deploy. Downstream systems (#220) can query for this to automatically
/// assign newly-built defense platforms to the correct fleet.
#[derive(Component, Debug, Clone, Copy, Reflect)]
#[reflect(Component)]
pub struct DefenseFleet {
    /// The star system entity this Defense Fleet is anchored to.
    pub system: Entity,
}

/// Add `ship_entity` to the Defense Fleet for `system`.
///
/// Returns `true` if the ship was successfully added, `false` if no
/// Defense Fleet exists for that system.
///
/// This is a `&mut World` helper intended for future #220 (defense
/// platform auto-assignment). It maintains the bidirectional
/// `Ship.fleet` ↔ `FleetMembers` invariant.
pub fn join_defense_fleet(world: &mut World, ship_entity: Entity, system: Entity) -> bool {
    // Find the Defense Fleet for `system`.
    let fleet_entity = {
        let mut query = world.query::<(Entity, &DefenseFleet)>();
        query
            .iter(world)
            .find(|(_, df)| df.system == system)
            .map(|(e, _)| e)
    };
    let Some(fleet_entity) = fleet_entity else {
        return false;
    };

    // Update Ship.fleet back-pointer.
    if let Some(mut ship) = world.get_mut::<super::Ship>(ship_entity) {
        ship.fleet = Some(fleet_entity);
    }

    // Add to FleetMembers.
    if let Some(mut members) = world.get_mut::<super::FleetMembers>(fleet_entity) {
        if !members.contains(ship_entity) {
            members.0.push(ship_entity);
        }
    }

    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ship::fleet::{Fleet, FleetMembers, create_fleet};
    use crate::ship::{Owner, Ship};

    fn make_test_ship() -> Ship {
        Ship {
            name: "Test".into(),
            design_id: "test".into(),
            hull_id: "test_hull".into(),
            modules: Vec::new(),
            owner: Owner::Neutral,
            sublight_speed: 0.0,
            ftl_range: 0.0,
            ruler_aboard: false,
            home_port: Entity::PLACEHOLDER,
            design_revision: 0,
            fleet: None,
        }
    }

    #[test]
    fn join_defense_fleet_adds_ship_to_existing_fleet() {
        let mut world = World::new();
        let system = world.spawn_empty().id();

        // Create a Core ship and its Defense Fleet.
        let core = world.spawn(make_test_ship()).id();
        let fleet = create_fleet(
            &mut world,
            "Defense Fleet".to_string(),
            vec![core],
            Some(core),
        );
        world.entity_mut(fleet).insert(DefenseFleet { system });

        // Create a new ship to join.
        let new_ship = world.spawn(make_test_ship()).id();

        assert!(join_defense_fleet(&mut world, new_ship, system));

        // Verify membership.
        let members = world.get::<FleetMembers>(fleet).unwrap();
        assert!(members.contains(new_ship));
        assert_eq!(members.len(), 2);

        // Verify back-pointer.
        assert_eq!(world.get::<Ship>(new_ship).unwrap().fleet, Some(fleet));
    }

    #[test]
    fn join_defense_fleet_returns_false_when_no_fleet() {
        let mut world = World::new();
        let system = world.spawn_empty().id();
        let ship = world.spawn(make_test_ship()).id();

        assert!(!join_defense_fleet(&mut world, ship, system));
    }

    #[test]
    fn join_defense_fleet_idempotent() {
        let mut world = World::new();
        let system = world.spawn_empty().id();
        let core = world.spawn(make_test_ship()).id();
        let fleet = create_fleet(
            &mut world,
            "Defense Fleet".to_string(),
            vec![core],
            Some(core),
        );
        world.entity_mut(fleet).insert(DefenseFleet { system });

        // Join twice.
        assert!(join_defense_fleet(&mut world, core, system));
        assert!(join_defense_fleet(&mut world, core, system));

        let members = world.get::<FleetMembers>(fleet).unwrap();
        assert_eq!(members.len(), 1, "no duplicate membership");
    }
}
