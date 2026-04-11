use bevy::prelude::*;
use rand::Rng;
use std::path::Path;

use crate::components::Position;
use crate::modifier::ScopedModifiers;
use crate::scripting::galaxy_api::{
    PlanetTypeDefinition, PlanetTypeRegistry, ResourceBias, StarTypeDefinition, StarTypeRegistry,
};
use crate::ship::Owner;
use crate::technology::TechKnowledge;

/// Galaxy configuration resource, inserted by generate_galaxy so other systems
/// (e.g. visualization) can reference galaxy parameters.
#[derive(Resource)]
pub struct GalaxyConfig {
    pub radius: f64,
    pub num_systems: usize,
}

pub struct GalaxyPlugin;

impl Plugin for GalaxyPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<StarTypeRegistry>()
            .init_resource::<PlanetTypeRegistry>()
            .add_systems(
                Startup,
                load_galaxy_types.after(crate::scripting::load_all_scripts),
            )
            .add_systems(Startup, generate_galaxy.after(load_galaxy_types));
    }
}

/// A star system in the galaxy
#[derive(Component)]
pub struct StarSystem {
    pub name: String,
    /// Whether this system has been surveyed (precise data available)
    pub surveyed: bool,
    /// Whether this system is the capital
    pub is_capital: bool,
    /// Star type id from Lua definitions (e.g. "yellow_dwarf")
    pub star_type: String,
}

/// A planet orbiting a star system.
#[derive(Component)]
pub struct Planet {
    pub name: String,
    /// The parent star system entity.
    pub system: Entity,
    /// Planet type id from Lua definitions (e.g. "terrestrial")
    pub planet_type: String,
}

/// Convert a 1-based index to a Roman numeral string (up to 12).
pub fn roman_numeral(n: usize) -> &'static str {
    match n {
        1 => "I",
        2 => "II",
        3 => "III",
        4 => "IV",
        5 => "V",
        6 => "VI",
        7 => "VII",
        8 => "VIII",
        9 => "IX",
        10 => "X",
        11 => "XI",
        12 => "XII",
        _ => "?",
    }
}

