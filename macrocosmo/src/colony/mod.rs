use bevy::prelude::*;

use crate::amount::Amt;
use crate::modifier::ModifiedValue;
// Re-export BuildingId and BuildingRegistry for consumers of the colony module
pub use crate::scripting::building_api::{BuildingId, BuildingRegistry};

use crate::scripting::building_api::parse_building_definitions;

pub mod authority;
pub mod build_tick;
pub mod building_queue;
pub mod colonization;
pub mod maintenance;
pub mod population;
pub mod production;
pub mod remote;
pub mod system_buildings;

pub use authority::*;
pub use building_queue::*;
pub use colonization::*;
pub use maintenance::*;
pub use population::*;
pub use production::*;
pub use remote::apply_remote_command;
pub use system_buildings::*;

pub struct ColonyPlugin;

#[derive(Resource, Default)]
pub struct LastProductionTick(pub i64);

impl Plugin for ColonyPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<LastProductionTick>()
            .init_resource::<BuildingRegistry>()
            .init_resource::<AlertCooldowns>()
            .add_systems(
                Startup,
                (
                    load_building_registry.after(crate::scripting::load_all_scripts),
                    // #297 (S-2): `spawn_capital_colony` now consults
                    // `PlayerEmpire` so the capital Colony + StarSystem get
                    // `FactionOwner` tagged at spawn. Explicit ordering
                    // guarantees the empire entity exists first.
                    spawn_capital_colony
                        .after(crate::galaxy::generate_galaxy)
                        .after(crate::player::spawn_player_empire),
                ),
            )
            // #250: Prime the colony sync pipeline at the end of Startup so
            // the UI's first frame shows correct production rates. Without
            // this, sync only runs on Update and `aggregate_job_contributions`
            // first fires after Startup completes — meaning the first render
            // reads Production with only the legacy base value loaded.
            .add_systems(
                Startup,
                (
                    sync_building_modifiers,
                    crate::species::sync_job_assignment,
                    sync_species_modifiers,
                    aggregate_job_contributions,
                )
                    .chain()
                    .after(crate::setup::run_faction_on_game_start)
                    .after(crate::setup::run_all_factions_on_game_start),
            )
            .add_systems(
                Update,
                (
                    (
                        tick_timed_effects,
                        tick_authority,
                        sync_building_modifiers,
                        crate::species::sync_job_assignment,
                        sync_species_modifiers,
                        sync_system_building_maintenance,
                        sync_maintenance_modifiers,
                        sync_food_consumption,
                        // #250: Aggregate job contributions every Update tick,
                        // independent of `delta`. This guarantees the UI sees a
                        // correct production rate even while paused.
                        aggregate_job_contributions,
                    ).chain(),
                    (
                        tick_production,
                        tick_maintenance,
                        tick_population_growth,
                        tick_build_queue,
                        tick_building_queue,
                        tick_system_building_queue,
                        tick_colonization_queue,
                        check_resource_alerts,
                        advance_production_tick,
                    ).chain(),
                )
                    .chain()
                    .after(crate::time_system::advance_game_time)
                    // #270: Arrived RemoteCommand::Colony payloads must be
                    // applied to the queue before the queue's tick consumes
                    // orders, otherwise an arrival on the same frame can be
                    // swallowed in a single tick_building_queue pass.
                    .after(crate::communication::process_pending_commands),
            )
            .add_systems(Update, (
                update_sovereignty,
                apply_pending_colonization_orders,
            ));
    }
}

#[derive(Component)]
pub struct Colony {
    pub planet: Entity,
    pub population: f64,
    pub growth_rate: f64,
}

impl Colony {
    /// Get the star system entity by looking up the planet's parent.
    pub fn system(&self, planets: &Query<&crate::galaxy::Planet>) -> Option<Entity> {
        planets.get(self.planet).ok().map(|p| p.system)
    }
}

