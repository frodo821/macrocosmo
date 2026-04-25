use bevy::prelude::*;

use crate::amount::{Amt, SignedAmt};
use crate::galaxy::{Planet, StarSystem};
use crate::modifier::{ModifiedValue, Modifier, ParsedModifier};
use crate::scripting::building_api::BuildingRegistry;
use crate::species::{ColonyJobs, JobRegistry, SpeciesRegistry};
use crate::time_system::GameClock;

use super::{
    AUTHORITY_DEFICIT_PENALTY, Buildings, Colony, LastProductionTick, ResourceCapacity,
    ResourceStockpile,
};

/// #29: Production focus weights for colony output
#[derive(Component, Reflect)]
#[reflect(Component)]
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
#[derive(Component, Reflect)]
#[reflect(Component)]
pub struct Production {
    pub minerals_per_hexadies: ModifiedValue,
    pub energy_per_hexadies: ModifiedValue,
    pub research_per_hexadies: ModifiedValue,
    pub food_per_hexadies: ModifiedValue,
}

/// #241: Per-job per-target rate buckets for a colony.
///
/// Keyed by `(job_id, target)` where `target` is e.g. `colony.minerals_per_hexadies`.
/// Each bucket holds a `ModifiedValue` whose `final_value()` is the **per-pop**
/// rate for that job → target combination. `tick_production` multiplies this by
/// the job's `assigned` count and pushes the result into the colony aggregator.
///
/// Modifiers pushed into these buckets come from:
/// - The job's own declaration (`define_job { modifiers = ... }`, `base_add`)
/// - Species modifiers with `target = "job:<id>::..."`
/// - Tech / event effects with `target = "job:<id>::..."`
#[derive(Component, Default, Debug, Reflect)]
#[reflect(Component)]
pub struct ColonyJobRates {
    buckets: std::collections::HashMap<(String, String), ModifiedValue>,
}

impl ColonyJobRates {
    pub fn get(&self, job_id: &str, target: &str) -> Option<&ModifiedValue> {
        self.buckets.get(&(job_id.to_string(), target.to_string()))
    }

    pub fn bucket_mut(&mut self, job_id: &str, target: &str) -> &mut ModifiedValue {
        self.buckets
            .entry((job_id.to_string(), target.to_string()))
            .or_default()
    }

    pub fn is_empty(&self) -> bool {
        self.buckets.is_empty()
    }

    pub fn iter(&self) -> impl Iterator<Item = (&String, &String, &ModifiedValue)> {
        self.buckets.iter().map(|((j, t), v)| (j, t, v))
    }

    /// Clear all per-job buckets (used before re-syncing from sources).
    pub fn clear(&mut self) {
        self.buckets.clear();
    }
}

/// Return the mutable ModifiedValue on `prod` matching a `colony.<resource>_per_hexadies`
/// target, or None if the target isn't a known colony aggregator.
fn colony_resource_bucket<'a>(
    prod: &'a mut Production,
    target: &str,
) -> Option<&'a mut ModifiedValue> {
    match target {
        "colony.minerals_per_hexadies" => Some(&mut prod.minerals_per_hexadies),
        "colony.energy_per_hexadies" => Some(&mut prod.energy_per_hexadies),
        "colony.research_per_hexadies" => Some(&mut prod.research_per_hexadies),
        "colony.food_per_hexadies" => Some(&mut prod.food_per_hexadies),
        _ => None,
    }
}

/// Extract the job id from a `colony.<job>_slot` target string.
fn slot_target_to_job(target: &str) -> Option<&str> {
    let rest = target.strip_prefix("colony.")?;
    rest.strip_suffix("_slot")
}