/// Physical and economic attributes of a star system.
#[derive(Component, Clone)]
pub struct SystemAttributes {
    pub habitability: Habitability,
    pub mineral_richness: ResourceLevel,
    pub energy_potential: ResourceLevel,
    pub research_potential: ResourceLevel,
    pub max_building_slots: u8,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Habitability {
    Ideal,
    Adequate,
    Marginal,
    Barren,
    GasGiant,
}

/// Maximum population that a colony can support at hab_score 1.0.
pub const BASE_CARRYING_CAPACITY: f64 = 200.0;
/// Food consumed per population per hexadies (as Amt: 0.100).
pub const FOOD_PER_POP_PER_HEXADIES: crate::amount::Amt = crate::amount::Amt::new(0, 100);

impl Habitability {
    /// Continuous habitability score in 0.0..=1.0.
    /// Used for carrying capacity and growth rate scaling.
    /// Technology bonuses can be added on top of this base value.
    pub fn base_score(&self) -> f64 {
        match self {
            Habitability::Ideal => 1.0,
            Habitability::Adequate => 0.7,
            Habitability::Marginal => 0.4,
            Habitability::Barren => 0.15,
            Habitability::GasGiant => 0.0,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ResourceLevel {
    Rich,
    Moderate,
    Poor,
    None,
}

/// Sovereignty status of a star system
#[derive(Component, Default)]
pub struct Sovereignty {
    pub owner: Option<Owner>,
    pub control_score: f64,
}

/// A hostile presence at a star system that player ships must fight.
#[derive(Component)]
pub struct HostilePresence {
    pub system: Entity,
    pub strength: f64,
    pub hp: f64,
    pub max_hp: f64,
    pub hostile_type: HostileType,
    pub evasion: f64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HostileType {
    SpaceCreature,
    AncientDefense,
}

/// Marker for systems obscured by interstellar gas
#[derive(Component)]
pub struct ObscuredByGas;

/// Marker for systems that have port facilities
#[derive(Component)]
pub struct PortFacility {
    /// The other star system entity this port connects to
    pub partner: Entity,
}

/// Persistent anomalies/points of interest discovered during surveys.
#[derive(Component, Default, Clone, Debug)]
pub struct Anomalies {
    pub discoveries: Vec<Anomaly>,
}

/// A single anomaly discovered during a survey.
#[derive(Clone, Debug)]
pub struct Anomaly {
    pub id: String,
    pub name: String,
    pub description: String,
    pub discovered_at: i64,
}

/// Modifiers that apply to all ships in a star system.
/// Example: solar storm reducing speed, nebula boosting shields.
#[derive(Component, Default)]
pub struct SystemModifiers {
    pub ship_speed: ScopedModifiers,
    pub ship_attack: ScopedModifiers,
    pub ship_defense: ScopedModifiers,
}

/// Sample from Poisson distribution using Knuth's algorithm.
/// Clamps result to [1, max].
pub fn poisson_sample(rng: &mut impl Rng, lambda: f64, max: usize) -> usize {
    let l = (-lambda).exp();
    let mut k: usize = 0;
    let mut p: f64 = 1.0;
    loop {
        k += 1;
        p *= rng.random::<f64>();
        if p <= l {
            break;
        }
    }
    (k - 1).max(1).min(max)
}

/// Convert a continuous habitability score to a Habitability enum value.
fn habitability_from_score(score: f64) -> Habitability {
    if score >= 0.8 {
        Habitability::Ideal
    } else if score >= 0.5 {
        Habitability::Adequate
    } else if score >= 0.2 {
        Habitability::Marginal
    } else if score > 0.0 {
        Habitability::Barren
    } else {
        Habitability::GasGiant
    }
}

/// Convert a resource bias value to a ResourceLevel using a random roll.
fn resource_level_from_bias(rng: &mut impl Rng, bias: f64) -> ResourceLevel {
    let roll: f64 = rng.random::<f64>() * bias;
    if roll > 0.8 {
        ResourceLevel::Rich
    } else if roll > 0.4 {
        ResourceLevel::Moderate
    } else if roll > 0.1 {
        ResourceLevel::Poor
    } else {
        ResourceLevel::None
    }
}

/// Select a random index from a slice of items using weighted random selection.
/// Returns None if weights sum to zero or items is empty.
fn weighted_random_index(rng: &mut impl Rng, weights: &[f64]) -> Option<usize> {
    let total: f64 = weights.iter().sum();
    if total <= 0.0 || weights.is_empty() {
        return None;
    }
    let mut roll = rng.random::<f64>() * total;
    for (i, &w) in weights.iter().enumerate() {
        roll -= w;
        if roll <= 0.0 {
            return Some(i);
        }
    }
    Some(weights.len() - 1)
}

fn random_habitability(rng: &mut impl Rng) -> Habitability {
    let roll: f32 = rng.random_range(0.0..1.0);
    if roll < 0.10 {
        Habitability::Ideal
    } else if roll < 0.35 {
        Habitability::Adequate
    } else if roll < 0.65 {
        Habitability::Marginal
    } else if roll < 0.90 {
        Habitability::Barren
    } else {
        Habitability::GasGiant
    }
}

fn random_resource_level(rng: &mut impl Rng) -> ResourceLevel {
    let roll: f32 = rng.random_range(0.0..1.0);
    if roll < 0.20 {
        ResourceLevel::Rich
    } else if roll < 0.55 {
        ResourceLevel::Moderate
    } else if roll < 0.80 {
        ResourceLevel::Poor
    } else {
        ResourceLevel::None
    }
}

fn building_slots_for(hab: Habitability, rng: &mut impl Rng) -> u8 {
    match hab {
        Habitability::Ideal => rng.random_range(5..=8),
        Habitability::Adequate => rng.random_range(3..=6),
        Habitability::Marginal => rng.random_range(2..=4),
        Habitability::Barren => rng.random_range(1..=2),
        Habitability::GasGiant => 0,
    }
}

fn random_attributes(rng: &mut impl Rng) -> SystemAttributes {
    let habitability = random_habitability(rng);
    SystemAttributes {
        habitability,
        mineral_richness: random_resource_level(rng),
        energy_potential: random_resource_level(rng),
        research_potential: random_resource_level(rng),
        max_building_slots: building_slots_for(habitability, rng),
    }
}

fn capital_attributes(rng: &mut impl Rng) -> SystemAttributes {
    SystemAttributes {
        habitability: Habitability::Ideal,
        mineral_richness: at_least_moderate(random_resource_level(rng)),
        energy_potential: at_least_moderate(random_resource_level(rng)),
        research_potential: at_least_moderate(random_resource_level(rng)),
        max_building_slots: building_slots_for(Habitability::Ideal, rng),
    }
}

fn at_least_moderate(level: ResourceLevel) -> ResourceLevel {
    match level {
        ResourceLevel::Poor | ResourceLevel::None => ResourceLevel::Moderate,
        other => other,
    }
}

fn is_habitable(h: Habitability) -> bool {
    !matches!(h, Habitability::GasGiant)
}

/// Generate planet attributes from a planet type definition and star habitability bonus.
fn planet_attributes_from_type(
    rng: &mut impl Rng,
    planet_type: &PlanetTypeDefinition,
    habitability_bonus: f64,
) -> SystemAttributes {
    let score = (planet_type.base_habitability + habitability_bonus).clamp(0.0, 1.0);
    let habitability = habitability_from_score(score);
    SystemAttributes {
        habitability,
        mineral_richness: resource_level_from_bias(rng, planet_type.resource_bias.minerals),
        energy_potential: resource_level_from_bias(rng, planet_type.resource_bias.energy),
        research_potential: resource_level_from_bias(rng, planet_type.resource_bias.research),
        max_building_slots: planet_type.base_slots as u8,
    }
}

/// Hardcoded fallback star types when no Lua definitions are loaded.
fn default_star_types() -> Vec<StarTypeDefinition> {
    vec![StarTypeDefinition {
        id: "default".to_string(),
        name: "Star".to_string(),
        description: String::new(),
        color: [1.0, 1.0, 0.9],
        planet_lambda: 2.0,
        max_planets: 3,
        habitability_bonus: 0.0,
        weight: 1.0,
    }]
}

/// Hardcoded fallback planet types when no Lua definitions are loaded.
fn default_planet_types() -> Vec<PlanetTypeDefinition> {
    vec![PlanetTypeDefinition {
        id: "default".to_string(),
        name: "Planet".to_string(),
        description: String::new(),
        base_habitability: 0.5,
        base_slots: 4,
        resource_bias: ResourceBias {
            minerals: 1.0,
            energy: 1.0,
            research: 1.0,
        },
        weight: 1.0,
    }]
}

/// Startup system that parses star and planet type definitions from Lua accumulators.
/// Scripts are loaded by `load_all_scripts`; this system only parses the results.
pub fn load_galaxy_types(
    engine: Res<crate::scripting::ScriptEngine>,
    mut star_registry: ResMut<StarTypeRegistry>,
    mut planet_registry: ResMut<PlanetTypeRegistry>,
) {
    match crate::scripting::galaxy_api::parse_star_types(engine.lua()) {
        Ok(types) => {
            info!("Loaded {} star type definitions", types.len());
            star_registry.types = types;
        }
        Err(e) => {
            warn!("Failed to parse star types: {e}");
        }
    }

    match crate::scripting::galaxy_api::parse_planet_types(engine.lua()) {
        Ok(types) => {
            info!("Loaded {} planet type definitions", types.len());
            planet_registry.types = types;
        }
        Err(e) => {
            warn!("Failed to parse planet types: {e}");
        }
    }
}

/// Galaxy generation parameters.
struct GalaxyParams {
    num_systems: usize,
    num_arms: usize,
    galaxy_radius: f64,
    arm_twist: f64,
    arm_spread: f64,
    min_distance: f64,
    max_neighbor_distance: f64,
}

/// An empty star system produced by Phase A (position + star type, no planets yet).
struct EmptySystem {
    name: String,
    position: [f64; 3],
    star_type_idx: usize,
}

/// Capital assignments produced by Phase B.
struct CapitalAssignments {
    /// Index into the systems vec that is the capital (always 0 after swap).
    capital_idx: usize,
}

/// Planet data generated during Phase C initialization.
struct PlanetData {
    type_idx: usize,
    attrs: SystemAttributes,
}

/// Phase A: Generate star system positions (spiral arms + bridge pass) and assign star types.
/// Returns a Vec of EmptySystem — no ECS entities are spawned yet.
fn generate_empty_systems(
    rng: &mut impl Rng,
    params: &GalaxyParams,
    star_weights: &[f64],
) -> Vec<EmptySystem> {
    let mut systems: Vec<(String, [f64; 3])> = Vec::new();
    let mut attempts = 0;

    while systems.len() < params.num_systems && attempts < params.num_systems * 50 {
        attempts += 1;

        // Choose a random arm
        let arm = rng.random_range(0..params.num_arms) as f64;
        let arm_base_angle = arm * std::f64::consts::TAU / params.num_arms as f64;

        // Random radius (biased toward middle, not too close to center)
        let r = rng.random_range(3.0_f64..params.galaxy_radius);
        // Apply sqrt for more uniform radial distribution, but with slight center bias
        let r = r.sqrt() / params.galaxy_radius.sqrt() * params.galaxy_radius;

        // Spiral angle increases with distance
        let spiral_angle =
            arm_base_angle + r / params.galaxy_radius * params.arm_twist * std::f64::consts::TAU;

        // Add random spread
        let angle_noise = rng.random_range(-params.arm_spread..params.arm_spread);
        let final_angle = spiral_angle + angle_noise;

        // Some extra noise in radius for natural look
        let r_noise = rng.random_range(-2.0_f64..2.0);
        let final_r = (r + r_noise).max(1.0);

        let x = final_r * final_angle.cos();
        let y = final_r * final_angle.sin();
        let z = rng.random_range(-1.0_f64..1.0); // thin disk

        // Minimum distance check
        let too_close = systems.iter().any(|(_, pos)| {
            let dx = pos[0] - x;
            let dy = pos[1] - y;
            let dz = pos[2] - z;
            (dx * dx + dy * dy + dz * dz).sqrt() < params.min_distance
        });
        if too_close {
            continue;
        }

        let name = format!("System-{:03}", systems.len());
        systems.push((name, [x, y, z]));
    }

    // Bridge pass: fix isolated systems (nearest neighbor > max_neighbor_distance).
    // For each isolated system, try to place a bridge system halfway to its nearest neighbor.
    let mut bridge_attempts = 0;
    let max_bridge_attempts = 100;
    loop {
        if bridge_attempts >= max_bridge_attempts {
            break;
        }
        // Find the most isolated system
        let mut worst_idx: Option<usize> = None;
        let mut worst_nearest_dist = 0.0_f64;
        let mut worst_nearest_idx = 0_usize;
        for (i, (_, pos_i)) in systems.iter().enumerate() {
            let mut nearest_dist = f64::MAX;
            let mut nearest_j = 0;
            for (j, (_, pos_j)) in systems.iter().enumerate() {
                if i == j {
                    continue;
                }
                let dx = pos_i[0] - pos_j[0];
                let dy = pos_i[1] - pos_j[1];
                let dz = pos_i[2] - pos_j[2];
                let dist = (dx * dx + dy * dy + dz * dz).sqrt();
                if dist < nearest_dist {
                    nearest_dist = dist;
                    nearest_j = j;
                }
            }
            if nearest_dist > params.max_neighbor_distance
                && nearest_dist > worst_nearest_dist
            {
                worst_nearest_dist = nearest_dist;
                worst_nearest_idx = nearest_j;
                worst_idx = Some(i);
            }
        }
        let Some(iso_idx) = worst_idx else {
            break; // No more isolated systems
        };
        bridge_attempts += 1;

        // Place a bridge system halfway between isolated system and its nearest neighbor
        let pos_a = systems[iso_idx].1;
        let pos_b = systems[worst_nearest_idx].1;
        let mid = [
            (pos_a[0] + pos_b[0]) / 2.0 + rng.random_range(-1.0_f64..1.0),
            (pos_a[1] + pos_b[1]) / 2.0 + rng.random_range(-1.0_f64..1.0),
            (pos_a[2] + pos_b[2]) / 2.0 + rng.random_range(-0.5_f64..0.5),
        ];
        // Check min_distance for bridge system
        let too_close = systems.iter().any(|(_, pos)| {
            let dx = pos[0] - mid[0];
            let dy = pos[1] - mid[1];
            let dz = pos[2] - mid[2];
            (dx * dx + dy * dy + dz * dz).sqrt() < params.min_distance
        });
        if !too_close {
            let name = format!("System-{:03}", systems.len());
            systems.push((name, mid));
        }
    }

    // Assign a star type to each system and build the result
    systems
        .into_iter()
        .map(|(name, position)| {
            let star_type_idx = weighted_random_index(rng, star_weights).unwrap_or(0);
            EmptySystem {
                name,
                position,
                star_type_idx,
            }
        })
        .collect()
}

/// Phase B: Choose which systems become faction capitals.
/// Currently selects the single player capital (~20 ly from center) and swaps it to index 0.
/// Returns capital assignments without modifying ECS state.
fn choose_faction_capitals(systems: &mut Vec<EmptySystem>) -> CapitalAssignments {
    let target_capital_radius = 20.0_f64;
    let capital_idx = systems
        .iter()
        .enumerate()
        .min_by(|(_, a), (_, b)| {
            let ra = (a.position[0] * a.position[0]
                + a.position[1] * a.position[1]
                + a.position[2] * a.position[2])
                .sqrt();
            let rb = (b.position[0] * b.position[0]
                + b.position[1] * b.position[1]
                + b.position[2] * b.position[2])
                .sqrt();
            let da = (ra - target_capital_radius).abs();
            let db = (rb - target_capital_radius).abs();
            da.partial_cmp(&db).unwrap()
        })
        .map(|(i, _)| i)
        .unwrap_or(0);

    // Swap capital to index 0 so the rest of the code treats systems[0] as capital
    systems.swap(0, capital_idx);

    CapitalAssignments { capital_idx: 0 }
}

/// Phase C: Initialize all systems — generate planets, spawn ECS entities, place hostiles.
fn initialize_systems(
    commands: &mut Commands,
    rng: &mut impl Rng,
    systems: &[EmptySystem],
    capitals: &CapitalAssignments,
    params: &GalaxyParams,
    star_types: &[StarTypeDefinition],
    planet_types: &[PlanetTypeDefinition],
    planet_weights: &[f64],
) {
    let actual_count = systems.len();

    // Determine planet counts per system
    let mut planet_counts: Vec<usize> = Vec::with_capacity(actual_count);
    for (i, sys) in systems.iter().enumerate() {
        let star = &star_types[sys.star_type_idx];
        let count = if i == capitals.capital_idx {
            // Capital always gets at least 2 planets
            poisson_sample(rng, star.planet_lambda, star.max_planets).max(2)
        } else {
            poisson_sample(rng, star.planet_lambda, star.max_planets)
        };
        planet_counts.push(count);
    }

    // Generate planet data: Vec of (planet_type_idx, attributes) per system
    let mut all_planets: Vec<Vec<PlanetData>> = Vec::with_capacity(actual_count);
    for (i, sys) in systems.iter().enumerate() {
        let star = &star_types[sys.star_type_idx];
        let count = planet_counts[i];
        let mut planets = Vec::with_capacity(count);
        for p in 0..count {
            if i == capitals.capital_idx && p == 0 {
                // Capital's first planet: use capital attributes and a terrestrial type
                let type_idx = planet_types
                    .iter()
                    .position(|pt| pt.id == "terrestrial")
                    .unwrap_or(0);
                planets.push(PlanetData {
                    type_idx,
                    attrs: capital_attributes(rng),
                });
            } else {
                let type_idx = weighted_random_index(rng, planet_weights).unwrap_or(0);
                let pt = &planet_types[type_idx];
                let attrs = planet_attributes_from_type(rng, pt, star.habitability_bonus);
                planets.push(PlanetData { type_idx, attrs });
            }
        }
        all_planets.push(planets);
    }

    // Ensure at least 2 habitable neighbours within 10 ly of capital
    let capital_pos = systems[capitals.capital_idx].position;
    let mut neighbours: Vec<(usize, f64)> = (1..actual_count)
        .map(|i| {
            let p = systems[i].position;
            let dx = p[0] - capital_pos[0];
            let dy = p[1] - capital_pos[1];
            let dz = p[2] - capital_pos[2];
            (i, (dx * dx + dy * dy + dz * dz).sqrt())
        })
        .collect();
    neighbours.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap());
    let nearby: Vec<usize> = neighbours
        .iter()
        .filter(|(_, dist)| *dist <= 10.0)
        .take(5)
        .map(|(i, _)| *i)
        .collect();

    // Check if nearby systems have at least one habitable planet
    let habitable_count = nearby
        .iter()
        .filter(|&&i| {
            all_planets[i]
                .iter()
                .any(|pd| is_habitable(pd.attrs.habitability))
        })
        .count();

    let needed = 2_usize.saturating_sub(habitable_count);
    let mut fixed = 0;
    for &idx in &nearby {
        if fixed >= needed {
            break;
        }
        let has_habitable = all_planets[idx]
            .iter()
            .any(|pd| is_habitable(pd.attrs.habitability));
        if !has_habitable {
            // Fix the first planet to be Adequate
            if let Some(first) = all_planets[idx].first_mut() {
                first.attrs.habitability = Habitability::Adequate;
                first.attrs.max_building_slots = building_slots_for(Habitability::Adequate, rng);
                fixed += 1;
            }
        }
    }

    // Gas obscured systems (15%)
    let gas_indices: Vec<usize> = (0..actual_count)
        .filter(|_| rng.random_range(0.0_f32..1.0) < 0.15)
        .collect();

    // Track spawned system entities and positions for hostile spawning
    let mut spawned_systems: Vec<(Entity, [f64; 3], bool)> = Vec::with_capacity(actual_count);

    for (i, sys) in systems.iter().enumerate() {
        let is_capital = i == capitals.capital_idx;
        let star_type = &star_types[sys.star_type_idx];

        let star = StarSystem {
            name: sys.name.clone(),
            surveyed: is_capital,
            is_capital,
            star_type: star_type.id.clone(),
        };

        // Capital sovereignty will be set by update_sovereignty once
        // the empire entity is spawned; start with default for all.
        let sovereignty = Sovereignty::default();

        let entity = commands.spawn((
            star,
            Position::from(sys.position),
            sovereignty,
            TechKnowledge::default(),
            SystemModifiers::default(),
            Anomalies::default(),
        ));
        let star_entity = entity.id();

        spawned_systems.push((star_entity, sys.position, is_capital));

        if gas_indices.contains(&i) && !is_capital {
            commands.entity(star_entity).insert(ObscuredByGas);
        }

        // Spawn planets for this star system
        for (p, planet_data) in all_planets[i].iter().enumerate() {
            let planet_name = format!("{} {}", sys.name, roman_numeral(p + 1));
            let planet_type = &planet_types[planet_data.type_idx];

            commands.spawn((
                Planet {
                    name: planet_name,
                    system: star_entity,
                    planet_type: planet_type.id.clone(),
                },
                planet_data.attrs.clone(),
                Position::from(sys.position), // same position as star for now
            ));
        }
    }

    commands.insert_resource(GalaxyConfig {
        radius: params.galaxy_radius,
        num_systems: actual_count,
    });

    // --- Spawn hostile presences (#52, #56) ---
    let hostile_fraction = 0.12;
    let capital_safe_zone = 10.0_f64; // no hostiles within 10 ly of capital
    let mut hostile_count = 0;
    for &(system_entity, pos, is_capital) in &spawned_systems {
        if is_capital {
            continue;
        }

        // Capital proximity exclusion
        let dx = pos[0] - capital_pos[0];
        let dy = pos[1] - capital_pos[1];
        let dz = pos[2] - capital_pos[2];
        let dist_from_capital = (dx * dx + dy * dy + dz * dz).sqrt();
        if dist_from_capital < capital_safe_zone {
            continue;
        }

        if rng.random::<f64>() > hostile_fraction {
            continue;
        }

        // Scale strength by distance from galaxy center
        let center_dist = (pos[0] * pos[0] + pos[1] * pos[1] + pos[2] * pos[2]).sqrt();
        let strength_mult = 1.0 + (center_dist / params.galaxy_radius) * 2.0;

        let hostile_type = if rng.random::<f64>() < 0.7 {
            HostileType::SpaceCreature
        } else {
            HostileType::AncientDefense
        };
        let base_hp = match hostile_type {
            HostileType::SpaceCreature => 80.0,
            HostileType::AncientDefense => 200.0,
        };
        let hp = base_hp * strength_mult;
        let strength = 10.0 * strength_mult;
        let evasion = match hostile_type {
            HostileType::SpaceCreature => 20.0,
            HostileType::AncientDefense => 10.0,
        };

        commands.spawn(HostilePresence {
            system: system_entity,
            strength,
            hp,
            max_hp: hp,
            hostile_type,
            evasion,
        });
        hostile_count += 1;
    }

    info!(
        "Galaxy generated: {} star systems (spiral, {} arms), {} hostile presences",
        actual_count, params.num_arms, hostile_count
    );
}

pub fn generate_galaxy(
    mut commands: Commands,
    star_registry: Res<StarTypeRegistry>,
    planet_registry: Res<PlanetTypeRegistry>,
) {
    let mut rng = rand::rng();
    let params = GalaxyParams {
        num_systems: 150,
        num_arms: 3,
        galaxy_radius: 80.0,
        arm_twist: 2.5,
        arm_spread: 0.4,
        min_distance: 2.0,
        max_neighbor_distance: 8.0,
    };

    // Use registries or fallback defaults
    let star_types = if star_registry.types.is_empty() {
        default_star_types()
    } else {
        star_registry.types.clone()
    };
    let planet_types = if planet_registry.types.is_empty() {
        default_planet_types()
    } else {
        planet_registry.types.clone()
    };

    let star_weights: Vec<f64> = star_types.iter().map(|s| s.weight).collect();
    let planet_weights: Vec<f64> = planet_types.iter().map(|p| p.weight).collect();

    // Phase A: Generate empty star systems (positions + star types only)
    let mut systems = generate_empty_systems(&mut rng, &params, &star_weights);

    // Phase B: Choose faction capitals
    let capitals = choose_faction_capitals(&mut systems);

    // Phase C: Initialize all systems (planets, resources, hostiles, ECS entities)
    initialize_systems(
        &mut commands,
        &mut rng,
        &systems,
        &capitals,
        &params,
        &star_types,
        &planet_types,
        &planet_weights,
    );
}