#[derive(Component)]
pub struct ResourceStockpile {
    pub minerals: Amt,
    pub energy: Amt,
    pub research: Amt,
    pub food: Amt,
    pub authority: Amt,
}

/// #223: Per-star-system cargo-item stockpile. Shipyard-built deliverables
/// land here when construction completes, ready to be loaded onto a ship's
/// Cargo via `QueuedCommand::LoadDeliverable`.
#[derive(Component, Default, Debug, Clone)]
pub struct DeliverableStockpile {
    pub items: Vec<crate::ship::CargoItem>,
}

impl DeliverableStockpile {
    pub fn push(&mut self, item: crate::ship::CargoItem) {
        self.items.push(item);
    }

    pub fn remove(&mut self, index: usize) -> Option<crate::ship::CargoItem> {
        if index < self.items.len() {
            Some(self.items.remove(index))
        } else {
            None
        }
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }
}

#[derive(Component)]
pub struct ResourceCapacity {
    pub minerals: Amt,
    pub energy: Amt,
    pub food: Amt,
    pub authority: Amt,
}

impl Default for ResourceCapacity {
    fn default() -> Self {
        Self {
            minerals: Amt::units(1000),
            energy: Amt::units(1000),
            food: Amt::units(500),
            authority: Amt::units(10000),
        }
    }
}

/// Global construction cost/time modifiers. Base = 1.0 for all fields.
/// Techs push multiplier modifiers (e.g. -0.15 for "15% cheaper ships").
/// Effective cost = base_cost * modifier.final_value().
#[derive(Resource, Component)]
pub struct ConstructionParams {
    pub ship_cost_modifier: ModifiedValue,
    pub building_cost_modifier: ModifiedValue,
    pub ship_build_time_modifier: ModifiedValue,
    pub building_build_time_modifier: ModifiedValue,
}

impl Default for ConstructionParams {
    fn default() -> Self {
        Self {
            ship_cost_modifier: ModifiedValue::new(Amt::units(1)),
            building_cost_modifier: ModifiedValue::new(Amt::units(1)),
            ship_build_time_modifier: ModifiedValue::new(Amt::units(1)),
            building_build_time_modifier: ModifiedValue::new(Amt::units(1)),
        }
    }
}

/// Parse building definitions from Lua accumulators into the BuildingRegistry.
/// Scripts are loaded by `load_all_scripts`; this system only parses the results.
pub fn load_building_registry(
    engine: Res<crate::scripting::ScriptEngine>,
    mut registry: ResMut<BuildingRegistry>,
) {
    match parse_building_definitions(engine.lua()) {
        Ok(defs) => {
            let count = defs.len();
            for def in defs {
                registry.insert(def);
            }
            info!("Building registry loaded with {} definitions", count);
        }
        Err(e) => {
            warn!("Failed to parse building definitions: {e}; building registry will be empty");
        }
    }
}