/// #241: Route a single ParsedModifier from a building into the correct
/// `Production` bucket or `ColonyJobRates` per-job bucket.
fn apply_building_modifier(
    source_id: &str,
    label: &str,
    pm: &ParsedModifier,
    prod: &mut Production,
    job_rates: &mut ColonyJobRates,
    job_slot_caps: &mut std::collections::HashMap<String, u32>,
    job_registry: &JobRegistry,
) {
    // Job slot capacity: `colony.<job>_slot`.
    if let Some(job_id) = slot_target_to_job(&pm.target) {
        // Slot capacity modifiers only use `base_add` and `add` — treat them
        // as whole-unit counts. Fractional/negative capacities are clamped to 0.
        let contribution = pm.base_add + pm.add;
        if contribution <= 0.0 {
            return;
        }
        if job_registry.get(job_id).is_none() {
            // Unknown job — warn once per source (we don't track dedupe here;
            // worst case is a handful of warnings per startup).
            warn!(
                "Building modifier from '{}' targets slot '{}' for unknown job '{}'; ignored",
                source_id, pm.target, job_id
            );
            return;
        }
        let entry = job_slot_caps.entry(job_id.to_string()).or_insert(0);
        *entry = entry.saturating_add(contribution.floor() as u32);
        return;
    }

    // Per-job rate bucket: `job:<id>::<target>`.
    if let Some((job_id, inner_target)) = pm.job_scope() {
        if job_registry.get(job_id).is_none() {
            warn!(
                "Building modifier from '{}' targets unknown job '{}' (target '{}'); ignored",
                source_id, job_id, pm.target
            );
            return;
        }
        let bucket = job_rates.bucket_mut(job_id, inner_target);
        bucket.push_modifier(pm.to_modifier(
            format!("bldg:{}:{}", source_id, pm.target),
            label.to_string(),
        ));
        return;
    }

    // Colony aggregator: `colony.<X>_per_hexadies`.
    if let Some(bucket) = colony_resource_bucket(prod, &pm.target) {
        bucket.push_modifier(pm.to_modifier(
            format!("bldg:{}:{}", source_id, pm.target),
            label.to_string(),
        ));
        return;
    }

    debug!(
        "Building modifier from '{}' has unknown target '{}'; ignored",
        source_id, pm.target
    );
}

/// #241: Remove all previously pushed building-sourced modifiers from the
/// Production aggregators. Called at the start of `sync_building_modifiers`
/// each tick so that demolitions and empty slots propagate. Building-sourced
/// modifier ids all begin with `bldg:`.
fn clear_building_mods(mv: &mut ModifiedValue) {
    let to_remove: Vec<String> = mv
        .modifiers()
        .iter()
        .filter(|m| m.id.starts_with("bldg:"))
        .map(|m| m.id.clone())
        .collect();
    for id in to_remove {
        mv.pop_modifier(&id);
    }
}

