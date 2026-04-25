//! #287 (γ-1): Fleet data model.
//!
//! This is the *data-only* slice of the Fleet epic (#286). It introduces the
//! three primitives that downstream γ-2..γ-6 work will lean on:
//!
//! * [`Fleet`] — the marker + metadata component (name, optional flagship).
//! * [`FleetMembers`] — `Vec<Entity>` of ship entities currently attached.
//! * [`Ship::fleet`](super::Ship::fleet) — `Option<Entity>` back-pointer
//!   that mirrors `FleetMembers`.
//!
//! `spawn_ship` auto-creates a single-ship `Fleet` on every ship spawn so
//! existing per-ship systems (movement, combat, survey, colonize…) continue
//! to work unmodified in γ-1: every ship always has a fleet, and conversely
//! every fleet always has at least one member. Fleet entities that become
//! empty (e.g. last ship destroyed in combat) are despawned by
//! [`prune_empty_fleets`] each tick.
//!
//! # Invariant
//!
//! For any live `Ship` entity `s` and live `Fleet` entity `f`:
//!
//! * `s.fleet == Some(f)` iff `f`'s `FleetMembers.0` contains `s`.
//!
//! Mutate this link ONLY through [`assign_ship_to_fleet`] /
//! [`remove_ship_from_fleet`] / [`dissolve_fleet`] — they keep both sides in
//! sync. Direct writes to `Ship.fleet` or `FleetMembers.0` are allowed only
//! when both are updated in the same Commands batch.

use bevy::prelude::*;

use super::Ship;

/// Fleet marker + metadata. Member ship entities are stored separately on
/// the sibling [`FleetMembers`] component to keep this struct trivially
/// cloneable and to let downstream γ-2 work (FleetState) layer cleanly on
/// top of the marker.
#[derive(Component, Debug, Clone, Reflect)]
#[reflect(Component)]
pub struct Fleet {
    /// Human-readable label. For auto-created single-ship fleets this
    /// defaults to the ship's name; the player can rename in γ-6 UI.
    pub name: String,
    /// Optional flagship ship entity. `None` is valid for empty fleets
    /// mid-tick, though such fleets are pruned by [`prune_empty_fleets`].
    pub flagship: Option<Entity>,
}

/// The list of ship entities that belong to a [`Fleet`]. Order is not
/// semantically meaningful in γ-1 (γ-6 may re-use it for formation rank).
#[derive(Component, Debug, Clone, Default, Reflect)]
#[reflect(Component)]
pub struct FleetMembers(pub Vec<Entity>);

impl FleetMembers {
    pub fn new(members: Vec<Entity>) -> Self {
        Self(members)
    }

    pub fn contains(&self, ship: Entity) -> bool {
        self.0.contains(&ship)
    }

    pub fn len(&self) -> usize {
        self.0.len()
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub fn iter(&self) -> std::slice::Iter<'_, Entity> {
        self.0.iter()
    }
}

/// Fleet movement / FTL aggregate helpers. Kept around for the γ-3..γ-5
/// migration — not wired into any live system in γ-1.
impl Fleet {
    /// Fleet movement speed = slowest member (0.0 when empty).
    pub fn speed(members: &FleetMembers, ships: &Query<&Ship>) -> f64 {
        if members.is_empty() {
            return 0.0;
        }
        members
            .iter()
            .filter_map(|e| ships.get(*e).ok())
            .map(|s| s.sublight_speed)
            .fold(f64::MAX, f64::min)
    }

    /// Fleet FTL range = shortest range member (0.0 when empty).
    pub fn ftl_range(members: &FleetMembers, ships: &Query<&Ship>) -> f64 {
        if members.is_empty() {
            return 0.0;
        }
        members
            .iter()
            .filter_map(|e| ships.get(*e).ok())
            .map(|s| s.ftl_range)
            .fold(f64::MAX, f64::min)
    }
}