/// Remove expired timed modifiers from all ModifiedValue-containing components.
/// Runs BEFORE sync_building_modifiers so that expired timed effects are cleaned
/// up before production values are recalculated.
pub fn tick_timed_effects(
    clock: Res<crate::time_system::GameClock>,
    mut productions: Query<(Entity, &mut Production)>,
    mut maintenance_costs: Query<(Entity, &mut MaintenanceCost)>,
    mut food_consumptions: Query<(Entity, &mut FoodConsumption)>,
    mut empire_q: Query<(&mut AuthorityParams, &mut ConstructionParams), With<crate::player::PlayerEmpire>>,
    mut event_system: ResMut<crate::event_system::EventSystem>,
) {
    let Ok((mut authority_params, mut construction_params)) = empire_q.single_mut() else {
        return;
    };
    let now = clock.elapsed;

    // Helper: drain expired modifiers and fire any on_expire_event via EventSystem
    fn drain_and_fire(
        mv: &mut ModifiedValue,
        now: i64,
        target: Option<Entity>,
        event_system: &mut crate::event_system::EventSystem,
    ) {
        let expired = mv.drain_expired(now);
        for m in &expired {
            if let Some(ref evt) = m.on_expire_event {
                info!(
                    "Modifier '{}' expired, triggering event: {}",
                    m.id, evt
                );
                event_system.fire_event(evt, target, now);
            }
        }
    }

    for (entity, mut prod) in &mut productions {
        drain_and_fire(&mut prod.minerals_per_hexadies, now, Some(entity), &mut event_system);
        drain_and_fire(&mut prod.energy_per_hexadies, now, Some(entity), &mut event_system);
        drain_and_fire(&mut prod.research_per_hexadies, now, Some(entity), &mut event_system);
        drain_and_fire(&mut prod.food_per_hexadies, now, Some(entity), &mut event_system);
    }
    for (entity, mut mc) in &mut maintenance_costs {
        drain_and_fire(&mut mc.energy_per_hexadies, now, Some(entity), &mut event_system);
    }
    for (entity, mut fc) in &mut food_consumptions {
        drain_and_fire(&mut fc.food_per_hexadies, now, Some(entity), &mut event_system);
    }
    drain_and_fire(&mut authority_params.production, now, None, &mut event_system);
    drain_and_fire(&mut authority_params.cost_per_colony, now, None, &mut event_system);
    drain_and_fire(&mut construction_params.ship_cost_modifier, now, None, &mut event_system);
    drain_and_fire(&mut construction_params.building_cost_modifier, now, None, &mut event_system);
    drain_and_fire(&mut construction_params.ship_build_time_modifier, now, None, &mut event_system);
    drain_and_fire(&mut construction_params.building_build_time_modifier, now, None, &mut event_system);
}

/// Tracks cooldowns for resource alerts to prevent spamming the same alert every tick.
#[derive(Resource, Default)]
pub struct AlertCooldowns {
    cooldowns: std::collections::HashMap<(String, Entity), i64>,
}

impl AlertCooldowns {
    /// Minimum hexadies between repeated alerts of the same type for the same system.
    const COOLDOWN: i64 = 30;

    pub fn can_alert(&self, alert_type: &str, system: Entity, now: i64) -> bool {
        match self.cooldowns.get(&(alert_type.to_string(), system)) {
            Some(last) => now - last >= Self::COOLDOWN,
            None => true,
        }
    }

    pub fn mark(&mut self, alert_type: &str, system: Entity, now: i64) {
        self.cooldowns.insert((alert_type.to_string(), system), now);
    }
}

