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
                load_galaxy_types.after(crate::scripting::init_scripting),
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

/// Startup system that loads star and planet type definitions from Lua scripts.
pub fn load_galaxy_types(
    engine: Res<crate::scripting::ScriptEngine>,
    mut star_registry: ResMut<StarTypeRegistry>,
    mut planet_registry: ResMut<PlanetTypeRegistry>,
) {
    // Load star types
    let star_dir = Path::new("scripts/stars");
    if star_dir.exists() {
        if let Err(e) = engine.load_directory(star_dir) {
            warn!("Failed to load star type scripts: {e}");
        }
    }
    match crate::scripting::galaxy_api::parse_star_types(engine.lua()) {
        Ok(types) => {
            info!("Loaded {} star type definitions", types.len());
            star_registry.types = types;
        }
        Err(e) => {
            warn!("Failed to parse star types: {e}");
        }
    }

    // Load planet types
    let planet_dir = Path::new("scripts/planets");
    if planet_dir.exists() {
        if let Err(e) = engine.load_directory(planet_dir) {
            warn!("Failed to load planet type scripts: {e}");
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

pub fn generate_galaxy(
    mut commands: Commands,
    star_registry: Res<StarTypeRegistry>,
    planet_registry: Res<PlanetTypeRegistry>,
) {
    let mut rng = rand::rng();
    let num_systems = 150;
    let num_arms = 3;
    let galaxy_radius = 60.0_f64; // light-years
    let arm_twist = 2.5; // how tightly the arms spiral
    let arm_spread = 0.4; // angular spread of each arm
    let min_distance = 2.0_f64;
    let max_neighbor_distance = 8.0_f64; // isolation threshold

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

    let mut systems: Vec<(String, [f64; 3])> = Vec::new();
    let mut attempts = 0;

    while systems.len() < num_systems && attempts < num_systems * 50 {
        attempts += 1;

        // Choose a random arm
        let arm = rng.random_range(0..num_arms) as f64;
        let arm_base_angle = arm * std::f64::consts::TAU / num_arms as f64;

        // Random radius (biased toward middle, not too close to center)
        let r = rng.random_range(3.0_f64..galaxy_radius);
        // Apply sqrt for more uniform radial distribution, but with slight center bias
        let r = r.sqrt() / galaxy_radius.sqrt() * galaxy_radius;

        // Spiral angle increases with distance
        let spiral_angle = arm_base_angle + r / galaxy_radius * arm_twist * std::f64::consts::TAU;

        // Add random spread
        let angle_noise = rng.random_range(-arm_spread..arm_spread);
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
            (dx * dx + dy * dy + dz * dz).sqrt() < min_distance
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
            if nearest_dist > max_neighbor_distance && nearest_dist > worst_nearest_dist {
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
            (dx * dx + dy * dy + dz * dz).sqrt() < min_distance
        });
        if !too_close {
            let name = format!("System-{:03}", systems.len());
            systems.push((name, mid));
        }
    }

    // Choose capital: find system closest to ~20 ly from center (1/3 galaxy radius)
    let target_capital_radius = 20.0_f64;
    let capital_idx = systems
        .iter()
        .enumerate()
        .min_by(|(_, (_, a)), (_, (_, b))| {
            let ra = (a[0] * a[0] + a[1] * a[1] + a[2] * a[2]).sqrt();
            let rb = (b[0] * b[0] + b[1] * b[1] + b[2] * b[2]).sqrt();
            let da = (ra - target_capital_radius).abs();
            let db = (rb - target_capital_radius).abs();
            da.partial_cmp(&db).unwrap()
        })
        .map(|(i, _)| i)
        .unwrap_or(0);

    // Swap capital to index 0 so the rest of the code treats systems[0] as capital
    systems.swap(0, capital_idx);

    let actual_count = systems.len();

    // Assign a star type to each system
    let mut system_star_types: Vec<usize> = Vec::with_capacity(actual_count);
    for _ in 0..actual_count {
        let idx = weighted_random_index(&mut rng, &star_weights).unwrap_or(0);
        system_star_types.push(idx);
    }

    // For the "ensure habitable neighbours" pass we need to track per-system attributes
    // of the first planet. We'll generate all planet data in a second pass.
    // First, determine planet counts per system.
    let mut planet_counts: Vec<usize> = Vec::with_capacity(actual_count);
    for (i, &star_idx) in system_star_types.iter().enumerate() {
        let star = &star_types[star_idx];
        let count = if i == 0 {
            // Capital always gets at least 2 planets
            poisson_sample(&mut rng, star.planet_lambda, star.max_planets).max(2)
        } else {
            poisson_sample(&mut rng, star.planet_lambda, star.max_planets)
        };
        planet_counts.push(count);
    }

    // Generate planet data: Vec of (planet_type_idx, attributes) per system
    struct PlanetData {
        type_idx: usize,
        attrs: SystemAttributes,
    }
    let mut all_planets: Vec<Vec<PlanetData>> = Vec::with_capacity(actual_count);
    for (i, &star_idx) in system_star_types.iter().enumerate() {
        let star = &star_types[star_idx];
        let count = planet_counts[i];
        let mut planets = Vec::with_capacity(count);
        for p in 0..count {
            if i == 0 && p == 0 {
                // Capital's first planet: use capital attributes and a terrestrial type
                let type_idx = planet_types
                    .iter()
                    .position(|pt| pt.id == "terrestrial")
                    .unwrap_or(0);
                planets.push(PlanetData {
                    type_idx,
                    attrs: capital_attributes(&mut rng),
                });
            } else {
                let type_idx =
                    weighted_random_index(&mut rng, &planet_weights).unwrap_or(0);
                let pt = &planet_types[type_idx];
                let attrs = planet_attributes_from_type(&mut rng, pt, star.habitability_bonus);
                planets.push(PlanetData { type_idx, attrs });
            }
        }
        all_planets.push(planets);
    }

    // Ensure at least 2 habitable neighbours within 10 ly of capital
    let capital_pos = systems[0].1;
    let mut neighbours: Vec<(usize, f64)> = (1..actual_count)
        .map(|i| {
            let p = systems[i].1;
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
                first.attrs.max_building_slots =
                    building_slots_for(Habitability::Adequate, &mut rng);
                fixed += 1;
            }
        }
    }

    // Gas obscured systems (15%)
    let gas_indices: Vec<usize> = (0..actual_count)
        .filter(|_| rng.random_range(0.0_f32..1.0) < 0.15)
        .collect();

    for (i, (name, position)) in systems.iter().enumerate() {
        let is_capital = i == 0;
        let star_idx = system_star_types[i];
        let star_type = &star_types[star_idx];

        let star = StarSystem {
            name: name.clone(),
            surveyed: is_capital,
            is_capital,
            star_type: star_type.id.clone(),
        };

        // Capital sovereignty will be set by update_sovereignty once
        // the empire entity is spawned; start with default for all.
        let sovereignty = Sovereignty::default();

        let entity = commands.spawn((
            star,
            Position::from(*position),
            sovereignty,
            TechKnowledge::default(),
            SystemModifiers::default(),
        ));
        let star_entity = entity.id();

        if gas_indices.contains(&i) && !is_capital {
            commands.entity(star_entity).insert(ObscuredByGas);
        }

        // Spawn planets for this star system
        for (p, planet_data) in all_planets[i].iter().enumerate() {
            let planet_name = format!("{} {}", name, roman_numeral(p + 1));
            let planet_type = &planet_types[planet_data.type_idx];

            commands.spawn((
                Planet {
                    name: planet_name,
                    system: star_entity,
                    planet_type: planet_type.id.clone(),
                },
                planet_data.attrs.clone(),
                Position::from(*position), // same position as star for now
            ));
        }
    }

    commands.insert_resource(GalaxyConfig {
        radius: galaxy_radius,
        num_systems: actual_count,
    });

    info!(
        "Galaxy generated: {} star systems (spiral, {} arms)",
        actual_count, num_arms
    );
}
