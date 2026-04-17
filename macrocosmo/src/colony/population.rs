use bevy::prelude::*;

use crate::amount::Amt;
use crate::galaxy::{Planet, StarSystem};
use crate::modifier::ModifiedValue;
use crate::time_system::GameClock;

use super::{Colony, LastProductionTick, Production, ResourceStockpile};

/// Colony-level food consumption as a ModifiedValue (food/hexady).
/// The sync_food_consumption system sets the base each tick based on population;
/// tech modifiers (e.g. Hydroponics -20%) stay attached as multiplier modifiers.
#[derive(Component)]
pub struct FoodConsumption {
    pub food_per_hexadies: ModifiedValue,
}

impl Default for FoodConsumption {
    fn default() -> Self {
        Self {
            food_per_hexadies: ModifiedValue::new(Amt::ZERO),
        }
    }
}

/// #69: Logistic population growth with carrying capacity.
/// #72: Food consumption and starvation.
///
/// K (carrying capacity) = min(BASE_CARRYING_CAPACITY * hab_score, food_production / FOOD_PER_POP)
/// Growth rate is scaled by hab_score.
/// dP/dt = r * hab_score * P * (1 - P/K) — when P > K, population declines naturally.
pub fn tick_population_growth(
    clock: Res<GameClock>,
    last_tick: Res<LastProductionTick>,
    empire_modifiers_q: Query<
        &crate::technology::EmpireModifiers,
        With<crate::player::PlayerEmpire>,
    >,
    mut colonies: Query<(&mut Colony, &Production, Option<&FoodConsumption>)>,
    mut stockpiles: Query<&mut ResourceStockpile, With<StarSystem>>,
    planet_attrs: Query<&crate::galaxy::SystemAttributes, With<Planet>>,
    planets: Query<&Planet>,
) {
    use crate::galaxy::{BASE_CARRYING_CAPACITY, FOOD_PER_POP_PER_HEXADIES};

    let Ok(empire_modifiers) = empire_modifiers_q.single() else {
        return;
    };

    let delta = clock.elapsed - last_tick.0;
    if delta <= 0 {
        return;
    }
    let d = delta as u64;

    // Collect colony data into a Vec to avoid borrow conflicts
    let colony_data: Vec<(Entity, f64, f64, Amt, Amt, f64, Entity)> = colonies
        .iter()
        .filter_map(|(colony, production, food_consumption)| {
            let sys = colony.system(&planets)?;
            let food_consumed = if let Some(fc) = food_consumption {
                fc.food_per_hexadies.final_value().mul_u64(d)
            } else {
                Amt::from_f64(colony.population)
                    .mul_amt(FOOD_PER_POP_PER_HEXADIES)
                    .mul_u64(d)
            };
            let hab_score = planet_attrs
                .get(colony.planet)
                .map(|attr| attr.habitability)
                .unwrap_or(0.5);
            let food_prod = production.food_per_hexadies.final_value();
            Some((
                colony.planet,
                colony.population,
                colony.growth_rate,
                food_consumed,
                food_prod,
                hab_score,
                sys,
            ))
        })
        .collect();

    // Process each colony: deduct food from system stockpile, update population
    for (_planet_entity, _population, _growth_rate, food_consumed, _food_prod, _hab_score, sys) in
        &colony_data
    {
        // Deduct food from system stockpile
        if let Ok(mut stockpile) = stockpiles.get_mut(*sys) {
            stockpile.food = stockpile.food.sub(*food_consumed);
        }
    }

    // Second pass: update colony populations based on updated stockpile
    for (planet_entity, _population, _growth_rate, _food_consumed, food_prod, hab_score, sys) in
        &colony_data
    {
        let food_at_zero = stockpiles
            .get(*sys)
            .ok()
            .is_some_and(|s| s.food == Amt::ZERO);

        // Find and mutate the colony
        for (mut colony, _production, _food_consumption) in &mut colonies {
            if colony.planet != *planet_entity {
                continue;
            }

            if food_at_zero {
                let starvation_loss = colony.population * 0.01 * d as f64;
                colony.population = (colony.population - starvation_loss).max(1.0);
            } else {
                let k_habitat = BASE_CARRYING_CAPACITY * hab_score;
                let k_food = if FOOD_PER_POP_PER_HEXADIES.raw() > 0 {
                    food_prod.div_amt(FOOD_PER_POP_PER_HEXADIES).to_f64()
                } else {
                    k_habitat
                };
                let k = k_habitat.min(k_food).max(1.0);

                let effective_growth =
                    colony.growth_rate + empire_modifiers.population_growth.final_value().to_f64();
                let dp = effective_growth
                    * hab_score
                    * colony.population
                    * (1.0 - colony.population / k)
                    * d as f64;
                colony.population = (colony.population + dp).max(1.0);
            }
            break;
        }
    }
}

/// Synchronise food consumption based on current population.
/// Sets the ModifiedValue base to `population * FOOD_PER_POP_PER_HEXADIES`.
/// Any tech modifiers (e.g. Hydroponics -20%) remain attached as multiplier modifiers.
/// Runs BEFORE tick_population_growth.
pub fn sync_food_consumption(mut query: Query<(&Colony, &mut FoodConsumption)>) {
    use crate::galaxy::FOOD_PER_POP_PER_HEXADIES;

    for (colony, mut consumption) in &mut query {
        let base = Amt::from_f64(colony.population).mul_amt(FOOD_PER_POP_PER_HEXADIES);
        consumption.food_per_hexadies.set_base(base);
    }
}
