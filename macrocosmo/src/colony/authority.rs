use bevy::prelude::*;

use crate::amount::Amt;
use crate::galaxy::{Planet, StarSystem, Sovereignty};
use crate::modifier::ModifiedValue;
use crate::ship::Owner;
use crate::time_system::GameClock;

use super::{
    Colony, LastProductionTick, ResourceCapacity, ResourceStockpile,
};

/// Default authority produced per hexady by the capital colony.
/// #160: canonical value is `GameBalance.base_authority_per_hexadies`.
pub const BASE_AUTHORITY_PER_HEXADIES: Amt = Amt::units(1);

/// Default authority cost per hexady for each non-capital colony.
/// #160: canonical value is `GameBalance.authority_cost_per_colony`.
pub const AUTHORITY_COST_PER_COLONY: Amt = Amt::new(0, 500);

/// Production efficiency multiplier applied to non-capital colonies when
/// the capital's authority stockpile is depleted.
/// 0.5 as fixed-point: Amt(500) means ×0.500
pub const AUTHORITY_DEFICIT_PENALTY: Amt = Amt::new(0, 500);

/// Configurable authority parameters. Tech effects can push modifiers to
/// adjust authority production or cost scaling.
#[derive(Resource, Component)]
pub struct AuthorityParams {
    /// Authority produced per hexady by the capital colony. Base = 1.0
    pub production: ModifiedValue,
    /// Authority cost per hexady per non-capital colony. Base = 0.5
    pub cost_per_colony: ModifiedValue,
}

impl Default for AuthorityParams {
    fn default() -> Self {
        Self {
            production: ModifiedValue::new(BASE_AUTHORITY_PER_HEXADIES),
            cost_per_colony: ModifiedValue::new(AUTHORITY_COST_PER_COLONY),
        }
    }
}

/// #73: Authority production and empire-scale consumption.
///
/// - The capital colony produces `BASE_AUTHORITY_PER_HEXADIES` authority per hexady.
/// - Each non-capital colony costs `AUTHORITY_COST_PER_COLONY` authority per hexady,
///   deducted from the capital's stockpile.
/// - When the capital's authority reaches 0, non-capital colonies suffer a production
///   efficiency penalty (applied in `tick_production`).
///
/// NOTE: Remote command costs (one-time authority cost when issuing commands to
/// distant colonies) are not implemented here -- they belong in the communication
/// module and will be handled separately.
pub fn tick_authority(
    clock: Res<GameClock>,
    last_tick: Res<LastProductionTick>,
    empire_authority_q: Query<&AuthorityParams, With<crate::player::PlayerEmpire>>,
    colonies: Query<&Colony>,
    mut stockpiles: Query<(&mut ResourceStockpile, Option<&ResourceCapacity>), With<StarSystem>>,
    stars: Query<&StarSystem>,
    planets: Query<&Planet>,
) {
    let Ok(authority_params) = empire_authority_q.single() else {
        return;
    };
    let delta = clock.elapsed - last_tick.0;
    if delta <= 0 {
        return;
    }
    let d = delta as u64;

    // First pass: find capital system and count non-capital colonies
    let mut capital_system: Option<Entity> = None;
    let mut non_capital_count: u64 = 0;
    for colony in colonies.iter() {
        if let Some(sys) = colony.system(&planets) {
            if let Ok(star) = stars.get(sys) {
                if star.is_capital {
                    capital_system = Some(sys);
                } else {
                    non_capital_count += 1;
                }
            } else {
                non_capital_count += 1;
            }
        } else {
            non_capital_count += 1;
        }
    }

    let Some(cap_sys) = capital_system else {
        return; // No capital found
    };

    // TODO (#76): Scale authority cost by light-speed distance from capital to each colony.
    // Distant colonies should cost more authority to maintain due to communication delay.
    // This should be its own issue — requires per-colony distance calculation and
    // Position queries which aren't currently available in this system.

    // Produce authority at capital system and deduct empire scale cost
    let auth_production = authority_params.production.final_value();
    let auth_cost_per_colony = authority_params.cost_per_colony.final_value();
    if let Ok((mut stockpile, capacity)) = stockpiles.get_mut(cap_sys) {
        // Capital produces authority
        stockpile.authority = stockpile.authority.add(auth_production.mul_u64(d));

        // Deduct empire scale cost for non-capital colonies
        let scale_cost = auth_cost_per_colony.mul_u64(non_capital_count).mul_u64(d);
        stockpile.authority = stockpile.authority.sub(scale_cost);

        // Clamp authority to capacity
        if let Some(cap) = capacity {
            stockpile.authority = stockpile.authority.min(cap.authority);
        }
    }
}

/// Updates sovereignty of star systems based on colony presence.
pub fn update_sovereignty(
    colonies: Query<&Colony>,
    mut sovereignties: Query<(Entity, &mut Sovereignty)>,
    empire_q: Query<Entity, With<crate::player::PlayerEmpire>>,
    planets: Query<&Planet>,
) {
    let player_empire = empire_q.single().ok();

    let mut colony_pop: std::collections::HashMap<Entity, f64> = std::collections::HashMap::new();
    for colony in &colonies {
        if let Some(sys) = colony.system(&planets) {
            *colony_pop.entry(sys).or_insert(0.0) += colony.population;
        }
    }

    for (entity, mut sov) in &mut sovereignties {
        if let Some(&pop) = colony_pop.get(&entity) {
            sov.owner = player_empire.map(Owner::Empire);
            sov.control_score = pop;
        } else {
            sov.owner = None;
            sov.control_score = 0.0;
        }
    }
}
