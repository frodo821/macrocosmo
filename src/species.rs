use std::collections::HashMap;

use bevy::prelude::*;

use crate::amount::Amt;
use crate::modifier::ModifiedValue;

// ---------------------------------------------------------------------------
// Species definitions
// ---------------------------------------------------------------------------

#[derive(Clone, Debug)]
pub struct SpeciesDefinition {
    pub id: String,
    pub name: String,
    pub base_growth_rate: f64,
    /// job_id -> bonus ModifiedValue (base=1.0 + bonus, modifiers for techs etc.)
    pub job_bonuses: HashMap<String, ModifiedValue>,
}

#[derive(Resource, Default)]
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

#[derive(Clone, Debug)]
pub struct ColonySpecies {
    pub species_id: String,
    pub population: u32,
}

#[derive(Component, Default)]
pub struct ColonyPopulation {
    pub species: Vec<ColonySpecies>,
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

#[derive(Clone, Debug)]
pub struct JobDefinition {
    pub id: String,
    pub label: String,
    /// resource_type -> amount per pop per hexady
    pub base_output: HashMap<String, Amt>,
}

#[derive(Resource, Default)]
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

#[derive(Clone, Debug)]
pub struct JobSlot {
    pub job_id: String,
    pub capacity: u32,
    pub assigned: u32,
}

#[derive(Component, Default)]
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
        app.init_resource::<SpeciesRegistry>()
            .init_resource::<JobRegistry>()
            .add_systems(
                Startup,
                load_species_and_jobs.after(crate::scripting::init_scripting),
            )
            .add_systems(
                Update,
                sync_job_assignment.after(crate::time_system::advance_game_time),
            );
    }
}

/// Startup system that loads species and job definitions from Lua scripts.
pub fn load_species_and_jobs(
    engine: Res<crate::scripting::ScriptEngine>,
    mut species_registry: ResMut<SpeciesRegistry>,
    mut job_registry: ResMut<JobRegistry>,
) {
    use crate::scripting::species_api::{parse_job_definitions, parse_species_definitions};
    use std::path::Path;

    // Load species scripts
    let species_dir = Path::new("scripts/species");
    if species_dir.exists() {
        match engine.load_directory(species_dir) {
            Err(e) => {
                warn!("Failed to load species scripts: {e}; species registry will be empty");
            }
            Ok(()) => match parse_species_definitions(engine.lua()) {
                Ok(defs) => {
                    let count = defs.len();
                    for def in defs {
                        species_registry.insert(def);
                    }
                    info!("Species registry loaded with {} definitions", count);
                }
                Err(e) => {
                    warn!(
                        "Failed to parse species definitions: {e}; species registry will be empty"
                    );
                }
            },
        }
    } else {
        info!("scripts/species directory not found; species registry will be empty");
    }

    // Load job scripts
    let jobs_dir = Path::new("scripts/jobs");
    if jobs_dir.exists() {
        match engine.load_directory(jobs_dir) {
            Err(e) => {
                warn!("Failed to load job scripts: {e}; job registry will be empty");
            }
            Ok(()) => match parse_job_definitions(engine.lua()) {
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
            },
        }
    } else {
        info!("scripts/jobs directory not found; job registry will be empty");
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
                },
                JobSlot {
                    job_id: "farmer".to_string(),
                    capacity: 5,
                    assigned: 3,
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
        };
        let jobs = ColonyJobs {
            slots: vec![
                JobSlot {
                    job_id: "miner".to_string(),
                    capacity: 5,
                    assigned: 5,
                },
                JobSlot {
                    job_id: "farmer".to_string(),
                    capacity: 5,
                    assigned: 5,
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
            base_growth_rate: 0.01,
            job_bonuses: HashMap::new(),
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
            base_output: {
                let mut m = HashMap::new();
                m.insert("minerals".to_string(), Amt::new(0, 600));
                m
            },
        });

        let miner = registry.get("miner").unwrap();
        assert_eq!(miner.label, "Miner");
        assert_eq!(
            miner.base_output.get("minerals"),
            Some(&Amt::new(0, 600))
        );
    }
}
