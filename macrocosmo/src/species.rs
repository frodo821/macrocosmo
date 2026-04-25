use std::collections::HashMap;

use bevy::prelude::*;

use crate::modifier::ParsedModifier;

// ---------------------------------------------------------------------------
// Species definitions
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, bevy::reflect::Reflect)]
pub struct SpeciesDefinition {
    pub id: String,
    pub name: String,
    pub description: String,
    pub base_growth_rate: f64,
    /// Raw modifiers declared in Lua. Targets are routed at runtime by
    /// `sync_species_modifiers`: job-scoped (`job:<id>::...`) go into per-job
    /// buckets, otherwise to the colony aggregator. See #241.
    pub modifiers: Vec<ParsedModifier>,
}

#[derive(Resource, Default, Reflect)]
#[reflect(Resource)]
pub struct SpeciesRegistry {
    pub species: HashMap<String, SpeciesDefinition>,
}

impl SpeciesRegistry {
    pub fn get(&self, id: &str) -> Option<&SpeciesDefinition> {
        self.species.get(id)
    }

    pub fn insert(&mut self, def: SpeciesDefinition) {
        self.species.insert(def.id.clone(), def);
    }
}

// ---------------------------------------------------------------------------
// Colony population
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, bevy::reflect::Reflect)]
pub struct ColonySpecies {
    pub species_id: String,
    pub population: u32,
}

#[derive(Component, Default, Reflect)]
#[reflect(Component)]
pub struct ColonyPopulation {
    pub species: Vec<ColonySpecies>,
    /// Sub-integer growth accumulated over ticks. When |accumulator| >= 1.0,
    /// one individual is added/removed from a species chosen by growth-rate
    /// weighted random selection.
    pub growth_accumulator: f64,
}

impl ColonyPopulation {
    pub fn total(&self) -> u32 {
        self.species.iter().map(|s| s.population).sum()
    }

    pub fn species_ratio(&self, species_id: &str) -> f64 {
        let total = self.total();
        if total == 0 {
            return 0.0;
        }
        let count = self
            .species
            .iter()
            .find(|s| s.species_id == species_id)
            .map(|s| s.population)
            .unwrap_or(0);
        count as f64 / total as f64
    }
}

// ---------------------------------------------------------------------------
// Job definitions
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, bevy::reflect::Reflect)]
pub struct JobDefinition {
    pub id: String,
    pub label: String,
    pub description: String,
    /// Per-pop rate modifiers declared in Lua. Prefix-less `colony.<x>` targets
    /// are auto-prefixed to `job:<self_id>::colony.<x>` at parse time. See #241.
    pub modifiers: Vec<ParsedModifier>,
}

impl JobDefinition {
    /// Targets this job declares per-pop output for (i.e. every `job:<id>::<target>`
    /// modifier it carries). Used by `tick_production` to know which colony
    /// aggregators should receive this job's contribution.
    #[allow(dead_code)]
    pub fn declared_targets(&self) -> Vec<&str> {
        let mut out: Vec<&str> = self
            .modifiers
            .iter()
            .filter_map(|m| m.job_scope())
            .map(|(_, t)| t)
            .collect();
        out.sort();
        out.dedup();
        out
    }
}

#[derive(Resource, Default, Reflect)]
#[reflect(Resource)]
pub struct JobRegistry {
    pub jobs: HashMap<String, JobDefinition>,
}

impl JobRegistry {
    pub fn get(&self, id: &str) -> Option<&JobDefinition> {
        self.jobs.get(id)
    }

    pub fn insert(&mut self, def: JobDefinition) {
        self.jobs.insert(def.id.clone(), def);
    }
}

// ---------------------------------------------------------------------------
// Colony jobs
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, Default, bevy::reflect::Reflect)]
pub struct JobSlot {
    pub job_id: String,
    pub capacity: u32,
    pub assigned: u32,
    /// #241: Portion of `capacity` sourced from building `colony.<job>_slot`
    /// modifiers. `sync_building_modifiers` resets this each tick; the rest
    /// (`capacity - capacity_from_buildings`) is treated as an externally
    /// configured fixed capacity (tests, events, etc.).
    #[doc(hidden)]
    pub capacity_from_buildings: u32,
}

impl JobSlot {
    /// Create a slot with externally-configured fixed capacity. Building sync
    /// will *add* to this, never reduce it below `capacity`.
    #[allow(dead_code)]
    pub fn fixed(job_id: impl Into<String>, capacity: u32) -> Self {
        Self {
            job_id: job_id.into(),
            capacity,
            assigned: 0,
            capacity_from_buildings: 0,
        }
    }
}

#[derive(Component, Default, Reflect)]
#[reflect(Component)]
pub struct ColonyJobs {
    pub slots: Vec<JobSlot>,
}

impl ColonyJobs {
    pub fn total_employed(&self) -> u32 {
        self.slots.iter().map(|s| s.assigned).sum()
    }

    pub fn total_capacity(&self) -> u32 {
        self.slots.iter().map(|s| s.capacity).sum()
    }
}

// ---------------------------------------------------------------------------
// Auto-assignment system
// ---------------------------------------------------------------------------

/// Synchronise job assignments with population: fill slots top-down when there
/// are unemployed pops, and trim from the bottom when population drops below
/// total employed.
pub fn sync_job_assignment(mut colonies: Query<(&ColonyPopulation, &mut ColonyJobs)>) {
    for (pop, mut jobs) in &mut colonies {
        let total_pop = pop.total();
        let mut remaining = total_pop;

        for slot in &mut jobs.slots {
            let assign = remaining.min(slot.capacity);
            slot.assigned = assign;
            remaining = remaining.saturating_sub(assign);
        }
    }
}