/// Synchronise building-declared modifiers onto the Production component,
/// per-job rate buckets (`ColonyJobRates`), and job slot capacities on
/// `ColonyJobs`. Must run BEFORE `tick_production`.
///
/// Walks both planet-level (`Buildings`) and system-level (`SystemBuildings`)
/// buildings. For each building, walks its `modifiers` Vec and dispatches:
/// - `colony.<job>_slot` → `ColonyJobs.slots[job].capacity`
/// - `job:<id>::<target>` → `ColonyJobRates[(id, target)]`
/// - `colony.<X>_per_hexadies` → `Production.<X>_per_hexadies`
pub fn sync_building_modifiers(
    registry: Res<BuildingRegistry>,
    job_registry: Res<JobRegistry>,
    planets: Query<&Planet>,
    system_buildings: Query<(Entity, &super::SystemBuildings)>,
    station_ships: Query<(
        Entity,
        &crate::ship::Ship,
        &crate::ship::ShipState,
        &super::SlotAssignment,
    )>,
    mut colonies: Query<(
        &Colony,
        &Buildings,
        &mut Production,
        Option<&mut ColonyJobRates>,
        Option<&mut ColonyJobs>,
    )>,
) {
    // Build system → max_slots map for slot-based systems.
    let sys_entities: std::collections::HashMap<Entity, usize> = system_buildings
        .iter()
        .map(|(e, sb)| (e, sb.max_slots))
        .collect();

    // Throwaway rates bucket used when the colony lacks a ColonyJobRates
    // component (slot-bearing buildings become no-ops, `rates.clear()` resets).
    let mut scratch_rates = ColonyJobRates::default();

    for (colony, buildings, mut prod, mut rates_opt, mut jobs_opt) in &mut colonies {
        let rates: &mut ColonyJobRates = match &mut rates_opt {
            Some(r) => &mut *r,
            None => {
                scratch_rates.clear();
                &mut scratch_rates
            }
        };
        // 1. Clear previously-synced building modifiers so changes propagate.
        clear_building_mods(&mut prod.minerals_per_hexadies);
        clear_building_mods(&mut prod.energy_per_hexadies);
        clear_building_mods(&mut prod.research_per_hexadies);
        clear_building_mods(&mut prod.food_per_hexadies);
        rates.clear();

        // 2. Start with zero capacity on every known job slot; fill from building
        //    modifiers below. Jobs the colony doesn't have yet get appended.
        let mut caps: std::collections::HashMap<String, u32> = std::collections::HashMap::new();

        // 3. Walk planet-level buildings in this colony.
        // #438: Include slot index in source_id so duplicate building types
        // generate unique modifier IDs (otherwise push_modifier deduplicates).
        for (slot_idx, slot) in buildings.slots.iter().enumerate() {
            if let Some(bid) = slot {
                let Some(def) = registry.get(bid.as_str()) else {
                    warn!("Building '{}' not found in registry", bid);
                    continue;
                };
                let source = format!("{}[{}]", def.id, slot_idx);
                for pm in &def.modifiers {
                    apply_building_modifier(
                        &source,
                        &def.name,
                        pm,
                        &mut prod,
                        &mut *rates,
                        &mut caps,
                        &job_registry,
                    );
                }
            }
        }

        // 4. Walk system-level buildings belonging to this colony's system.
        //    Now queries station ships with SlotAssignment instead of
        //    SystemBuildings.slots.
        if let Some(sys_entity) = colony.system(&planets) {
            if sys_entities.contains_key(&sys_entity) {
                let reverse = super::system_buildings::build_reverse_design_map(&registry);
                for (ship_entity, ship, state, _slot) in &station_ships {
                    let in_system = match state {
                        crate::ship::ShipState::InSystem { system: s } => *s == sys_entity,
                        crate::ship::ShipState::Refitting { system: s, .. } => *s == sys_entity,
                        _ => false,
                    };
                    if !in_system {
                        continue;
                    }
                    if let Some(bid) = reverse.get(&ship.design_id) {
                        let Some(def) = registry.get(bid.as_str()) else {
                            continue;
                        };
                        // #438: Include ship entity index for unique modifier IDs.
                        let source = format!("{}[{:?}]", def.id, ship_entity);
                        for pm in &def.modifiers {
                            apply_building_modifier(
                                &source,
                                &def.name,
                                pm,
                                &mut prod,
                                &mut *rates,
                                &mut caps,
                                &job_registry,
                            );
                        }
                    }
                }
            }
        }

        // 5. Reconcile `ColonyJobs.slots[*].capacity` with `caps`. Buildings
        //    "own" a portion of each slot's capacity (tracked via
        //    `capacity_from_buildings`). We recompute that portion from
        //    `caps` and leave any additional externally-set capacity (tests,
        //    events) in place.
        if let Some(ref mut jobs) = jobs_opt {
            for slot in jobs.slots.iter_mut() {
                let fixed = slot.capacity.saturating_sub(slot.capacity_from_buildings);
                let new_bldg_cap = caps.remove(&slot.job_id).unwrap_or(0);
                slot.capacity_from_buildings = new_bldg_cap;
                slot.capacity = fixed.saturating_add(new_bldg_cap);
                if slot.assigned > slot.capacity {
                    slot.assigned = slot.capacity;
                }
            }
            // Append fresh slots for jobs that now have building-sourced
            // capacity but had no entry.
            for (job_id, capacity) in caps {
                if capacity > 0 {
                    jobs.slots.push(crate::species::JobSlot {
                        job_id,
                        capacity,
                        assigned: 0,
                        capacity_from_buildings: capacity,
                    });
                }
            }
        }
    }
}

