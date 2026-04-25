use bevy::prelude::*;

use crate::amount::Amt;
use crate::galaxy::{Planet, StarSystem};
use crate::modifier::ModifiedValue;
use crate::species::{ColonyPopulation, SpeciesRegistry};
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
/// #414: SSoT on ColonyPopulation — growth is discrete, one individual at a
/// time, species selected by growth-rate weighted probability.
///
/// K (carrying capacity) = min(BASE_CARRYING_CAPACITY * hab_score, food_production / FOOD_PER_POP)
/// dP/dt = r_eff * hab_score * P * (1 - P/K)
/// Growth accumulates in `ColonyPopulation.growth_accumulator`; when >= 1.0,
/// one individual is added to a species chosen with probability proportional
/// to that species' `base_growth_rate`. Starvation removes one individual
/// when accumulator <= -1.0.
pub fn tick_population_growth(
    clock: Res<GameClock>,
    last_tick: Res<LastProductionTick>,
    empire_modifiers_q: Query<
        (Entity, &crate::technology::EmpireModifiers),
        With<crate::player::Empire>,
    >,
    mut colonies: Query<(
        &Colony,
        &mut ColonyPopulation,
        &Production,
        Option<&FoodConsumption>,
        &crate::faction::FactionOwner,
    )>,
    mut stockpiles: Query<&mut ResourceStockpile, With<StarSystem>>,
    planet_attrs: Query<&crate::galaxy::SystemAttributes, With<Planet>>,
    planets: Query<&Planet>,
    species_registry: Res<SpeciesRegistry>,
    rng: Res<crate::scripting::GameRng>,
) {
    use crate::galaxy::{BASE_CARRYING_CAPACITY, FOOD_PER_POP_PER_HEXADIES};

    let delta = clock.elapsed - last_tick.0;
    if delta <= 0 {
        return;
    }
    let d = delta as u64;

    // Build a map from empire entity to its EmpireModifiers for quick lookup.
    let empire_map: std::collections::HashMap<Entity, &crate::technology::EmpireModifiers> =
        empire_modifiers_q.iter().map(|(e, m)| (e, m)).collect();

    let colony_data: Vec<(Entity, u32, f64, Amt, Amt, f64, Entity, Entity)> = colonies
        .iter()
        .filter_map(
            |(colony, pop, production, food_consumption, faction_owner)| {
                let sys = colony.system(&planets)?;
                let total_pop = pop.total();
                let food_consumed = if let Some(fc) = food_consumption {
                    fc.food_per_hexadies.final_value().mul_u64(d)
                } else {
                    Amt::from_f64(total_pop as f64)
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
                    total_pop,
                    colony.growth_rate,
                    food_consumed,
                    food_prod,
                    hab_score,
                    sys,
                    faction_owner.0,
                ))
            },
        )
        .collect();

    for (
        _planet_entity,
        _population,
        _growth_rate,
        food_consumed,
        _food_prod,
        _hab_score,
        sys,
        _owner,
    ) in &colony_data
    {
        if let Ok(mut stockpile) = stockpiles.get_mut(*sys) {
            stockpile.food = stockpile.food.sub(*food_consumed);
        }
    }

    for (planet_entity, total_pop, growth_rate, _food_consumed, food_prod, hab_score, sys, owner) in
        &colony_data
    {
        let food_at_zero = stockpiles
            .get(*sys)
            .ok()
            .is_some_and(|s| s.food == Amt::ZERO);

        // Look up the empire modifiers for this colony's owner
        let pop_growth_bonus = empire_map
            .get(owner)
            .map(|m| m.population_growth.final_value().to_f64())
            .unwrap_or(0.0);

        for (colony, mut pop, _production, _food_consumption, _fo) in &mut colonies {
            if colony.planet != *planet_entity {
                continue;
            }

            let population = *total_pop as f64;
            let dp = if food_at_zero {
                -population * 0.01 * d as f64
            } else {
                let k_habitat = BASE_CARRYING_CAPACITY * hab_score;
                let k_food = if FOOD_PER_POP_PER_HEXADIES.raw() > 0 {
                    food_prod.div_amt(FOOD_PER_POP_PER_HEXADIES).to_f64()
                } else {
                    k_habitat
                };
                let k = k_habitat.min(k_food).max(1.0);

                let effective_growth = growth_rate + pop_growth_bonus;
                effective_growth * hab_score * population * (1.0 - population / k) * d as f64
            };

            pop.growth_accumulator += dp;

            while pop.growth_accumulator >= 1.0 && !pop.species.is_empty() {
                let idx = pick_species_weighted(&pop.species, &species_registry, &rng);
                pop.species[idx].population += 1;
                pop.growth_accumulator -= 1.0;
            }

            while pop.growth_accumulator <= -1.0 && pop.total() > 1 {
                let idx = pick_species_weighted(&pop.species, &species_registry, &rng);
                if pop.species[idx].population > 0 {
                    pop.species[idx].population -= 1;
                }
                pop.growth_accumulator += 1.0;
            }

            break;
        }
    }
}

/// Pick a species index with probability proportional to `base_growth_rate`.
/// Falls back to uniform selection if all rates are zero.
fn pick_species_weighted(
    species: &[crate::species::ColonySpecies],
    registry: &SpeciesRegistry,
    rng: &crate::scripting::GameRng,
) -> usize {
    use rand::Rng;
    if species.len() == 1 {
        return 0;
    }
    let weights: Vec<f64> = species
        .iter()
        .map(|s| {
            registry
                .get(&s.species_id)
                .map(|def| def.base_growth_rate.max(0.001))
                .unwrap_or(0.01)
        })
        .collect();
    let total: f64 = weights.iter().sum();
    let handle = rng.handle();
    let mut guard = handle.lock().unwrap();
    if total <= 0.0 {
        return guard.random_range(0..species.len());
    }
    let mut roll: f64 = guard.random::<f64>() * total;
    for (i, w) in weights.iter().enumerate() {
        roll -= w;
        if roll <= 0.0 {
            return i;
        }
    }
    species.len() - 1
}

/// Synchronise food consumption based on current population.
pub fn sync_food_consumption(mut query: Query<(&ColonyPopulation, &mut FoodConsumption)>) {
    use crate::galaxy::FOOD_PER_POP_PER_HEXADIES;

    for (pop, mut consumption) in &mut query {
        let base = Amt::from_f64(pop.total() as f64).mul_amt(FOOD_PER_POP_PER_HEXADIES);
        consumption.food_per_hexadies.set_base(base);
    }
}
