use bevy::prelude::*;
use rand::Rng;

use crate::components::Position;
use crate::ship::Owner;
use crate::technology::TechKnowledge;

pub struct GalaxyPlugin;

impl Plugin for GalaxyPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Startup, generate_galaxy);
    }
}

/// A star system in the galaxy
#[derive(Component)]
pub struct StarSystem {
    pub name: String,
    /// Whether this system has been surveyed (precise data available)
    pub surveyed: bool,
    /// Whether this system is colonized
    pub colonized: bool,
    /// Whether this system is the capital
    pub is_capital: bool,
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

pub fn generate_galaxy(mut commands: Commands) {
    let mut rng = rand::rng();
    let num_systems = 100;
    let num_arms = 3;
    let galaxy_radius = 80.0_f64; // light-years
    let arm_twist = 2.5; // how tightly the arms spiral
    let arm_spread = 0.4; // angular spread of each arm
    let min_distance = 3.0_f64;

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

    // Choose capital: find system closest to ~27 ly from center (1/3 galaxy radius)
    let target_capital_radius = 27.0_f64;
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

    // Generate attributes
    let mut attributes: Vec<SystemAttributes> = Vec::with_capacity(actual_count);
    for i in 0..actual_count {
        if i == 0 {
            attributes.push(capital_attributes(&mut rng));
        } else {
            attributes.push(random_attributes(&mut rng));
        }
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

    let habitable_count = nearby
        .iter()
        .filter(|&&i| is_habitable(attributes[i].habitability))
        .count();

    // Ensure at least 2 habitable neighbours
    let needed = 2_usize.saturating_sub(habitable_count);
    let mut fixed = 0;
    for &idx in &nearby {
        if fixed >= needed {
            break;
        }
        if !is_habitable(attributes[idx].habitability) {
            attributes[idx].habitability = Habitability::Adequate;
            attributes[idx].max_building_slots =
                building_slots_for(Habitability::Adequate, &mut rng);
            fixed += 1;
        }
    }

    // Gas obscured systems (15%)
    let gas_indices: Vec<usize> = (0..actual_count)
        .filter(|_| rng.random_range(0.0_f32..1.0) < 0.15)
        .collect();

    for (i, (name, position)) in systems.iter().enumerate() {
        let is_capital = i == 0;
        let star = StarSystem {
            name: name.clone(),
            surveyed: is_capital,
            colonized: is_capital,
            is_capital,
        };

        // Capital sovereignty will be set by update_sovereignty once
        // the empire entity is spawned; start with default for all.
        let sovereignty = Sovereignty::default();

        let entity = commands.spawn((star, Position::from(*position), attributes[i].clone(), sovereignty, TechKnowledge::default()));
        let entity_id = entity.id();

        if gas_indices.contains(&i) && !is_capital {
            commands.entity(entity_id).insert(ObscuredByGas);
        }
    }

    info!("Galaxy generated: {} star systems (spiral, {} arms)", actual_count, num_arms);
}