/// #241: Propagate species-level modifiers (`job:<id>::<target>` and plain
/// `colony.<x>`) and technology production multipliers into per-job rate buckets
/// and/or colony aggregators. Runs after `sync_building_modifiers` so the
/// "contribution" computed by `tick_production` sees species/tech bonuses.
///
/// For simplicity, this system walks every species defined in
/// `SpeciesRegistry` and pushes their modifiers into buckets with id
/// `species:<species_id>:<target>`. Since the fixture `ColonyPopulation`
/// tracks which species live here, only modifiers from present species are
/// applied.
pub fn sync_species_modifiers(
    species_registry: Res<SpeciesRegistry>,
    job_registry: Res<JobRegistry>,
    mut query: Query<(
        &crate::species::ColonyPopulation,
        &mut Production,
        Option<&mut ColonyJobRates>,
    )>,
) {
    let mut scratch = ColonyJobRates::default();
    for (pop, mut prod, mut rates_opt) in &mut query {
        let rates: &mut ColonyJobRates = match &mut rates_opt {
            Some(r) => &mut *r,
            None => {
                scratch.clear();
                &mut scratch
            }
        };

        // First, (re)populate per-job rate buckets with each job's own
        // declared modifiers. We do this here (instead of in
        // sync_building_modifiers) because buckets are cleared there and must
        // be rebuilt before tick_production runs.
        for (job_id, def) in &job_registry.jobs {
            for pm in &def.modifiers {
                let Some((declared_job, inner_target)) = pm.job_scope() else {
                    continue;
                };
                if declared_job != job_id {
                    continue;
                }
                let bucket = rates.bucket_mut(job_id, inner_target);
                bucket.push_modifier(pm.to_modifier(
                    format!("job:{}:base:{}", job_id, pm.target),
                    format!("Job '{}' base rate", def.label),
                ));
            }
        }
        // First, remove previously-synced species modifiers from colony aggregators
        // (species:* ids) so changes propagate.
        let clear = |mv: &mut ModifiedValue| {
            let to_remove: Vec<String> = mv
                .modifiers()
                .iter()
                .filter(|m| m.id.starts_with("species:"))
                .map(|m| m.id.clone())
                .collect();
            for id in to_remove {
                mv.pop_modifier(&id);
            }
        };
        clear(&mut prod.minerals_per_hexadies);
        clear(&mut prod.energy_per_hexadies);
        clear(&mut prod.research_per_hexadies);
        clear(&mut prod.food_per_hexadies);

        for sp in &pop.species {
            let Some(def) = species_registry.get(&sp.species_id) else {
                continue;
            };
            // Species with zero pop contribute nothing.
            if sp.population == 0 {
                continue;
            }
            for pm in &def.modifiers {
                if let Some((job_id, inner_target)) = pm.job_scope() {
                    if job_registry.get(job_id).is_none() {
                        warn!(
                            "Species '{}' targets unknown job '{}' (target '{}'); ignored",
                            def.id, job_id, pm.target
                        );
                        continue;
                    }
                    let bucket = rates.bucket_mut(job_id, inner_target);
                    bucket.push_modifier(pm.to_modifier(
                        format!("species:{}:{}", def.id, pm.target),
                        format!("Species '{}'", def.name),
                    ));
                } else if let Some(bucket) = colony_resource_bucket(&mut prod, &pm.target) {
                    bucket.push_modifier(pm.to_modifier(
                        format!("species:{}:{}", def.id, pm.target),
                        format!("Species '{}'", def.name),
                    ));
                } else {
                    debug!(
                        "Species '{}' has unknown modifier target '{}'; ignored",
                        def.id, pm.target
                    );
                }
            }
        }
    }
}