/// Checks colonies for resource depletion and emits `ResourceAlert` events.
/// Runs after maintenance/growth so stockpiles are up to date.
#[allow(clippy::too_many_arguments)]
pub fn check_resource_alerts(
    clock: Res<crate::time_system::GameClock>,
    last_tick: Res<LastProductionTick>,
    colonies: Query<(
        &Colony,
        Option<&FoodConsumption>,
        Option<&MaintenanceCost>,
    )>,
    stockpiles: Query<&ResourceStockpile, With<crate::galaxy::StarSystem>>,
    stars: Query<&crate::galaxy::StarSystem>,
    planets: Query<&crate::galaxy::Planet>,
    mut events: MessageWriter<crate::events::GameEvent>,
    mut alert_cooldowns: ResMut<AlertCooldowns>,
    mut next_event_id: ResMut<crate::knowledge::NextEventId>,
) {
    let delta = clock.elapsed - last_tick.0;
    if delta <= 0 {
        return;
    }

    for (colony, food_consumption, _maintenance) in &colonies {
        let colony_sys = colony.system(&planets);
        let system_name = colony_sys
            .and_then(|sys| stars.get(sys).ok())
            .map(|s| s.name.clone())
            .unwrap_or_default();
        let Some(sys) = colony_sys else { continue };
        let Ok(stockpile) = stockpiles.get(sys) else { continue };
        // Use planet entity as alert key (unique per colony)
        let alert_key = colony.planet;

        // Food starvation alert: food == 0
        if stockpile.food == Amt::ZERO {
            if alert_cooldowns.can_alert("food_starving", alert_key, clock.elapsed) {
                events.write(crate::events::GameEvent {
                    id: next_event_id.allocate(),
                    timestamp: clock.elapsed,
                    kind: crate::events::GameEventKind::ResourceAlert,
                    description: format!("{}: Starvation! Food depleted", system_name),
                    related_system: colony_sys,
                });
                alert_cooldowns.mark("food_starving", alert_key, clock.elapsed);
            }
        }

        // Food low alert: food < food_consumption * 10 (less than 10 hexadies of food)
        if let Some(fc) = food_consumption {
            let threshold = fc.food_per_hexadies.final_value().mul_u64(10);
            if stockpile.food < threshold && stockpile.food > Amt::ZERO {
                if alert_cooldowns.can_alert("food_low", alert_key, clock.elapsed) {
                    events.write(crate::events::GameEvent {
                        id: next_event_id.allocate(),
                        timestamp: clock.elapsed,
                        kind: crate::events::GameEventKind::ResourceAlert,
                        description: format!(
                            "{}: Food supply low ({} remaining)",
                            system_name, stockpile.food
                        ),
                        related_system: colony_sys,
                    });
                    alert_cooldowns.mark("food_low", alert_key, clock.elapsed);
                }
            }
        }

        // Energy depleted alert
        if stockpile.energy == Amt::ZERO {
            if alert_cooldowns.can_alert("energy_depleted", alert_key, clock.elapsed) {
                events.write(crate::events::GameEvent {
                    id: next_event_id.allocate(),
                    timestamp: clock.elapsed,
                    kind: crate::events::GameEventKind::ResourceAlert,
                    description: format!(
                        "{}: Energy depleted! Maintenance unpaid",
                        system_name
                    ),
                    related_system: colony_sys,
                });
                alert_cooldowns.mark("energy_depleted", alert_key, clock.elapsed);
            }
        }
    }
}

pub fn advance_production_tick(clock: Res<crate::time_system::GameClock>, mut last_tick: ResMut<LastProductionTick>) {
    last_tick.0 = clock.elapsed;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maintenance_negative_energy_capped_at_zero() {
        let mut energy = Amt::units(2);
        let total_maintenance = Amt::units(1);
        let delta = Amt::units(5);

        // total_maintenance * delta = 5, energy = 2, saturating sub => 0
        energy = energy.sub(total_maintenance.mul_amt(delta));
        assert_eq!(energy, Amt::ZERO);
    }

    #[test]
    fn food_consumption_by_population() {
        // population=100, food=100, 1 hexadies: consumes 100*0.1*1 = 10 food
        let population: f64 = 100.0;
        let mut food: f64 = 100.0;
        let delta: f64 = 1.0;
        food -= population * 0.1 * delta;
        assert!((food - 90.0).abs() < 1e-10);
    }

    #[test]
    fn starvation_reduces_population() {
        // population=100, food=0, 1 hexadies: loses 100*0.01*1 = 1 pop
        let mut population: f64 = 100.0;
        let food: f64 = 0.0;
        let delta: f64 = 1.0;
        if food <= 0.0 {
            let loss = population * 0.01 * delta;
            population = (population - loss).max(1.0);
        }
        assert!((population - 99.0).abs() < 1e-10);
    }

    #[test]
    fn starvation_population_minimum() {
        // population should not drop below 1.0
        let mut population: f64 = 0.5;
        let food: f64 = 0.0;
        let delta: f64 = 1.0;
        if food <= 0.0 {
            let loss = population * 0.01 * delta;
            population = (population - loss).max(1.0);
        }
        assert_eq!(population, 1.0);
    }
}
