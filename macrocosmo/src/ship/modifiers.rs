use bevy::prelude::*;

use crate::amount::SignedAmt;
use crate::modifier::{Modifier, ScopedModifiers};
use crate::ship_design::{HullRegistry, ModuleRegistry};

use super::{Ship, ShipModifiers};

/// Syncs module modifiers from equipped modules to ShipModifiers.
/// Clears and rebuilds module modifiers each time a ship's modules change.
pub fn sync_ship_module_modifiers(
    ships: Query<(Entity, &Ship), Changed<Ship>>,
    mut ship_mods: Query<&mut ShipModifiers>,
    module_registry: Res<ModuleRegistry>,
    hull_registry: Res<HullRegistry>,
) {
    for (entity, ship) in &ships {
        let Ok(mut mods) = ship_mods.get_mut(entity) else {
            continue;
        };
        // Reset all module modifiers by creating fresh scoped modifiers
        // (preserving base values but clearing modifiers)
        mods.speed = ScopedModifiers::default();
        mods.ftl_range = ScopedModifiers::default();
        mods.survey_speed = ScopedModifiers::default();
        mods.colonize_speed = ScopedModifiers::default();
        mods.evasion = ScopedModifiers::default();
        mods.cargo_capacity = ScopedModifiers::default();
        mods.attack = ScopedModifiers::default();
        mods.defense = ScopedModifiers::default();
        mods.armor_max = ScopedModifiers::default();
        mods.shield_max = ScopedModifiers::default();
        mods.shield_regen = ScopedModifiers::default();

        // Apply hull modifiers first
        if let Some(hull_def) = hull_registry.get(&ship.hull_id) {
            for mod_def in &hull_def.modifiers {
                let modifier = Modifier {
                    id: format!("hull_{}_{}", ship.hull_id, mod_def.target),
                    label: hull_def.name.clone(),
                    base_add: SignedAmt::from_f64(mod_def.base_add),
                    multiplier: SignedAmt::from_f64(mod_def.multiplier),
                    add: SignedAmt::from_f64(mod_def.add),
                    expires_at: None,
                    on_expire_event: None,
                };
                push_ship_modifier(&mut mods, &mod_def.target, modifier);
            }
        }

        // Apply module modifiers
        for (i, equipped) in ship.modules.iter().enumerate() {
            if let Some(module_def) = module_registry.modules.get(&equipped.module_id) {
                for mod_def in &module_def.modifiers {
                    let modifier = Modifier {
                        id: format!("module_{}_{}", equipped.module_id, i),
                        label: module_def.name.clone(),
                        base_add: SignedAmt::from_f64(mod_def.base_add),
                        multiplier: SignedAmt::from_f64(mod_def.multiplier),
                        add: SignedAmt::from_f64(mod_def.add),
                        expires_at: None,
                        on_expire_event: None,
                    };
                    push_ship_modifier(&mut mods, &mod_def.target, modifier);
                }
            }
        }
    }
}

/// Push a modifier to the appropriate ShipModifiers field based on target string.
fn push_ship_modifier(mods: &mut Mut<ShipModifiers>, target: &str, modifier: Modifier) {
    match target {
        "ship.speed" => mods.speed.push_modifier(modifier),
        "ship.ftl_range" => mods.ftl_range.push_modifier(modifier),
        "ship.survey_speed" => mods.survey_speed.push_modifier(modifier),
        "ship.colonize_speed" => mods.colonize_speed.push_modifier(modifier),
        "ship.evasion" => mods.evasion.push_modifier(modifier),
        "ship.cargo_capacity" => mods.cargo_capacity.push_modifier(modifier),
        "ship.attack" => mods.attack.push_modifier(modifier),
        "ship.defense" => mods.defense.push_modifier(modifier),
        "ship.armor_max" => mods.armor_max.push_modifier(modifier),
        "ship.shield_max" => mods.shield_max.push_modifier(modifier),
        "ship.shield_regen" => mods.shield_regen.push_modifier(modifier),
        _ => {}
    }
}