/// #250: Aggregate per-job production contributions into each colony's
/// `Production` component. Runs every tick (including while the clock is
/// paused) so the UI reads a correct rate even with `delta = 0`. Previously
/// this was Stage 1 inside `tick_production`, but that system early-returns
/// when `delta <= 0`, which meant the aggregator held only the legacy base
/// value during pauses and the first Update after Startup — leaving the UI
/// showing e.g. `Minerals: +5` instead of `base + miner contribution`.
///
/// Must run AFTER `sync_building_modifiers` (sets slot capacities),
/// `sync_job_assignment` (sets `slot.assigned`), and `sync_species_modifiers`
/// (populates per-job rate buckets) so the values it reads are current.
/// Must run BEFORE `tick_production` so the accumulator sees the fresh rate.
pub fn aggregate_job_contributions(
    mut colonies: Query<(
        &Colony,
        &mut Production,
        Option<&ColonyJobRates>,
        Option<&ColonyJobs>,
    )>,
) {
    for (_colony, mut prod, rates_opt, jobs_opt) in &mut colonies {
        // Remove any contributions from the previous aggregation first so
        // changes (pop shifts, slot changes, demolitions, etc.) propagate.
        let remove_job_mods = |mv: &mut ModifiedValue| {
            let ids: Vec<String> = mv
                .modifiers()
                .iter()
                .filter(|m| m.id.starts_with("job_"))
                .map(|m| m.id.clone())
                .collect();
            for id in ids {
                mv.pop_modifier(&id);
            }
        };
        remove_job_mods(&mut prod.minerals_per_hexadies);
        remove_job_mods(&mut prod.energy_per_hexadies);
        remove_job_mods(&mut prod.research_per_hexadies);
        remove_job_mods(&mut prod.food_per_hexadies);

        let (Some(rates), Some(jobs)) = (rates_opt, jobs_opt) else {
            continue;
        };

        // Sum each job's per-target contribution across assigned pops.
        // Target key -> accumulated contribution. One modifier per (job, target)
        // pair so the UI can show per-job breakdown later.
        let mut job_contribs: std::collections::HashMap<(String, String), f64> =
            std::collections::HashMap::new();
        for slot in &jobs.slots {
            if slot.assigned == 0 {
                continue;
            }
            for (job_id, target, mv) in rates.iter() {
                if job_id != &slot.job_id {
                    continue;
                }
                let rate = mv.final_value().to_f64();
                let contribution = rate * slot.assigned as f64;
                *job_contribs
                    .entry((job_id.clone(), target.clone()))
                    .or_insert(0.0) += contribution;
            }
        }

        for ((job_id, target), value) in &job_contribs {
            if let Some(bucket) = colony_resource_bucket(&mut prod, target) {
                bucket.push_modifier(Modifier {
                    id: format!("job_{}_contribution", job_id),
                    label: format!("Job '{}' contribution", job_id),
                    base_add: SignedAmt::from_f64(*value),
                    multiplier: SignedAmt::ZERO,
                    add: SignedAmt::ZERO,
                    expires_at: None,
                    on_expire_event: None,
                });
            }
        }
    }
}

/// #29: tick_production uses ProductionFocus weights and building bonuses.
/// #44: Research is no longer accumulated in the stockpile; emitted via emit_research.
/// #73: Non-capital colonies have production reduced when capital authority is depleted.
/// #241/#250: Production is a two-stage aggregation split across systems:
/// 1. `aggregate_job_contributions` pushes per-job contributions into each
///    colony's aggregators. Runs every Update (delta-independent) so the UI
///    sees a correct rate even while paused.
/// 2. This system multiplies `Production.<X>_per_hexadies.final_value()` by
///    the elapsed `delta` and applies the result to system stockpiles.
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
            colony
                .system(&planets)
                .filter(|&sys| stars.get(sys).ok().is_some_and(|s| s.is_capital))
        });
        capital_sys.and_then(|sys| stockpiles.get(sys).ok().map(|(s, _)| s.authority))
    };
    let authority_deficit = matches!(capital_authority, Some(a) if a == Amt::ZERO);

    // Collect production deltas per system
    let mut system_deltas: std::collections::HashMap<Entity, (Amt, Amt, Amt)> =
        std::collections::HashMap::new();
    for (colony, prod, focus) in &colonies {
        let Some(sys) = colony.system(&planets) else {
            continue;
        };

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

        let minerals = prod
            .minerals_per_hexadies
            .final_value()
            .mul_amt(mw)
            .mul_amt(d_amt)
            .mul_amt(authority_multiplier);
        let energy = prod
            .energy_per_hexadies
            .final_value()
            .mul_amt(ew)
            .mul_amt(d_amt)
            .mul_amt(authority_multiplier);
        let food = prod
            .food_per_hexadies
            .final_value()
            .mul_amt(d_amt)
            .mul_amt(authority_multiplier);

        let entry = system_deltas
            .entry(sys)
            .or_insert((Amt::ZERO, Amt::ZERO, Amt::ZERO));
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