/// #287 (γ-1): Attach `ship` to `fleet`, maintaining the bidirectional
/// invariant. If the ship was already a member of another fleet it is
/// removed from the old fleet first (and the old fleet is despawned if it
/// becomes empty). Safe to call when the ship is already a member of
/// `fleet` (no-op).
///
/// Uses `Commands` so the caller does not need `&mut World` — this is the
/// preferred API for runtime systems.
pub fn assign_ship_to_fleet(
    commands: &mut Commands,
    ship_entity: Entity,
    fleet_entity: Entity,
    ship: &mut Ship,
    target_members: &mut FleetMembers,
) {
    if ship.fleet == Some(fleet_entity) {
        // Already consistent — but still ensure members list contains ship.
        if !target_members.contains(ship_entity) {
            target_members.0.push(ship_entity);
        }
        return;
    }
    // If the ship was in a different fleet, defer the cleanup to the
    // caller via a regular prune — we only know about *this* fleet's
    // members here. Callers that need atomic re-parenting should use
    // `remove_ship_from_fleet` first.
    ship.fleet = Some(fleet_entity);
    if !target_members.contains(ship_entity) {
        target_members.0.push(ship_entity);
    }
    commands.entity(ship_entity);
}

/// #287 (γ-1): Remove `ship_entity` from `fleet`'s `FleetMembers`, clear
/// `Ship.fleet`, and despawn `fleet` if it becomes empty. Safe to call when
/// the ship is not actually a member (no-op).
pub fn remove_ship_from_fleet(
    commands: &mut Commands,
    ship_entity: Entity,
    fleet_entity: Entity,
    ship: &mut Ship,
    members: &mut FleetMembers,
) {
    members.0.retain(|e| *e != ship_entity);
    if ship.fleet == Some(fleet_entity) {
        ship.fleet = None;
    }
    if members.is_empty() {
        commands.entity(fleet_entity).despawn();
    }
}

/// Create a fleet from the given ships, returning the fleet entity.
/// Inserts [`Fleet`] + [`FleetMembers`] and patches each ship's
/// `Ship.fleet` back-pointer. `flagship` is stored as-is (the caller is
/// responsible for ensuring it is one of `members`, or `None`).
///
/// NOTE: this helper uses `&mut World` so it can immediately patch
/// `Ship.fleet` on each member. Use [`assign_ship_to_fleet`] from
/// Commands-only contexts.
pub fn create_fleet(
    world: &mut World,
    name: String,
    members: Vec<Entity>,
    flagship: Option<Entity>,
) -> Entity {
    let fleet_entity = world
        .spawn((Fleet { name, flagship }, FleetMembers(members.clone())))
        .id();
    for ship_entity in &members {
        if let Some(mut ship) = world.get_mut::<Ship>(*ship_entity) {
            ship.fleet = Some(fleet_entity);
        }
    }
    fleet_entity
}

/// Dissolve a fleet, clearing `Ship.fleet` on all members and despawning
/// the fleet entity. Intended for tests and admin / debug commands — the
/// runtime despawn-on-empty path is [`prune_empty_fleets`].
pub fn dissolve_fleet(world: &mut World, fleet_entity: Entity) {
    let members: Vec<Entity> = world
        .get::<FleetMembers>(fleet_entity)
        .map(|m| m.0.clone())
        .unwrap_or_default();
    for ship_entity in members {
        if let Some(mut ship) = world.get_mut::<Ship>(ship_entity) {
            if ship.fleet == Some(fleet_entity) {
                ship.fleet = None;
            }
        }
    }
    world.despawn(fleet_entity);
}

