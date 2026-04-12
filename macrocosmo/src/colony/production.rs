use bevy::prelude::*;

use crate::amount::{Amt, SignedAmt};
use crate::galaxy::{Planet, StarSystem};
use crate::modifier::{ModifiedValue, Modifier};
use crate::scripting::building_api::BuildingRegistry;
use crate::time_system::GameClock;

use super::{
    Buildings, Colony, LastProductionTick, ResourceCapacity, ResourceStockpile,
    AUTHORITY_DEFICIT_PENALTY,
};

/// #29: Production focus weights for colony output
#[derive(Component)]
pub struct ProductionFocus {
    pub minerals_weight: Amt,
    pub energy_weight: Amt,
    pub research_weight: Amt,
}

impl Default for ProductionFocus {
    fn default() -> Self {
        Self {
            minerals_weight: Amt::units(1),
            energy_weight: Amt::units(1),
            research_weight: Amt::units(1),
        }
    }
}

impl ProductionFocus {
    pub fn balanced() -> Self {
        Self::default()
    }
    pub fn minerals() -> Self {
        Self {
            minerals_weight: Amt::units(2),
            energy_weight: Amt::new(0, 500),
            research_weight: Amt::new(0, 500),
        }
    }
    pub fn energy() -> Self {
        Self {
            minerals_weight: Amt::new(0, 500),
            energy_weight: Amt::units(2),
            research_weight: Amt::new(0, 500),
        }
    }
    pub fn research() -> Self {
        Self {
            minerals_weight: Amt::new(0, 500),
            energy_weight: Amt::new(0, 500),
            research_weight: Amt::units(2),
        }
    }

    pub fn label(&self) -> &'static str {
        if self.minerals_weight == Amt::units(1)
            && self.energy_weight == Amt::units(1)
            && self.research_weight == Amt::units(1)
        {
            "Balanced"
        } else if self.minerals_weight > Amt::new(1, 500) {
            "Minerals"
        } else if self.energy_weight > Amt::new(1, 500) {
            "Energy"
        } else if self.research_weight > Amt::new(1, 500) {
            "Research"
        } else {
            "Custom"
        }
    }
}

/// Per-colony production rates as ModifiedValues.
#[derive(Component)]
pub struct Production {
    pub minerals_per_hexadies: ModifiedValue,
    pub energy_per_hexadies: ModifiedValue,
    pub research_per_hexadies: ModifiedValue,
    pub food_per_hexadies: ModifiedValue,
}

/// Synchronise building-slot bonuses as modifiers on the Production component.
/// For each occupied building slot, a `base_add` modifier is pushed.
/// For empty slots, any previously set modifier is removed.
/// Runs BEFORE tick_production so that `.final_value()` reflects current buildings.
pub fn sync_building_modifiers(
    registry: Res<BuildingRegistry>,
    mut query: Query<(&Buildings, &mut Production)>,
) {
    for (buildings, mut prod) in &mut query {
        for (slot_idx, slot) in buildings.slots.iter().enumerate() {
            let id_m = format!("building_slot_{}_minerals", slot_idx);
            let id_e = format!("building_slot_{}_energy", slot_idx);
            let id_r = format!("building_slot_{}_research", slot_idx);
            let id_f = format!("building_slot_{}_food", slot_idx);
            if let Some(bid) = slot {
                let Some(def) = registry.get(bid.as_str()) else {
                    warn!("Building '{}' not found in registry", bid);
                    continue;
                };
                let (m, e, r, f) = def.production_bonus();
                let label = format!("{} (slot {})", def.name, slot_idx);
                if m != Amt::ZERO {
                    prod.minerals_per_hexadies.push_modifier(Modifier {
                        id: id_m,
                        label: label.clone(),
                        base_add: SignedAmt::from_amt(m),
                        multiplier: SignedAmt::ZERO,
                        add: SignedAmt::ZERO,
                        expires_at: None,
                        on_expire_event: None,
                    });
                } else {
                    prod.minerals_per_hexadies.pop_modifier(&id_m);
                }
                if e != Amt::ZERO {
                    prod.energy_per_hexadies.push_modifier(Modifier {
                        id: id_e,
                        label: label.clone(),
                        base_add: SignedAmt::from_amt(e),
                        multiplier: SignedAmt::ZERO,
                        add: SignedAmt::ZERO,
                        expires_at: None,
                        on_expire_event: None,
                    });
                } else {
                    prod.energy_per_hexadies.pop_modifier(&id_e);
                }
                if r != Amt::ZERO {
                    prod.research_per_hexadies.push_modifier(Modifier {
                        id: id_r,
                        label: label.clone(),
                        base_add: SignedAmt::from_amt(r),
                        multiplier: SignedAmt::ZERO,
                        add: SignedAmt::ZERO,
                        expires_at: None,
                        on_expire_event: None,
                    });
                } else {
                    prod.research_per_hexadies.pop_modifier(&id_r);
                }
                if f != Amt::ZERO {
                    prod.food_per_hexadies.push_modifier(Modifier {
                        id: id_f,
                        label,
                        base_add: SignedAmt::from_amt(f),
                        multiplier: SignedAmt::ZERO,
                        add: SignedAmt::ZERO,
                        expires_at: None,
                        on_expire_event: None,
                    });
                } else {
                    prod.food_per_hexadies.pop_modifier(&id_f);
                }
            } else {
                prod.minerals_per_hexadies.pop_modifier(&id_m);
                prod.energy_per_hexadies.pop_modifier(&id_e);
                prod.research_per_hexadies.pop_modifier(&id_r);
                prod.food_per_hexadies.pop_modifier(&id_f);
            }
        }
    }
}

