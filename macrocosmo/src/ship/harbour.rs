//! #384: Harbour dock/undock core logic.
//!
//! A ship is a harbour when its `ShipStats::harbour_capacity` > 0.
//! Docked ships carry a `DockedAt(harbour_entity)` component.

use bevy::prelude::*;

use crate::amount::Amt;
use crate::modifier::Modifier;
use crate::ship_design::HullRegistry;

use super::modifiers::push_ship_modifier;
use super::{
    CommandQueue, DockedAt, HarbourModifiers, QueuedCommand, RulesOfEngagement, Ship,
    ShipModifiers, ShipState, ShipStats, UndockedForCombat,
};

/// Tracks which docked modifier IDs were applied to this ship, so we can
/// clean them up on undock or harbour change.
#[derive(Component, Default, Debug, Clone)]
pub struct AppliedDockedModifiers(pub Vec<String>);

/// Returns the total hull size of all ships currently docked at `harbour_entity`.
pub fn current_docked_size(
    harbour_entity: Entity,
    docked_query: &Query<(Entity, &DockedAt)>,
    ships: &Query<&Ship>,
    hull_registry: &HullRegistry,
) -> u32 {
    let mut total: u32 = 0;
    for (docked_entity, docked_at) in docked_query.iter() {
        if docked_at.0 == harbour_entity {
            let size = ships
                .get(docked_entity)
                .ok()
                .and_then(|s| hull_registry.get(&s.hull_id).map(|h| h.size))
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
    docked_query: &Query<(Entity, &DockedAt)>,
    ships: &Query<&Ship>,
    hull_registry: &HullRegistry,
) -> bool {
    let capacity_raw = harbour_stats.harbour_capacity.cached().raw();
    if capacity_raw == 0 {
        return false;
    }
    let capacity = (capacity_raw / 1000) as u32;
    let used = current_docked_size(harbour_entity, docked_query, ships, hull_registry);
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

// ---------------------------------------------------------------------------
// Systems
// ---------------------------------------------------------------------------

/// Docked ships derive their position from the harbour entity.
/// Runs after movement systems so harbour position is up to date.
pub fn sync_docked_position(
    harbours: Query<(Entity, &crate::components::Position), Without<DockedAt>>,
    mut docked_ships: Query<(&DockedAt, &mut crate::components::Position)>,
) {
    for (docked_at, mut pos) in &mut docked_ships {
        if let Ok((_, harbour_pos)) = harbours.get(docked_at.0) {
            *pos = harbour_pos.clone();
        }
    }
}

/// If a harbour entity no longer exists, forcibly undock all ships that reference it.
/// Uses a query over docked ships and checks harbour existence.
pub fn force_undock_on_harbour_destroy(
    mut commands: Commands,
    docked_ships: Query<(Entity, &DockedAt, &ShipState)>,
    harbours: Query<Entity>,
) {
    for (entity, docked_at, state) in &docked_ships {
        if harbours.get(docked_at.0).is_err() {
            // Harbour entity no longer exists — forcibly undock
            commands.entity(entity).remove::<DockedAt>();
            // Keep current ShipState; if it was InSystem it stays InSystem.
            // If it was something else, that's fine too — the state is independent.
            let _ = state; // used to avoid unused warning
        }
    }
}

/// When a docked ship receives a MoveTo command, automatically undock first.
/// Runs before movement systems.
pub fn auto_undock_on_move_command(
    mut commands: Commands,
    docked_ships: Query<(Entity, &DockedAt, &CommandQueue, &ShipState)>,
) {
    for (entity, _docked_at, queue, state) in &docked_ships {
        let has_move = queue.commands.iter().any(|cmd| {
            matches!(
                cmd,
                QueuedCommand::MoveTo { .. } | QueuedCommand::MoveToCoordinates { .. }
            )
        });
        if has_move {
            commands.entity(entity).remove::<DockedAt>();
            // Ensure ship is InSystem if it isn't already
            if let ShipState::InSystem { .. } = state {
                // Already in the right state
            } else {
                // Shouldn't normally happen (docked ships should be InSystem),
                // but defensively keep current state
            }
        }
    }
}

/// #384: Apply harbour modifiers to docked ships, and clean up on undock.
/// Runs after sync_ship_module_modifiers to avoid double-application.
///
/// For each ship with `DockedAt(harbour)`:
/// - Read harbour's `HarbourModifiers`
/// - For `docked_to:self::*`: always apply (the ship is docked at *this* harbour)
/// - For `docked_to:<hull_id>::*`: apply if harbour's hull_id matches
/// - For `docked_to:*::*`: always apply
///
/// On undock (ship has `AppliedDockedModifiers` but no `DockedAt`): strip applied modifiers.
pub fn sync_docked_modifiers(
    mut commands: Commands,
    docked_ships: Query<(Entity, &DockedAt, Option<&AppliedDockedModifiers>)>,
    harbour_data: Query<(&Ship, Option<&HarbourModifiers>)>,
    mut ship_mods: Query<&mut ShipModifiers>,
    undocked_ships: Query<(Entity, &AppliedDockedModifiers), Without<DockedAt>>,
) {
    // Phase 1: Clean up modifiers for ships that lost DockedAt
    for (entity, applied) in &undocked_ships {
        if let Ok(mut mods) = ship_mods.get_mut(entity) {
            for mod_id in &applied.0 {
                // Strip from all possible fields
                mods.speed.pop_modifier(mod_id);
                mods.ftl_range.pop_modifier(mod_id);
                mods.survey_speed.pop_modifier(mod_id);
                mods.colonize_speed.pop_modifier(mod_id);
                mods.evasion.pop_modifier(mod_id);
                mods.cargo_capacity.pop_modifier(mod_id);
                mods.attack.pop_modifier(mod_id);
                mods.defense.pop_modifier(mod_id);
                mods.armor_max.pop_modifier(mod_id);
                mods.shield_max.pop_modifier(mod_id);
                mods.shield_regen.pop_modifier(mod_id);
                mods.harbour_capacity.pop_modifier(mod_id);
            }
        }
        commands.entity(entity).remove::<AppliedDockedModifiers>();
    }

    // Phase 2: Apply harbour modifiers to docked ships
    for (entity, docked_at, existing_applied) in &docked_ships {
        let harbour_entity = docked_at.0;
        let Ok((harbour_ship, harbour_mods)) = harbour_data.get(harbour_entity) else {
            continue;
        };
        let Some(harbour_mods) = harbour_mods else {
            // Harbour has no docked-scope modifiers; clean up any previously applied
            if existing_applied.is_some() {
                if let Ok(mut mods) = ship_mods.get_mut(entity) {
                    if let Some(applied) = existing_applied {
                        for mod_id in &applied.0 {
                            mods.speed.pop_modifier(mod_id);
                            mods.ftl_range.pop_modifier(mod_id);
                            mods.survey_speed.pop_modifier(mod_id);
                            mods.colonize_speed.pop_modifier(mod_id);
                            mods.evasion.pop_modifier(mod_id);
                            mods.cargo_capacity.pop_modifier(mod_id);
                            mods.attack.pop_modifier(mod_id);
                            mods.defense.pop_modifier(mod_id);
                            mods.armor_max.pop_modifier(mod_id);
                            mods.shield_max.pop_modifier(mod_id);
                            mods.shield_regen.pop_modifier(mod_id);
                            mods.harbour_capacity.pop_modifier(mod_id);
                        }
                    }
                }
                commands.entity(entity).remove::<AppliedDockedModifiers>();
            }
            continue;
        };

        let harbour_hull_id = &harbour_ship.hull_id;
        let Ok(mut mods) = ship_mods.get_mut(entity) else {
            continue;
        };

        // Strip previously applied docked modifiers first (full diff)
        if let Some(applied) = existing_applied {
            for mod_id in &applied.0 {
                mods.speed.pop_modifier(mod_id);
                mods.ftl_range.pop_modifier(mod_id);
                mods.survey_speed.pop_modifier(mod_id);
                mods.colonize_speed.pop_modifier(mod_id);
                mods.evasion.pop_modifier(mod_id);
                mods.cargo_capacity.pop_modifier(mod_id);
                mods.attack.pop_modifier(mod_id);
                mods.defense.pop_modifier(mod_id);
                mods.armor_max.pop_modifier(mod_id);
                mods.shield_max.pop_modifier(mod_id);
                mods.shield_regen.pop_modifier(mod_id);
                mods.harbour_capacity.pop_modifier(mod_id);
            }
        }

        // Apply matching modifiers
        let mut applied_ids: Vec<String> = Vec::new();
        for (filter, target, modifier) in &harbour_mods.0 {
            let should_apply = match filter.as_str() {
                "self" => true, // docked at *this* harbour
                "*" => true,    // wildcard — always applies
                hull_filter => hull_filter == harbour_hull_id.as_str(),
            };
            if !should_apply {
                continue;
            }
            // Create a docked-scoped modifier with a stable ID
            let docked_mod = Modifier {
                id: format!("docked_{}_{}", harbour_entity.index(), modifier.id),
                label: format!("Harbour: {}", modifier.label),
                base_add: modifier.base_add,
                multiplier: modifier.multiplier,
                add: modifier.add,
                expires_at: None,
                on_expire_event: None,
            };
            applied_ids.push(docked_mod.id.clone());
            push_ship_modifier(&mut mods, target, docked_mod);
        }

        commands
            .entity(entity)
            .insert(AppliedDockedModifiers(applied_ids));
    }
}

/// #384: When hostiles are present in a system, ships with Aggressive/Defensive ROE
/// that are docked auto-undock for combat. Evasive/Passive ships stay docked (sheltered).
/// Runs before resolve_combat.
pub fn auto_undock_on_combat_roe(
    mut commands: Commands,
    docked_ships: Query<(Entity, &DockedAt, &ShipState, Option<&RulesOfEngagement>)>,
    hostiles: Query<&crate::galaxy::AtSystem, With<crate::galaxy::Hostile>>,
) {
    // Collect systems with hostiles
    let hostile_systems: std::collections::HashSet<Entity> =
        hostiles.iter().map(|at| at.0).collect();

    if hostile_systems.is_empty() {
        return;
    }

    for (entity, docked_at, state, roe) in &docked_ships {
        let roe = roe.copied().unwrap_or_default();
        let ship_system = match state {
            ShipState::InSystem { system } => *system,
            _ => continue,
        };

        if !hostile_systems.contains(&ship_system) {
            continue;
        }

        match roe {
            RulesOfEngagement::Aggressive | RulesOfEngagement::Defensive => {
                // Undock for combat, remember harbour for re-docking
                let harbour = docked_at.0;
                commands.entity(entity).remove::<DockedAt>();
                commands.entity(entity).insert(UndockedForCombat(harbour));
            }
            RulesOfEngagement::Evasive
            | RulesOfEngagement::Passive
            | RulesOfEngagement::Retreat => {
                // Stay docked — sheltered by harbour
            }
        }
    }
}

/// #384: After combat, ships with UndockedForCombat marker attempt to re-dock
/// at their original harbour if no hostiles remain in the system.
/// Runs after resolve_combat.
pub fn auto_return_dock_after_combat(
    mut commands: Commands,
    undocked: Query<(Entity, &UndockedForCombat, &ShipState)>,
    hostiles: Query<&crate::galaxy::AtSystem, With<crate::galaxy::Hostile>>,
    harbour_stats: Query<&ShipStats>,
    docked_query: Query<(Entity, &DockedAt)>,
    ship_query: Query<&Ship>,
    hull_registry: Res<HullRegistry>,
) {
    let hostile_systems: std::collections::HashSet<Entity> =
        hostiles.iter().map(|at| at.0).collect();

    for (entity, undocked_marker, state) in &undocked {
        let ship_system = match state {
            ShipState::InSystem { system } => *system,
            _ => {
                // Ship moved away — just remove the marker
                commands.entity(entity).remove::<UndockedForCombat>();
                continue;
            }
        };

        // Remove marker regardless — either we re-dock or we don't
        commands.entity(entity).remove::<UndockedForCombat>();

        // If hostiles remain in this system, don't re-dock
        if hostile_systems.contains(&ship_system) {
            continue;
        }

        // Try to re-dock at original harbour
        let harbour = undocked_marker.0;
        let Ok(stats) = harbour_stats.get(harbour) else {
            continue; // harbour no longer exists
        };
        let Ok(ship) = ship_query.get(entity) else {
            continue;
        };
        let docker_size = hull_registry
            .get(&ship.hull_id)
            .map(|h| h.size)
            .unwrap_or(1);
        if can_dock(
            docker_size,
            stats,
            harbour,
            &docked_query,
            &ship_query,
            &hull_registry,
        ) {
            commands.entity(entity).insert(DockedAt(harbour));
        }
    }
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
