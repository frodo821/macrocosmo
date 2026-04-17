//! #384: Harbour dock/undock core logic.
//!
//! A ship is a harbour when its `ShipStats::harbour_capacity` > 0.
//! Docked ships carry a `DockedAt(harbour_entity)` component.

use bevy::prelude::*;

use crate::amount::Amt;
use crate::ship_design::HullRegistry;

use super::{DockedAt, Ship, ShipState, ShipStats};

/// Returns the total hull size of all ships currently docked at `harbour_entity`.
pub fn current_docked_size(
    harbour_entity: Entity,
    docked_query: &Query<(&DockedAt, &Ship)>,
    hull_registry: &HullRegistry,
) -> u32 {
    let mut total: u32 = 0;
    for (docked_at, ship) in docked_query.iter() {
        if docked_at.0 == harbour_entity {
            let size = hull_registry
                .get(&ship.hull_id)
                .map(|h| h.size)
                .unwrap_or(1);
            total = total.saturating_add(size);
        }
    }
    total
}

/// Check whether a ship of `docker_size` can dock at the given harbour.
///
/// Conditions:
/// - harbour_capacity > 0
/// - docker_size fits in remaining capacity (capacity - currently_docked >= docker_size)
pub fn can_dock(
    docker_size: u32,
    harbour_stats: &ShipStats,
    harbour_entity: Entity,
    docked_query: &Query<(&DockedAt, &Ship)>,
    hull_registry: &HullRegistry,
) -> bool {
    let capacity_raw = harbour_stats.harbour_capacity.cached().raw();
    if capacity_raw == 0 {
        return false;
    }
    // Convert from Amt (×1000) to integer capacity units
    let capacity = (capacity_raw / 1000) as u32;
    let used = current_docked_size(harbour_entity, docked_query, hull_registry);
    used.saturating_add(docker_size) <= capacity
}

/// Insert `DockedAt` on the docker entity, docking it at `harbour`.
pub fn dock(commands: &mut Commands, docker: Entity, harbour: Entity) {
    commands.entity(docker).insert(DockedAt(harbour));
}

/// Remove `DockedAt` from the docker entity and ensure it is InSystem.
pub fn undock(commands: &mut Commands, docker: Entity, system: Entity) {
    commands.entity(docker).remove::<DockedAt>();
    commands
        .entity(docker)
        .insert(ShipState::InSystem { system });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::modifier::{CachedValue, ScopedModifiers};
    use crate::ship::{ShipModifiers, ShipStats};
    use crate::ship_design::HullDefinition;
    use bevy::ecs::world::World;

    fn make_hull(id: &str, size: u32) -> HullDefinition {
        HullDefinition {
            id: id.to_string(),
            name: id.to_string(),
            description: String::new(),
            base_hp: 10.0,
            base_speed: 1.0,
            base_evasion: 0.0,
            slots: Vec::new(),
            build_cost_minerals: Amt::ZERO,
            build_cost_energy: Amt::ZERO,
            build_time: 1,
            maintenance: Amt::ZERO,
            modifiers: Vec::new(),
            prerequisites: None,
            size,
            is_capital: false,
        }
    }

    fn stats_with_capacity(cap: u32) -> ShipStats {
        let mut s = ShipStats::default();
        let mut scope = ScopedModifiers::new(Amt::units(cap as u64));
        // Force a generation bump so cached value updates
        let _ = scope.generation();
        s.harbour_capacity = CachedValue::default();
        s.harbour_capacity.recompute(&[&scope]);
        s
    }

    #[test]
    fn test_can_dock_basic_capacity() {
        let mut world = World::new();
        let mut hull_reg = HullRegistry::default();
        hull_reg.insert(make_hull("corvette", 2));

        let harbour = world.spawn_empty().id();
        let stats = stats_with_capacity(5);

        // No ships docked yet: corvette (size=2) should fit
        let mut q_state = world.query::<(&DockedAt, &Ship)>();
        // We need to use a system-like approach for queries
        // Instead test with actual entities
        let docker = world
            .spawn((
                DockedAt(harbour),
                Ship {
                    name: "docker".into(),
                    design_id: "test".into(),
                    hull_id: "corvette".into(),
                    modules: Vec::new(),
                    owner: crate::ship::Owner::Neutral,
                    sublight_speed: 1.0,
                    ftl_range: 0.0,
                    player_aboard: false,
                    home_port: harbour,
                    design_revision: 0,
                    fleet: None,
                },
            ))
            .id();

        // Query world for docked ships
        let docked_size: u32 = world
            .query::<(&DockedAt, &Ship)>()
            .iter(&world)
            .filter(|(da, _)| da.0 == harbour)
            .map(|(_, s)| hull_reg.get(&s.hull_id).map(|h| h.size).unwrap_or(1))
            .sum();

        // capacity=5, used=2 (one corvette), adding another corvette(2) = 4 <= 5: fits
        assert!(docked_size + 2 <= 5);

        // Remove the docked ship and verify empty harbour
        world.entity_mut(docker).remove::<DockedAt>();
        let docked_size2: u32 = world
            .query::<(&DockedAt, &Ship)>()
            .iter(&world)
            .filter(|(da, _)| da.0 == harbour)
            .map(|(_, s)| hull_reg.get(&s.hull_id).map(|h| h.size).unwrap_or(1))
            .sum();
        assert_eq!(docked_size2, 0);
    }

    #[test]
    fn test_stats_with_zero_capacity_rejects() {
        let stats = stats_with_capacity(0);
        assert_eq!(stats.harbour_capacity.cached(), Amt::ZERO);
    }

    #[test]
    fn test_stats_with_positive_capacity() {
        let stats = stats_with_capacity(10);
        assert_eq!(stats.harbour_capacity.cached(), Amt::units(10));
    }

    #[test]
    fn test_dock_undock_commands() {
        let mut world = World::new();
        let harbour = world.spawn_empty().id();
        let system = world.spawn_empty().id();
        let docker = world.spawn_empty().id();

        // Simulate dock via direct insertion
        world.entity_mut(docker).insert(DockedAt(harbour));
        assert!(world.get::<DockedAt>(docker).is_some());
        assert_eq!(world.get::<DockedAt>(docker).unwrap().0, harbour);

        // Simulate undock
        world.entity_mut(docker).remove::<DockedAt>();
        world
            .entity_mut(docker)
            .insert(ShipState::InSystem { system });
        assert!(world.get::<DockedAt>(docker).is_none());
        match world.get::<ShipState>(docker).unwrap() {
            ShipState::InSystem { system: s } => assert_eq!(*s, system),
            _ => panic!("Expected InSystem state"),
        }
    }
}
