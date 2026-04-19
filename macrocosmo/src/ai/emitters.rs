//! Military metric emitters — read ship ECS state and emit to the AI bus.
//!
//! Phase 1 (#190): emits a single global view of the NPC empire's fleet.
//! Per-faction scoping will land in a later phase.

use bevy::prelude::*;

use crate::ai::emit::AiBusWriter;
use crate::ai::schema::ids::metric;
use crate::ship::{CoreShip, Owner, Ship, ShipHitpoints, ShipModifiers, ShipState};

/// Emit military metrics for NPC empires.
///
/// Registered under [`AiTickSet::MetricProduce`](super::AiTickSet::MetricProduce).
/// Phase 1: aggregates all empire-owned ships into a single global view.
pub fn emit_military_metrics(
    mut writer: AiBusWriter,
    ships: Query<(
        &Ship,
        &ShipHitpoints,
        &ShipModifiers,
        &ShipState,
        Option<&CoreShip>,
    )>,
) {
    let mut total_ships: f64 = 0.0;
    let mut total_attack: f64 = 0.0;
    let mut total_defense: f64 = 0.0;
    let mut total_strength: f64 = 0.0;
    let mut total_armor: f64 = 0.0;
    let mut total_shields: f64 = 0.0;
    let mut total_shield_regen: f64 = 0.0;
    let mut total_current_hp: f64 = 0.0;
    let mut total_max_hp: f64 = 0.0;
    let mut ships_in_system: f64 = 0.0;
    let mut has_flagship = false;

    for (ship, hp, mods, state, core) in &ships {
        // Phase 1: only count empire-owned ships (skip neutrals).
        if !ship.owner.is_empire() {
            continue;
        }

        total_ships += 1.0;

        let attack = mods.attack.final_value().to_f64();
        let defense = mods.defense.final_value().to_f64();
        let current_hp = hp.hull + hp.armor + hp.shield;
        let max_hp = hp.hull_max + hp.armor_max + hp.shield_max;

        total_attack += attack;
        total_defense += defense;
        total_strength += attack + defense + current_hp;
        total_armor += hp.armor;
        total_shields += hp.shield;
        total_shield_regen += hp.shield_regen;
        total_current_hp += current_hp;
        total_max_hp += max_hp;

        if matches!(state, ShipState::InSystem { .. }) {
            ships_in_system += 1.0;
        }

        if core.is_some() {
            has_flagship = true;
        }
    }

    writer.emit(&metric::my_total_ships(), total_ships);
    writer.emit(&metric::my_strength(), total_strength);
    writer.emit(&metric::my_total_attack(), total_attack);
    writer.emit(&metric::my_total_defense(), total_defense);
    writer.emit(&metric::my_armor(), total_armor);
    writer.emit(&metric::my_shields(), total_shields);
    writer.emit(&metric::my_shield_regen_rate(), total_shield_regen);

    let vulnerability = if total_max_hp > 0.0 {
        1.0 - (total_current_hp / total_max_hp)
    } else {
        0.0
    };
    writer.emit(&metric::my_vulnerability_score(), vulnerability);

    let fleet_ready = if total_ships > 0.0 {
        ships_in_system / total_ships
    } else {
        0.0
    };
    writer.emit(&metric::my_fleet_ready(), fleet_ready);

    writer.emit(
        &metric::my_has_flagship(),
        if has_flagship { 1.0 } else { 0.0 },
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ai::plugin::AiBusResource;
    use crate::ai::schema;
    use crate::amount::Amt;
    use crate::modifier::ScopedModifiers;
    use crate::time_system::{GameClock, GameSpeed};
    use macrocosmo_ai::WarningMode;

    fn test_app() -> App {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.insert_resource(GameClock::new(10));
        app.insert_resource(GameSpeed::default());
        app.insert_resource(AiBusResource::with_warning_mode(WarningMode::Silent));
        // Declare metrics on the bus.
        {
            let mut bus = app.world_mut().resource_mut::<AiBusResource>();
            schema::declare_metrics_standalone(&mut bus.0);
        }
        app.add_systems(Update, emit_military_metrics);
        app
    }

    fn spawn_ship(app: &mut App, in_system: bool, is_core: bool) -> Entity {
        let system_entity = app.world_mut().spawn_empty().id();
        let state = if in_system {
            ShipState::InSystem {
                system: system_entity,
            }
        } else {
            ShipState::SubLight {
                origin: [0.0; 3],
                destination: [1.0, 0.0, 0.0],
                target_system: None,
                departed_at: 0,
                arrival_at: 100,
            }
        };

        let mut mods = ShipModifiers::default();
        // Set base attack/defense so final_value() returns non-zero.
        mods.attack = ScopedModifiers::new(Amt::from_f64(10.0));
        mods.defense = ScopedModifiers::new(Amt::from_f64(5.0));

        let mut entity_commands = app.world_mut().spawn((
            Ship {
                name: "Test Ship".into(),
                design_id: "corvette".into(),
                hull_id: "corvette_hull".into(),
                modules: vec![],
                owner: Owner::Empire(Entity::PLACEHOLDER),
                sublight_speed: 1.0,
                ftl_range: 5.0,
                player_aboard: false,
                home_port: system_entity,
                design_revision: 0,
                fleet: None,
            },
            ShipHitpoints {
                hull: 40.0,
                hull_max: 50.0,
                armor: 15.0,
                armor_max: 20.0,
                shield: 8.0,
                shield_max: 10.0,
                shield_regen: 1.0,
            },
            mods,
            state,
        ));
        if is_core {
            entity_commands.insert(CoreShip);
        }
        entity_commands.id()
    }

    #[test]
    fn emit_military_metrics_counts_ships() {
        let mut app = test_app();
        spawn_ship(&mut app, true, false);
        spawn_ship(&mut app, true, false);
        spawn_ship(&mut app, false, false);
        app.update();

        let bus = app.world().resource::<AiBusResource>();
        let total = bus.current(&metric::my_total_ships()).unwrap();
        assert!((total - 3.0).abs() < 1e-9);
    }

    #[test]
    fn emit_military_metrics_fleet_ready_fraction() {
        let mut app = test_app();
        spawn_ship(&mut app, true, false);
        spawn_ship(&mut app, false, false);
        app.update();

        let bus = app.world().resource::<AiBusResource>();
        let ready = bus.current(&metric::my_fleet_ready()).unwrap();
        assert!((ready - 0.5).abs() < 1e-9);
    }

    #[test]
    fn emit_military_metrics_flagship_detection() {
        let mut app = test_app();
        spawn_ship(&mut app, true, false);
        app.update();

        let bus = app.world().resource::<AiBusResource>();
        let flag = bus.current(&metric::my_has_flagship()).unwrap();
        assert!((flag - 0.0).abs() < 1e-9);

        // Now add a core ship.
        spawn_ship(&mut app, true, true);
        app.update();

        let bus = app.world().resource::<AiBusResource>();
        let flag = bus.current(&metric::my_has_flagship()).unwrap();
        assert!((flag - 1.0).abs() < 1e-9);
    }

    #[test]
    fn emit_military_metrics_attack_defense() {
        let mut app = test_app();
        spawn_ship(&mut app, true, false);
        spawn_ship(&mut app, true, false);
        app.update();

        let bus = app.world().resource::<AiBusResource>();
        let attack = bus.current(&metric::my_total_attack()).unwrap();
        let defense = bus.current(&metric::my_total_defense()).unwrap();
        // 2 ships x 10 attack = 20
        assert!((attack - 20.0).abs() < 1e-9);
        // 2 ships x 5 defense = 10
        assert!((defense - 10.0).abs() < 1e-9);
    }

    #[test]
    fn emit_military_metrics_vulnerability() {
        let mut app = test_app();
        spawn_ship(&mut app, true, false);
        app.update();

        let bus = app.world().resource::<AiBusResource>();
        let vuln = bus.current(&metric::my_vulnerability_score()).unwrap();
        // hull=40/50, armor=15/20, shield=8/10 => current=63, max=80
        // vuln = 1 - 63/80 = 0.2125
        assert!((vuln - 0.2125).abs() < 1e-4);
    }
}