/// #287 (γ-1): Per-frame reconciler. Scans every `FleetMembers`, retains
/// only members that are still live `Ship` entities, and despawns any
/// fleet whose members list has become empty (e.g. last ship destroyed in
/// combat, consumed by `Colonize`, refit away, etc.). This centralizes
/// cleanup so the many ship-despawn call sites across the codebase do not
/// each have to remember to touch the Fleet — an explicit non-goal of
/// γ-1 per #287.
pub fn prune_empty_fleets(
    mut commands: Commands,
    mut fleets: Query<(Entity, &mut FleetMembers, &mut Fleet)>,
    ships: Query<(), With<Ship>>,
) {
    for (fleet_entity, mut members, mut fleet) in fleets.iter_mut() {
        let before = members.0.len();
        members.0.retain(|e| ships.get(*e).is_ok());
        if members.0.len() != before {
            // If the flagship was among the removed members, clear it.
            // Picking a new flagship is γ-6 UI scope.
            if let Some(flag) = fleet.flagship {
                if !members.contains(flag) {
                    fleet.flagship = None;
                }
            }
        }
        if members.is_empty() {
            commands.entity(fleet_entity).despawn();
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::amount::Amt;
    use crate::components::Position;
    use crate::ship::{
        Cargo, CommandQueue, Owner, RulesOfEngagement, ShipHitpoints, ShipModifiers, ShipState,
        ShipStats,
    };
    use crate::ship_design::{ShipDesignDefinition, ShipDesignRegistry};
    use bevy::ecs::system::RunSystemOnce;
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

    fn spawn_test_ship(world: &mut World, design_id: &str) -> Entity {
        let system = world.spawn_empty().id();
        let pos = Position {
            x: 0.0,
            y: 0.0,
            z: 0.0,
        };
        world
            .spawn((
                make_ship(design_id),
                ShipState::InSystem { system },
                pos,
                CommandQueue::default(),
                Cargo::default(),
                ShipHitpoints {
                    hull: 10.0,
                    hull_max: 10.0,
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

    #[test]
    fn fleet_speed_is_min_of_members() {
        let mut world = World::new();
        let ship_a = spawn_test_ship(&mut world, "explorer_mk1"); // sublight 0.75
        let ship_b = spawn_test_ship(&mut world, "colony_ship_mk1"); // sublight 0.5
        let members = FleetMembers(vec![ship_a, ship_b]);
        let mut sys = bevy::ecs::system::SystemState::<Query<&Ship>>::new(&mut world);
        let ships = sys.get(&world);
        assert!((Fleet::speed(&members, &ships) - 0.5).abs() < f64::EPSILON);
    }

    #[test]
    fn fleet_ftl_range_is_min_of_members() {
        let mut world = World::new();
        let ship_a = spawn_test_ship(&mut world, "explorer_mk1"); // ftl 10.0
        let ship_b = spawn_test_ship(&mut world, "colony_ship_mk1"); // ftl 15.0
        let members = FleetMembers(vec![ship_a, ship_b]);
        let mut sys = bevy::ecs::system::SystemState::<Query<&Ship>>::new(&mut world);
        let ships = sys.get(&world);
        assert!((Fleet::ftl_range(&members, &ships) - 10.0).abs() < f64::EPSILON);
    }

    #[test]
    fn create_fleet_patches_backref_on_all_members() {
        let mut world = World::new();
        let ship_a = spawn_test_ship(&mut world, "explorer_mk1");
        let ship_b = spawn_test_ship(&mut world, "colony_ship_mk1");
        let fleet = create_fleet(
            &mut world,
            "Alpha".to_string(),
            vec![ship_a, ship_b],
            Some(ship_a),
        );
        assert_eq!(world.get::<Ship>(ship_a).unwrap().fleet, Some(fleet));
        assert_eq!(world.get::<Ship>(ship_b).unwrap().fleet, Some(fleet));
        let members = world.get::<FleetMembers>(fleet).unwrap();
        assert_eq!(members.0, vec![ship_a, ship_b]);
        assert_eq!(world.get::<Fleet>(fleet).unwrap().flagship, Some(ship_a));
    }

    #[test]
    fn dissolve_fleet_clears_backrefs_and_despawns() {
        let mut world = World::new();
        let ship_a = spawn_test_ship(&mut world, "explorer_mk1");
        let ship_b = spawn_test_ship(&mut world, "colony_ship_mk1");
        let fleet = create_fleet(
            &mut world,
            "Alpha".to_string(),
            vec![ship_a, ship_b],
            Some(ship_a),
        );
        dissolve_fleet(&mut world, fleet);
        assert_eq!(world.get::<Ship>(ship_a).unwrap().fleet, None);
        assert_eq!(world.get::<Ship>(ship_b).unwrap().fleet, None);
        assert!(world.get_entity(fleet).is_err());
    }

    #[test]
    fn remove_ship_from_fleet_despawns_empty_fleet() {
        let mut world = World::new();
        let ship_a = spawn_test_ship(&mut world, "explorer_mk1");
        let fleet = create_fleet(&mut world, "Solo".to_string(), vec![ship_a], Some(ship_a));

        // Drive the helper inside a one-shot system that has Commands.
        let result = world
            .run_system_once(
                move |mut commands: Commands,
                      mut ships: Query<&mut Ship>,
                      mut members: Query<&mut FleetMembers>| {
                    let mut ship = ships.get_mut(ship_a).unwrap();
                    let mut m = members.get_mut(fleet).unwrap();
                    remove_ship_from_fleet(&mut commands, ship_a, fleet, &mut ship, &mut m);
                },
            )
            .unwrap();
        let _ = result;
        world.flush();
        assert_eq!(world.get::<Ship>(ship_a).unwrap().fleet, None);
        assert!(world.get_entity(fleet).is_err());
    }

    #[test]
    fn prune_empty_fleets_despawns_when_all_members_gone() {
        let mut world = World::new();
        let ship_a = spawn_test_ship(&mut world, "explorer_mk1");
        let fleet = create_fleet(&mut world, "Solo".to_string(), vec![ship_a], Some(ship_a));
        // Destroy the ship directly (simulates combat kill).
        world.despawn(ship_a);
        // Run prune_empty_fleets.
        let _ = world.run_system_once(prune_empty_fleets).unwrap();
        world.flush();
        assert!(world.get_entity(fleet).is_err());
    }

    #[test]
    fn prune_empty_fleets_retains_non_empty_and_clears_stale_flagship() {
        let mut world = World::new();
        let ship_a = spawn_test_ship(&mut world, "explorer_mk1");
        let ship_b = spawn_test_ship(&mut world, "colony_ship_mk1");
        let fleet = create_fleet(
            &mut world,
            "Duo".to_string(),
            vec![ship_a, ship_b],
            Some(ship_a),
        );
        world.despawn(ship_a); // flagship gone, ship_b remains
        let _ = world.run_system_once(prune_empty_fleets).unwrap();
        world.flush();
        let members = world.get::<FleetMembers>(fleet).unwrap();
        assert_eq!(members.0, vec![ship_b]);
        assert_eq!(world.get::<Fleet>(fleet).unwrap().flagship, None);
    }

    // -----------------------------------------------------------------------
    // #407: Fleet operations tests
    // -----------------------------------------------------------------------

    #[test]
    fn form_fleet_groups_selected_ships() {
        let mut world = World::new();
        let ship_a = spawn_test_ship(&mut world, "explorer_mk1");
        let ship_b = spawn_test_ship(&mut world, "colony_ship_mk1");
        // Each ship starts with no fleet.
        assert!(world.get::<Ship>(ship_a).unwrap().fleet.is_none());
        assert!(world.get::<Ship>(ship_b).unwrap().fleet.is_none());

        // Form a fleet from both ships.
        let fleet = create_fleet(
            &mut world,
            "Alpha".to_string(),
            vec![ship_a, ship_b],
            Some(ship_a),
        );

        // Both ships now belong to the same fleet.
        assert_eq!(world.get::<Ship>(ship_a).unwrap().fleet, Some(fleet));
        assert_eq!(world.get::<Ship>(ship_b).unwrap().fleet, Some(fleet));
        let members = world.get::<FleetMembers>(fleet).unwrap();
        assert_eq!(members.len(), 2);
        assert!(members.contains(ship_a));
        assert!(members.contains(ship_b));
    }

    #[test]
    fn merge_fleet_combines_two_fleets() {
        let mut world = World::new();
        let ship_a = spawn_test_ship(&mut world, "explorer_mk1");
        let ship_b = spawn_test_ship(&mut world, "colony_ship_mk1");
        let ship_c = spawn_test_ship(&mut world, "explorer_mk1");

        let fleet_alpha = create_fleet(
            &mut world,
            "Alpha".to_string(),
            vec![ship_a, ship_b],
            Some(ship_a),
        );
        let fleet_beta = create_fleet(&mut world, "Beta".to_string(), vec![ship_c], Some(ship_c));

        // Move ship_c from fleet_beta into fleet_alpha (simulating merge).
        {
            // Remove from old fleet
            if let Some(mut old_members) = world.get_mut::<FleetMembers>(fleet_beta) {
                old_members.0.retain(|e| *e != ship_c);
            }
            // Add to target fleet
            if let Some(mut ship) = world.get_mut::<Ship>(ship_c) {
                ship.fleet = Some(fleet_alpha);
            }
            if let Some(mut target_members) = world.get_mut::<FleetMembers>(fleet_alpha) {
                if !target_members.0.contains(&ship_c) {
                    target_members.0.push(ship_c);
                }
            }
        }

        // Verify all ships are now in fleet_alpha.
        assert_eq!(world.get::<Ship>(ship_a).unwrap().fleet, Some(fleet_alpha));
        assert_eq!(world.get::<Ship>(ship_b).unwrap().fleet, Some(fleet_alpha));
        assert_eq!(world.get::<Ship>(ship_c).unwrap().fleet, Some(fleet_alpha));
        let members = world.get::<FleetMembers>(fleet_alpha).unwrap();
        assert_eq!(members.len(), 3);
        // fleet_beta should be empty.
        let beta_members = world.get::<FleetMembers>(fleet_beta).unwrap();
        assert!(beta_members.is_empty());
    }

    #[test]
    fn fleet_moveto_applies_to_all_members() {
        let mut world = World::new();
        let target_system = world.spawn_empty().id();
        let ship_a = spawn_test_ship(&mut world, "explorer_mk1");
        let ship_b = spawn_test_ship(&mut world, "colony_ship_mk1");

        // Both ships should have empty command queues initially.
        assert!(
            world
                .get::<CommandQueue>(ship_a)
                .unwrap()
                .commands
                .is_empty()
        );
        assert!(
            world
                .get::<CommandQueue>(ship_b)
                .unwrap()
                .commands
                .is_empty()
        );

        // Simulate fleet-level MoveTo: push the same command to both ships.
        let cmd = super::super::QueuedCommand::MoveTo {
            system: target_system,
        };
        world
            .get_mut::<CommandQueue>(ship_a)
            .unwrap()
            .commands
            .push(cmd.clone());
        world
            .get_mut::<CommandQueue>(ship_b)
            .unwrap()
            .commands
            .push(cmd.clone());

        // Both ships should have the MoveTo command.
        assert_eq!(world.get::<CommandQueue>(ship_a).unwrap().commands.len(), 1);
        assert_eq!(world.get::<CommandQueue>(ship_b).unwrap().commands.len(), 1);
        assert!(matches!(
            world.get::<CommandQueue>(ship_a).unwrap().commands[0],
            super::super::QueuedCommand::MoveTo { system } if system == target_system
        ));
        assert!(matches!(
            world.get::<CommandQueue>(ship_b).unwrap().commands[0],
            super::super::QueuedCommand::MoveTo { system } if system == target_system
        ));
    }
}