/// #29: tick_production uses ProductionFocus weights and building bonuses
/// #44: Research is no longer accumulated in the stockpile; emitted via emit_research
/// #73: Non-capital colonies have production reduced when capital authority is depleted
pub fn tick_production(
    clock: Res<GameClock>,
    last_tick: Res<LastProductionTick>,
    colonies: Query<(&Colony, &Production, Option<&ProductionFocus>)>,
    mut stockpiles: Query<(&mut ResourceStockpile, Option<&ResourceCapacity>), With<StarSystem>>,
    stars: Query<&StarSystem>,
    planets: Query<&Planet>,
) {
    let delta = clock.elapsed - last_tick.0;
    if delta <= 0 {
        return;
    }
    let d = delta as u64;
    let d_amt = Amt::units(d);

    // #73: Check if the capital has an authority deficit.
    let capital_authority = {
        let capital_sys = colonies.iter().find_map(|(colony, _, _)| {
            colony.system(&planets).filter(|&sys| stars.get(sys).ok().is_some_and(|s| s.is_capital))
        });
        capital_sys.and_then(|sys| stockpiles.get(sys).ok().map(|(s, _)| s.authority))
    };
    let authority_deficit = matches!(capital_authority, Some(a) if a == Amt::ZERO);

    // Collect production deltas per system
    let mut system_deltas: std::collections::HashMap<Entity, (Amt, Amt, Amt)> = std::collections::HashMap::new();
    for (colony, prod, focus) in &colonies {
        let Some(sys) = colony.system(&planets) else { continue };
        let (mw, ew) = match focus {
            Some(f) => (f.minerals_weight, f.energy_weight),
            None => (Amt::units(1), Amt::units(1)),
        };

        // #73: Apply authority deficit penalty to non-capital colonies
        let is_capital = stars.get(sys).ok().is_some_and(|s| s.is_capital);
        let authority_multiplier = if authority_deficit && !is_capital {
            AUTHORITY_DEFICIT_PENALTY
        } else {
            Amt::units(1)
        };

        let minerals = prod.minerals_per_hexadies.final_value().mul_amt(mw).mul_amt(d_amt).mul_amt(authority_multiplier);
        let energy = prod.energy_per_hexadies.final_value().mul_amt(ew).mul_amt(d_amt).mul_amt(authority_multiplier);
        let food = prod.food_per_hexadies.final_value().mul_amt(d_amt).mul_amt(authority_multiplier);

        let entry = system_deltas.entry(sys).or_insert((Amt::ZERO, Amt::ZERO, Amt::ZERO));
        entry.0 = entry.0.add(minerals);
        entry.1 = entry.1.add(energy);
        entry.2 = entry.2.add(food);
    }

    // Apply deltas to system stockpiles
    for (sys, (minerals, energy, food)) in system_deltas {
        if let Ok((mut stockpile, capacity)) = stockpiles.get_mut(sys) {
            stockpile.minerals = stockpile.minerals.add(minerals);
            stockpile.energy = stockpile.energy.add(energy);
            stockpile.food = stockpile.food.add(food);
            // Clamp resources to capacity
            if let Some(cap) = capacity {
                stockpile.minerals = stockpile.minerals.min(cap.minerals);
                stockpile.energy = stockpile.energy.min(cap.energy);
                stockpile.food = stockpile.food.min(cap.food);
                stockpile.authority = stockpile.authority.min(cap.authority);
            }
        }
    }
}