// ---------------------------------------------------------------------------
// Species plugin (for registration convenience)
// ---------------------------------------------------------------------------

pub struct SpeciesPlugin;

impl Plugin for SpeciesPlugin {
    fn build(&self, app: &mut App) {
        // Note: `sync_job_assignment` is scheduled by ColonyPlugin so it runs
        // between `sync_building_modifiers` (which sets slot capacities) and
        // `tick_production` (which reads `slot.assigned`). See #241.
        app.init_resource::<SpeciesRegistry>()
            .init_resource::<JobRegistry>()
            .add_systems(
                Startup,
                load_species_and_jobs.after(crate::scripting::load_all_scripts),
            );
    }
}

/// Parse species and job definitions from Lua accumulators.
/// Scripts are loaded by `load_all_scripts`; this system only parses the results.
pub fn load_species_and_jobs(
    engine: Res<crate::scripting::ScriptEngine>,
    mut species_registry: ResMut<SpeciesRegistry>,
    mut job_registry: ResMut<JobRegistry>,
) {
    use crate::scripting::species_api::{parse_job_definitions, parse_species_definitions};

    match parse_species_definitions(engine.lua()) {
        Ok(defs) => {
            let count = defs.len();
            for def in defs {
                species_registry.insert(def);
            }
            info!("Species registry loaded with {} definitions", count);
        }
        Err(e) => {
            warn!("Failed to parse species definitions: {e}; species registry will be empty");
        }
    }

    match parse_job_definitions(engine.lua()) {
        Ok(defs) => {
            let count = defs.len();
            for def in defs {
                job_registry.insert(def);
            }
            info!("Job registry loaded with {} definitions", count);
        }
        Err(e) => {
            warn!("Failed to parse job definitions: {e}; job registry will be empty");
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_colony_population_total() {
        let pop = ColonyPopulation {
            species: vec![
                ColonySpecies {
                    species_id: "human".to_string(),
                    population: 80,
                },
                ColonySpecies {
                    species_id: "alien".to_string(),
                    population: 20,
                },
            ],
            growth_accumulator: 0.0,
        };
        assert_eq!(pop.total(), 100);
    }

    #[test]
    fn test_colony_population_total_empty() {
        let pop = ColonyPopulation::default();
        assert_eq!(pop.total(), 0);
    }

    #[test]
    fn test_colony_population_species_ratio() {
        let pop = ColonyPopulation {
            species: vec![
                ColonySpecies {
                    species_id: "human".to_string(),
                    population: 75,
                },
                ColonySpecies {
                    species_id: "alien".to_string(),
                    population: 25,
                },
            ],
            growth_accumulator: 0.0,
        };
        assert!((pop.species_ratio("human") - 0.75).abs() < 1e-10);
        assert!((pop.species_ratio("alien") - 0.25).abs() < 1e-10);
        assert!((pop.species_ratio("unknown") - 0.0).abs() < 1e-10);
    }

    #[test]
    fn test_colony_population_species_ratio_empty() {
        let pop = ColonyPopulation::default();
        assert!((pop.species_ratio("human") - 0.0).abs() < 1e-10);
    }

    #[test]
    fn test_colony_jobs_assignment() {
        let jobs = ColonyJobs {
            slots: vec![
                JobSlot {
                    job_id: "miner".to_string(),
                    capacity: 5,
                    assigned: 5,
                    capacity_from_buildings: 0,
                },
                JobSlot {
                    job_id: "farmer".to_string(),
                    capacity: 5,
                    assigned: 3,
                    capacity_from_buildings: 0,
                },
            ],
        };
        assert_eq!(jobs.total_employed(), 8);
        assert_eq!(jobs.total_capacity(), 10);
    }

    #[test]
    fn test_colony_jobs_unemployed() {
        let pop = ColonyPopulation {
            species: vec![ColonySpecies {
                species_id: "human".to_string(),
                population: 12,
            }],
            growth_accumulator: 0.0,
        };
        let jobs = ColonyJobs {
            slots: vec![
                JobSlot {
                    job_id: "miner".to_string(),
                    capacity: 5,
                    assigned: 5,
                    capacity_from_buildings: 0,
                },
                JobSlot {
                    job_id: "farmer".to_string(),
                    capacity: 5,
                    assigned: 5,
                    capacity_from_buildings: 0,
                },
            ],
        };
        let unemployed = pop.total() - jobs.total_employed();
        assert_eq!(unemployed, 2);
    }

    #[test]
    fn test_species_registry() {
        let mut registry = SpeciesRegistry::default();
        assert!(registry.get("human").is_none());

        registry.insert(SpeciesDefinition {
            id: "human".to_string(),
            name: "Human".to_string(),
            description: String::new(),
            base_growth_rate: 0.01,
            modifiers: Vec::new(),
        });

        let human = registry.get("human").unwrap();
        assert_eq!(human.name, "Human");
        assert!((human.base_growth_rate - 0.01).abs() < 1e-10);
    }

    #[test]
    fn test_job_registry() {
        let mut registry = JobRegistry::default();
        assert!(registry.get("miner").is_none());

        registry.insert(JobDefinition {
            id: "miner".to_string(),
            label: "Miner".to_string(),
            description: String::new(),
            modifiers: vec![ParsedModifier {
                target: "job:miner::colony.minerals_per_hexadies".to_string(),
                base_add: 0.6,
                multiplier: 0.0,
                add: 0.0,
            }],
        });

        let miner = registry.get("miner").unwrap();
        assert_eq!(miner.label, "Miner");
        let targets = miner.declared_targets();
        assert_eq!(targets, vec!["colony.minerals_per_hexadies"]);
    }
}
