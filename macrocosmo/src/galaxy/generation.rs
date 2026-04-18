use bevy::prelude::*;
use rand::{Rng, SeedableRng};
use std::sync::Arc;

use crate::components::Position;
use crate::scripting::ScriptEngine;
use crate::scripting::galaxy_api::{
    PlanetTypeDefinition, PlanetTypeRegistry, StarTypeDefinition, StarTypeRegistry,
};
use crate::scripting::galaxy_gen_ctx::{
    self, ChooseCapitalsCtx, GalaxyGenerateCtx, GenerationSettings, InitializeSystemActions,
    InitializeSystemCtx, PlanetAttrsOverride, SystemSnapshot,
};
use crate::scripting::map_api::{
    MapTypeRegistry, PredefinedPlanetSpec, PredefinedSystemRegistry, lookup_map_type_generator,
};
use crate::technology::TechKnowledge;

use super::biome::resolve_biome_id;
use super::types::{default_planet_types, default_star_types};
use super::{
    Anomalies, AtSystem, Biome, BiomeRegistry, GalaxyConfig, Hostile, HostileHitpoints,
    HostileStats, ObscuredByGas, Planet, Sovereignty, StarSystem, StarTypeModifierSet,
    SystemAttributes, SystemModifiers,
};
use crate::amount::SignedAmt;
use crate::faction::{FactionOwner, HostileFactions};
use crate::modifier::Modifier;
use crate::scripting::galaxy_api::StarTypeModifier;

/// Galaxy generation parameters.
pub(crate) struct GalaxyParams {
    pub num_systems: usize,
    pub num_arms: usize,
    pub galaxy_radius: f64,
    pub arm_twist: f64,
    pub arm_spread: f64,
    pub min_distance: f64,
    pub max_neighbor_distance: f64,
    /// #199: Baseline FTL range used by Lua-side connectivity loops (exposed
    /// via `ctx.settings.initial_ftl_range`). Independent from per-ship FTL
    /// values; this is the reference threshold for generation-time FTL
    /// reachability checks.
    pub initial_ftl_range: f64,
}

/// An empty star system produced by Phase A (position + star type, no planets yet).
///
/// `predefined_planets` and `capital_for_faction` are filled only when Phase A
/// spawned this system via `ctx:spawn_predefined_system` (#182). They flow
/// through Phase B (capital hint) and Phase C (planet list replaces the
/// default Poisson roll).
pub(crate) struct EmptySystem {
    pub name: String,
    pub position: [f64; 3],
    pub star_type_idx: usize,
    pub predefined_planets: Vec<PredefinedPlanetSpec>,
    pub capital_for_faction: Option<String>,
}

/// Capital assignments produced by Phase B.
pub(crate) struct CapitalAssignments {
    /// Index into the systems vec that is the capital (always 0 after swap).
    pub capital_idx: usize,
}

/// Planet data generated during Phase C initialization.
#[derive(Clone, Debug)]
pub(crate) struct PlanetData {
    pub type_idx: usize,
    pub attrs: SystemAttributes,
    /// Lua-spawned planets carry their explicit name here; default-generated
    /// planets leave this `None` and receive the standard
    /// `"{system} {roman}"` name.
    pub name_override: Option<String>,
}

/// Build a Modifier from a StarTypeModifier, using the star type id as a stable id prefix.
fn make_modifier_for_star(star_id: &str, m: &StarTypeModifier) -> Modifier {
    Modifier {
        id: format!("star_type:{}:{}", star_id, m.target),
        label: format!("Star type: {}", star_id),
        base_add: SignedAmt::from_f64(m.base_add),
        multiplier: SignedAmt::from_f64(m.multiplier),
        add: SignedAmt::from_f64(m.add),
        expires_at: None,
        on_expire_event: None,
    }
}

/// Apply any known `ship.*` star-type modifier targets to a SystemModifiers.
/// Unknown targets (e.g. `system.research_bonus`, `ship.shield_regen`) are
/// preserved in StarTypeModifierSet for future wiring — they are intentionally
/// no-ops here rather than warnings, since definitions may declare targets
/// ahead of the engine gaining support for them.
fn apply_star_type_modifiers_to_system(
    modifiers: &[StarTypeModifier],
    star_id: &str,
    mods: &mut SystemModifiers,
) {
    for m in modifiers {
        let modifier = make_modifier_for_star(star_id, m);
        match m.target.as_str() {
            "ship.speed" => mods.ship_speed.push_modifier(modifier),
            "ship.attack" => mods.ship_attack.push_modifier(modifier),
            "ship.defense" => mods.ship_defense.push_modifier(modifier),
            _ => {}
        }
    }
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

/// Convert a resource bias value to a continuous resource level (0.0..1.0) using a random roll.
fn resource_level_from_bias(rng: &mut impl Rng, bias: f64) -> f64 {
    // Generate a random value scaled by the bias, then clamp to [0.0, 1.0]
    (rng.random::<f64>() * bias).clamp(0.0, 1.0)
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

/// Generate a random habitability value using a weighted distribution.
fn random_habitability(rng: &mut impl Rng) -> f64 {
    let roll: f32 = rng.random_range(0.0..1.0);
    if roll < 0.10 {
        // Ideal range: 0.9-1.0
        rng.random_range(0.9..=1.0)
    } else if roll < 0.35 {
        // Adequate range: 0.6-0.9
        rng.random_range(0.6..0.9)
    } else if roll < 0.65 {
        // Marginal range: 0.3-0.6
        rng.random_range(0.3..0.6)
    } else if roll < 0.90 {
        // Barren range: 0.01-0.3
        rng.random_range(0.01..0.3)
    } else {
        // Uninhabitable (gas giant equivalent)
        0.0
    }
}

/// Generate a random resource level value (0.0..1.0).
fn random_resource_level(rng: &mut impl Rng) -> f64 {
    rng.random_range(0.0..1.0)
}

/// Calculate building slots based on habitability score.
fn building_slots_for(hab: f64, rng: &mut impl Rng) -> u8 {
    if hab >= 0.9 {
        rng.random_range(5..=8)
    } else if hab >= 0.6 {
        rng.random_range(3..=6)
    } else if hab >= 0.3 {
        rng.random_range(2..=4)
    } else if hab > 0.0 {
        rng.random_range(1..=2)
    } else {
        0
    }
}

fn capital_attributes(rng: &mut impl Rng) -> SystemAttributes {
    let habitability = 1.0;
    SystemAttributes {
        habitability,
        mineral_richness: random_resource_level(rng).max(0.4),
        energy_potential: random_resource_level(rng).max(0.4),
        research_potential: random_resource_level(rng).max(0.4),
        max_building_slots: building_slots_for(habitability, rng),
    }
}

/// Generate planet attributes from a planet type definition and star habitability bonus.
fn planet_attributes_from_type(
    rng: &mut impl Rng,
    planet_type: &PlanetTypeDefinition,
    habitability_bonus: f64,
) -> SystemAttributes {
    let habitability = (planet_type.base_habitability + habitability_bonus).clamp(0.0, 1.0);
    SystemAttributes {
        habitability,
        mineral_richness: resource_level_from_bias(rng, planet_type.resource_bias.minerals),
        energy_potential: resource_level_from_bias(rng, planet_type.resource_bias.energy),
        research_potential: resource_level_from_bias(rng, planet_type.resource_bias.research),
        max_building_slots: planet_type.base_slots as u8,
    }
}

/// Phase A (default): Generate star system positions (spiral arms + bridge pass)
/// and assign star types. Returns a Vec of EmptySystem — no ECS entities are
/// spawned yet.
///
/// This is the Rust built-in that runs when no `on_galaxy_generate_empty`
/// hook is registered in Lua.
pub(crate) fn default_generate_empty_systems(
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
            if nearest_dist > params.max_neighbor_distance && nearest_dist > worst_nearest_dist {
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
                predefined_planets: Vec::new(),
                capital_for_faction: None,
            }
        })
        .collect()
}

/// Phase B (default): Choose which systems become faction capitals.
/// Currently selects the single player capital (~20 ly from center) and swaps
/// it to index 0. Returns capital assignments without modifying ECS state.
///
/// Runs when no `on_choose_capitals` hook is registered.
pub(crate) fn default_choose_faction_capitals(
    systems: &mut Vec<EmptySystem>,
) -> CapitalAssignments {
    // #182: if Phase A tagged any system with `capital_for_faction` (via a
    // predefined system definition), pick the first such system as capital.
    if let Some(idx) = systems.iter().position(|s| s.capital_for_faction.is_some()) {
        systems.swap(0, idx);
        return CapitalAssignments { capital_idx: 0 };
    }

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

/// Per-system override produced by the `on_initialize_system` Lua hook.
/// Consumed by `initialize_systems` after the default planet data has been
/// computed — if present, the default planets for that system are replaced.
#[derive(Default, Clone, Debug)]
pub(crate) struct SystemInitOverride {
    /// Lua-spawned planets for this system, awaiting planet-type resolution
    /// against the `PlanetTypeRegistry`.
    pub pending_planets: Vec<PendingPlanet>,
    /// If true, replace defaults with `pending_planets` (which may be empty
    /// to intentionally suppress planets).
    pub override_planets: bool,
    /// Optional override for system name.
    pub name: Option<String>,
    /// Optional override for the surveyed flag.
    pub surveyed: Option<bool>,
}

/// Phase C: Initialize all systems — generate planets, spawn ECS entities,
/// place hostiles. `overrides[i]` (if present) is applied to system `i`.
pub(crate) fn initialize_systems(
    commands: &mut Commands,
    rng: &mut impl Rng,
    systems: &[EmptySystem],
    capitals: &CapitalAssignments,
    params: &GalaxyParams,
    star_types: &[StarTypeDefinition],
    planet_types: &[PlanetTypeDefinition],
    planet_weights: &[f64],
    overrides: &[SystemInitOverride],
    hostile_factions: HostileFactions,
    biome_registry: &BiomeRegistry,
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

    // Generate planet data: Vec of (planet_type_idx, attributes) per system.
    // Priority (highest first):
    //   1. `on_initialize_system` hook override (if override_planets = true)
    //   2. predefined-system planets (when Phase A used spawn_predefined_system)
    //   3. default Poisson-roll generation
    let mut all_planets: Vec<Vec<PlanetData>> = Vec::with_capacity(actual_count);
    for (i, sys) in systems.iter().enumerate() {
        // 1. Hook override wins.
        if overrides
            .get(i)
            .map(|o| o.override_planets)
            .unwrap_or(false)
        {
            let resolved = resolve_pending_planets(
                &overrides[i].pending_planets,
                planet_types,
                &star_types[sys.star_type_idx],
            );
            all_planets.push(resolved);
            continue;
        }
        // 2. Predefined planets (from spawn_predefined_system).
        if !sys.predefined_planets.is_empty() {
            let resolved = resolve_predefined_planets(
                &sys.predefined_planets,
                planet_types,
                &star_types[sys.star_type_idx],
            );
            all_planets.push(resolved);
            continue;
        }
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
                    name_override: None,
                });
            } else {
                let type_idx = weighted_random_index(rng, planet_weights).unwrap_or(0);
                let pt = &planet_types[type_idx];
                let attrs = planet_attributes_from_type(rng, pt, star.habitability_bonus);
                planets.push(PlanetData {
                    type_idx,
                    attrs,
                    name_override: None,
                });
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
                .any(|pd| super::is_habitable(pd.attrs.habitability))
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
            .any(|pd| super::is_habitable(pd.attrs.habitability));
        if !has_habitable {
            // Fix the first planet to be Adequate (0.7 habitability)
            if let Some(first) = all_planets[idx].first_mut() {
                first.attrs.habitability = 0.7;
                first.attrs.max_building_slots = building_slots_for(0.7, rng);
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

        // Apply optional per-system name / surveyed override.
        let sys_override = overrides.get(i);
        let name = sys_override
            .and_then(|o| o.name.clone())
            .unwrap_or_else(|| sys.name.clone());
        let surveyed = sys_override.and_then(|o| o.surveyed).unwrap_or(is_capital);

        let star = StarSystem {
            name: name.clone(),
            surveyed,
            is_capital,
            star_type: star_type.id.clone(),
        };

        // #295 (S-1): Sovereignty is now a derived view of Core ship
        // presence — `update_sovereignty` writes `owner` based on a
        // `(AtSystem, FactionOwner)` query. Start at default (None) for
        // all systems; ownership appears once a Core ship enters.
        let sovereignty = Sovereignty::default();

        // Build SystemModifiers with any known ship.* targets from the star type
        // applied. Unknown targets are retained in StarTypeModifierSet below.
        let mut system_modifiers = SystemModifiers::default();
        apply_star_type_modifiers_to_system(
            &star_type.modifiers,
            &star_type.id,
            &mut system_modifiers,
        );

        let entity = commands.spawn((
            star,
            Position::from(sys.position),
            sovereignty,
            TechKnowledge::default(),
            system_modifiers,
            StarTypeModifierSet {
                entries: star_type.modifiers.clone(),
            },
            Anomalies::default(),
        ));
        let star_entity = entity.id();

        spawned_systems.push((star_entity, sys.position, is_capital));

        if gas_indices.contains(&i) && !is_capital {
            commands.entity(star_entity).insert(ObscuredByGas);
        }

        // Spawn planets for this star system. Planet names from
        // `spawn_planet` hook calls are used verbatim; default-generated
        // planets keep the `"{system} {roman}"` convention.
        for (p, planet_data) in all_planets[i].iter().enumerate() {
            let planet_name = planet_data
                .name_override
                .clone()
                .unwrap_or_else(|| format!("{} {}", name, super::roman_numeral(p + 1)));
            let planet_type = &planet_types[planet_data.type_idx];
            // #335: Resolve biome from planet_type.default_biome via the
            // BiomeRegistry. `resolve_biome_id` falls back to
            // DEFAULT_BIOME_ID when the referenced biome is unknown.
            let biome_id = resolve_biome_id(planet_type.default_biome.as_deref(), biome_registry);

            commands.spawn((
                Planet {
                    name: planet_name,
                    system: star_entity,
                    planet_type: planet_type.id.clone(),
                },
                Biome::new(biome_id),
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

        // #293: 70/30 split between space_creature and ancient_defense
        // faction buckets. Values move to Lua FactionTypeDefinition for
        // the base hp/strength/evasion; galaxy-center distance scales hp
        // and strength via `strength_mult`. `spawn_hostile_factions` runs
        // before `generate_galaxy` so `HostileFactions` is populated and
        // `FactionOwner` can be attached directly at spawn time.
        let (faction_entity, base_hp, base_strength, evasion) = if rng.random::<f64>() < 0.7 {
            (hostile_factions.space_creature, 80.0, 10.0, 20.0)
        } else {
            (hostile_factions.ancient_defense, 200.0, 10.0, 10.0)
        };
        let Some(faction_entity) = faction_entity else {
            warn!(
                "Hostile spawn skipped: HostileFactions not populated. \
                 Ensure spawn_hostile_factions runs before generate_galaxy."
            );
            continue;
        };
        let hp = base_hp * strength_mult;
        let strength = base_strength * strength_mult;

        commands.spawn((
            AtSystem(system_entity),
            HostileHitpoints { hp, max_hp: hp },
            HostileStats { strength, evasion },
            Hostile,
            FactionOwner(faction_entity),
        ));
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
    // #335: BiomeRegistry is `Option<Res<_>>` so tests that don't install
    // `GalaxyPlugin` (and therefore lack the registry) still run. Absent
    // registry → all planets resolve to DEFAULT_BIOME_ID.
    biome_registry: Option<Res<BiomeRegistry>>,
    engine: Option<Res<ScriptEngine>>,
    predefined_registry: Option<Res<PredefinedSystemRegistry>>,
    map_type_registry: Option<Res<MapTypeRegistry>>,
    rng_seed: Option<Res<crate::observer::RngSeed>>,
    hostile_factions: Option<Res<HostileFactions>>,
) {
    let mut rng: rand::rngs::StdRng = match rng_seed.as_deref().and_then(|s| s.0) {
        Some(seed) => {
            info!("Galaxy generation: using deterministic seed {}", seed);
            rand::rngs::StdRng::seed_from_u64(seed)
        }
        None => rand::rngs::StdRng::from_os_rng(),
    };
    let params = GalaxyParams {
        num_systems: 150,
        num_arms: 3,
        galaxy_radius: 80.0,
        arm_twist: 2.5,
        arm_spread: 0.4,
        min_distance: 2.0,
        max_neighbor_distance: 8.0,
        initial_ftl_range: 10.0,
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

    let lua = engine.as_deref().map(|e| e.lua());

    // #182: snapshot registries for Lua ctx use.
    let predefined_arc: Option<Arc<PredefinedSystemRegistry>> = predefined_registry
        .as_deref()
        .map(|r| Arc::new(clone_predefined_registry(r)));
    let active_map_type: Option<String> =
        map_type_registry.as_deref().and_then(|r| r.current.clone());

    // Phase A: Generate empty star systems (positions + star types only).
    // Precedence: active map_type.generator → on_galaxy_generate_empty hook
    // → default spiral.
    let mut systems = run_phase_a(
        lua,
        &mut rng,
        &params,
        &star_types,
        &star_weights,
        predefined_arc.clone(),
        active_map_type.as_deref(),
    );

    // Phase A' (#199): run `on_after_phase_a` hook if registered, to allow
    // Lua-driven connectivity enforcement (FTL-reachability bridges).
    run_after_phase_a(
        lua,
        &mut systems,
        &params,
        &star_types,
        predefined_arc.clone(),
    );

    // Phase B: Choose faction capitals. Honors `on_choose_capitals` if registered.
    let capitals = run_phase_b(lua, &mut systems, &star_types);

    // Phase C per-system hook: gather overrides before entity spawning.
    let overrides = run_phase_c_hooks(lua, &systems, &capitals, &star_types);

    // Phase C: Initialize all systems (planets, resources, hostiles, ECS entities)
    // Fall back to a default registry when none was inserted (test-only path).
    let biome_fallback = BiomeRegistry::default();
    let biome_ref: &BiomeRegistry = biome_registry.as_deref().unwrap_or(&biome_fallback);

    initialize_systems(
        &mut commands,
        &mut rng,
        &systems,
        &capitals,
        &params,
        &star_types,
        &planet_types,
        &planet_weights,
        &overrides,
        hostile_factions.as_deref().copied().unwrap_or_default(),
        biome_ref,
    );
}

// --- Hook dispatchers -------------------------------------------------

/// Phase A dispatcher: if an `on_galaxy_generate_empty` hook is registered,
/// run it and convert the recorded `SpawnedEmptySystemSpec`s into
/// `EmptySystem`s. Otherwise fall back to the default Rust implementation.
///
/// Lua-provided star-type ids that don't match any definition are rejected —
/// the system is skipped with a warning rather than silently defaulting to
/// index 0.
fn run_phase_a(
    lua: Option<&mlua::Lua>,
    rng: &mut impl Rng,
    params: &GalaxyParams,
    star_types: &[StarTypeDefinition],
    star_weights: &[f64],
    predefined: Option<Arc<PredefinedSystemRegistry>>,
    active_map_type: Option<&str>,
) -> Vec<EmptySystem> {
    let Some(lua) = lua else {
        return default_generate_empty_systems(rng, params, star_weights);
    };

    // #182: active map_type.generator wins over `on_galaxy_generate_empty`.
    let func = if let Some(map_id) = active_map_type {
        match lookup_map_type_generator(lua, map_id) {
            Ok(Some(f)) => Some(f),
            Ok(None) => {
                warn!(
                    "active map_type '{}' has no generator; falling back to on_galaxy_generate_empty / default",
                    map_id
                );
                galaxy_gen_ctx::last_registered_hook(lua, galaxy_gen_ctx::GENERATE_EMPTY_HANDLERS)
                    .ok()
                    .flatten()
            }
            Err(e) => {
                warn!("map_type lookup error: {e}; falling back to default");
                None
            }
        }
    } else {
        galaxy_gen_ctx::last_registered_hook(lua, galaxy_gen_ctx::GENERATE_EMPTY_HANDLERS)
            .ok()
            .flatten()
    };

    let Some(func) = func else {
        return default_generate_empty_systems(rng, params, star_weights);
    };

    let settings = GenerationSettings {
        num_systems: params.num_systems,
        num_arms: params.num_arms,
        galaxy_radius: params.galaxy_radius,
        arm_twist: params.arm_twist,
        arm_spread: params.arm_spread,
        min_distance: params.min_distance,
        max_neighbor_distance: params.max_neighbor_distance,
        initial_ftl_range: params.initial_ftl_range,
    };
    let mut ctx = GalaxyGenerateCtx::new(settings);
    if let Some(reg) = predefined {
        ctx = ctx.with_predefined(reg);
    }
    if let Err(e) = func.call::<()>(ctx.clone()) {
        warn!("Phase A hook error: {e}; falling back to default");
        return default_generate_empty_systems(rng, params, star_weights);
    }
    let actions = ctx.take_actions();
    if actions.spawned_systems.is_empty() {
        warn!("Phase A hook produced no systems; falling back to default");
        return default_generate_empty_systems(rng, params, star_weights);
    }

    let mut out = Vec::with_capacity(actions.spawned_systems.len());
    for spec in actions.spawned_systems {
        let Some(idx) = star_types.iter().position(|s| s.id == spec.star_type) else {
            warn!(
                "on_galaxy_generate_empty: unknown star_type '{}' for system '{}' — skipping",
                spec.star_type, spec.name
            );
            continue;
        };
        out.push(EmptySystem {
            name: spec.name,
            position: spec.position,
            star_type_idx: idx,
            predefined_planets: spec.planets.planets,
            capital_for_faction: spec.capital_for_faction,
        });
    }
    out
}

/// Phase B dispatcher.
///
/// Semantics when the hook is registered: the hook SHOULD call
/// `ctx:assign_capital(idx, faction_id)` for every capital it wants to mark.
/// We interpret the *first* assignment as the player's capital (swapped to
/// index 0), matching legacy behavior. If the hook does not assign any
/// capital, we fall back to the default heuristic.
fn run_phase_b(
    lua: Option<&mlua::Lua>,
    systems: &mut Vec<EmptySystem>,
    star_types: &[StarTypeDefinition],
) -> CapitalAssignments {
    let Some(lua) = lua else {
        return default_choose_faction_capitals(systems);
    };
    let Some(func) =
        galaxy_gen_ctx::last_registered_hook(lua, galaxy_gen_ctx::CHOOSE_CAPITALS_HANDLERS)
            .ok()
            .flatten()
    else {
        return default_choose_faction_capitals(systems);
    };

    let snapshots: Vec<SystemSnapshot> = systems
        .iter()
        .map(|s| SystemSnapshot {
            name: s.name.clone(),
            position: s.position,
            star_type: star_types
                .get(s.star_type_idx)
                .map(|st| st.id.clone())
                .unwrap_or_default(),
            capital_for_faction: s.capital_for_faction.clone(),
        })
        .collect();
    // TODO(#182): expose defined faction ids via FactionRegistry once that
    // resource is available at galaxy-generation time. For now pass empty —
    // hooks typically know their own faction ids.
    let ctx = ChooseCapitalsCtx::new(snapshots, Vec::new());
    if let Err(e) = func.call::<()>(ctx.clone()) {
        warn!("on_choose_capitals hook error: {e}; falling back to default");
        return default_choose_faction_capitals(systems);
    }
    let actions = ctx.take_actions();
    let Some(first) = actions.assignments.first() else {
        warn!("on_choose_capitals made no assignments; falling back to default");
        return default_choose_faction_capitals(systems);
    };
    let idx = first.system_index;
    if idx == 0 || idx > systems.len() {
        warn!(
            "on_choose_capitals: capital index {} out of range (1..={}); falling back to default",
            idx,
            systems.len()
        );
        return default_choose_faction_capitals(systems);
    }
    // Swap the selected capital to index 0, matching default behavior.
    systems.swap(0, idx - 1);
    CapitalAssignments { capital_idx: 0 }
}

/// Phase A' dispatcher (#199): run the `on_after_phase_a` hook if registered.
///
/// The hook is given a fresh `GalaxyGenerateCtx` seeded with the current list
/// of systems (reconstructed as `SpawnedEmptySystemSpec` records). Any newly
/// spawned systems / bridges recorded via `ctx:insert_bridge_at` or
/// `ctx:spawn_empty_system` are merged back into the `systems` vector.
///
/// Unknown star-type ids on newly recorded systems are skipped with a
/// warning (matching `run_phase_a` behavior).
fn run_after_phase_a(
    lua: Option<&mlua::Lua>,
    systems: &mut Vec<EmptySystem>,
    params: &GalaxyParams,
    star_types: &[StarTypeDefinition],
    predefined: Option<Arc<PredefinedSystemRegistry>>,
) {
    let Some(lua) = lua else {
        return;
    };
    let Some(func) =
        galaxy_gen_ctx::last_registered_hook(lua, galaxy_gen_ctx::AFTER_PHASE_A_HANDLERS)
            .ok()
            .flatten()
    else {
        return;
    };

    // Rebuild a ctx that reflects post-Phase-A state. The hook can inspect
    // `ctx.systems` / `ctx:build_ftl_graph(...)` and append new systems via
    // `ctx:insert_bridge_at` / `ctx:spawn_empty_system`.
    let settings = GenerationSettings {
        num_systems: params.num_systems,
        num_arms: params.num_arms,
        galaxy_radius: params.galaxy_radius,
        arm_twist: params.arm_twist,
        arm_spread: params.arm_spread,
        min_distance: params.min_distance,
        max_neighbor_distance: params.max_neighbor_distance,
        initial_ftl_range: params.initial_ftl_range,
    };
    let mut ctx = GalaxyGenerateCtx::new(settings);
    if let Some(reg) = predefined {
        ctx = ctx.with_predefined(reg);
    }
    // Seed the ctx with already-generated systems so `ctx.systems` and graph
    // methods reflect the current state.
    {
        let mut actions = ctx.actions.lock().unwrap();
        for sys in systems.iter() {
            let star_id = star_types
                .get(sys.star_type_idx)
                .map(|st| st.id.clone())
                .unwrap_or_default();
            actions.spawned_systems.push(
                crate::scripting::galaxy_gen_ctx::SpawnedEmptySystemSpec {
                    name: sys.name.clone(),
                    position: sys.position,
                    star_type: star_id,
                    planets: crate::scripting::galaxy_gen_ctx::PredefinedPlanetsForSpawn {
                        planets: sys.predefined_planets.clone(),
                    },
                    capital_for_faction: sys.capital_for_faction.clone(),
                },
            );
        }
    }
    let before = systems.len();

    if let Err(e) = func.call::<()>(ctx.clone()) {
        warn!("on_after_phase_a hook error: {e}; skipping");
        return;
    }
    let actions = ctx.take_actions();
    // Append any systems recorded beyond the seeded ones.
    for spec in actions.spawned_systems.into_iter().skip(before) {
        let Some(idx) = star_types.iter().position(|s| s.id == spec.star_type) else {
            warn!(
                "on_after_phase_a: unknown star_type '{}' for system '{}' — skipping",
                spec.star_type, spec.name
            );
            continue;
        };
        systems.push(EmptySystem {
            name: spec.name,
            position: spec.position,
            star_type_idx: idx,
            predefined_planets: spec.planets.planets,
            capital_for_faction: spec.capital_for_faction,
        });
    }
}

/// Phase C hook dispatcher: for each system, optionally invoke the
/// `on_initialize_system` hook and collect the resulting per-system
/// override. Systems with no hook (or with a hook that records no planet
/// spawns / attribute overrides) get a default `SystemInitOverride`.
fn run_phase_c_hooks(
    lua: Option<&mlua::Lua>,
    systems: &[EmptySystem],
    capitals: &CapitalAssignments,
    star_types: &[StarTypeDefinition],
) -> Vec<SystemInitOverride> {
    let Some(lua) = lua else {
        return vec![SystemInitOverride::default(); systems.len()];
    };
    let Some(func) =
        galaxy_gen_ctx::last_registered_hook(lua, galaxy_gen_ctx::INITIALIZE_SYSTEM_HANDLERS)
            .ok()
            .flatten()
    else {
        return vec![SystemInitOverride::default(); systems.len()];
    };

    let mut out = Vec::with_capacity(systems.len());
    for (i, sys) in systems.iter().enumerate() {
        let star_type_id = star_types
            .get(sys.star_type_idx)
            .map(|st| st.id.clone())
            .unwrap_or_default();
        let ctx = InitializeSystemCtx::new(
            i + 1,
            sys.name.clone(),
            star_type_id,
            sys.position,
            i == capitals.capital_idx,
        );
        if let Err(e) = func.call::<()>(ctx.clone()) {
            warn!(
                "on_initialize_system hook error (system {}): {e}; using default planets",
                sys.name
            );
            out.push(SystemInitOverride::default());
            continue;
        }
        let actions = ctx.take_actions();
        out.push(convert_initialize_actions(actions));
    }
    out
}

/// Convert Lua-recorded `InitializeSystemActions` into a `SystemInitOverride`
/// consumable by `initialize_systems`. Planet-type ids are resolved later
/// against the planet_types registry in `initialize_systems`.
fn convert_initialize_actions(actions: InitializeSystemActions) -> SystemInitOverride {
    let pending = actions
        .spawned_planets
        .into_iter()
        .map(|p| PendingPlanet {
            name: p.name,
            type_id: p.planet_type,
            attrs: p.attrs,
        })
        .collect();
    SystemInitOverride {
        pending_planets: pending,
        override_planets: actions.override_default_planets,
        name: actions.name,
        surveyed: actions.surveyed,
    }
}

/// Unresolved planet record from the Lua hook, awaiting `type_id` lookup
/// against the planet_types registry in `initialize_systems`.
#[derive(Clone, Debug)]
pub(crate) struct PendingPlanet {
    pub name: String,
    pub type_id: String,
    pub attrs: PlanetAttrsOverride,
}

/// #182: deep-copy the PredefinedSystemRegistry so it can be wrapped in an
/// `Arc` for Lua ctx sharing without holding a Bevy resource borrow.
fn clone_predefined_registry(src: &PredefinedSystemRegistry) -> PredefinedSystemRegistry {
    PredefinedSystemRegistry {
        systems: src.systems.clone(),
    }
}

/// #182: resolve a predefined system's planet list into `PlanetData`. Same
/// shape as `resolve_pending_planets` but consumes the richer
/// `PredefinedPlanetSpec` (which carries the same optional attr fields under
/// a different struct).
fn resolve_predefined_planets(
    predefined: &[PredefinedPlanetSpec],
    planet_types: &[PlanetTypeDefinition],
    star: &StarTypeDefinition,
) -> Vec<PlanetData> {
    let mut out = Vec::with_capacity(predefined.len());
    for p in predefined {
        let Some(type_idx) = planet_types.iter().position(|pt| pt.id == p.planet_type_id) else {
            warn!(
                "predefined_system: unknown planet_type '{}' for planet '{}' — skipping",
                p.planet_type_id, p.name
            );
            continue;
        };
        let pt = &planet_types[type_idx];
        let attrs = SystemAttributes {
            habitability: p
                .attrs
                .habitability
                .unwrap_or((pt.base_habitability + star.habitability_bonus).clamp(0.0, 1.0)),
            mineral_richness: p.attrs.mineral_richness.unwrap_or(0.0),
            energy_potential: p.attrs.energy_potential.unwrap_or(0.0),
            research_potential: p.attrs.research_potential.unwrap_or(0.0),
            max_building_slots: p.attrs.max_building_slots.unwrap_or(pt.base_slots as u8),
        };
        out.push(PlanetData {
            type_idx,
            attrs,
            name_override: Some(p.name.clone()),
        });
    }
    out
}

/// Resolve a list of `PendingPlanet` (from a Lua hook) into `PlanetData`
/// ready for `initialize_systems` to consume.
///
/// Missing attribute fields are filled from the planet type definition's
/// defaults (base_habitability + habitability_bonus from the star, base_slots,
/// zero resource levels). Unknown planet-type ids are skipped with a warning.
fn resolve_pending_planets(
    pending: &[PendingPlanet],
    planet_types: &[PlanetTypeDefinition],
    star: &StarTypeDefinition,
) -> Vec<PlanetData> {
    let mut out = Vec::with_capacity(pending.len());
    for p in pending {
        let Some(type_idx) = planet_types.iter().position(|pt| pt.id == p.type_id) else {
            warn!(
                "on_initialize_system: unknown planet_type '{}' for planet '{}' — skipping",
                p.type_id, p.name
            );
            continue;
        };
        let pt = &planet_types[type_idx];
        let attrs = SystemAttributes {
            habitability: p
                .attrs
                .habitability
                .unwrap_or((pt.base_habitability + star.habitability_bonus).clamp(0.0, 1.0)),
            mineral_richness: p.attrs.mineral_richness.unwrap_or(0.0),
            energy_potential: p.attrs.energy_potential.unwrap_or(0.0),
            research_potential: p.attrs.research_potential.unwrap_or(0.0),
            max_building_slots: p.attrs.max_building_slots.unwrap_or(pt.base_slots as u8),
        };
        out.push(PlanetData {
            type_idx,
            attrs,
            name_override: Some(p.name.clone()),
        });
    }
    out
}

/// #145: Place forbidden regions (nebulae, subspace storms) into the galaxy.
///
/// Runs after `generate_galaxy` (so all star systems exist) and after the
/// region type + spec registries have been loaded. Drains
/// `RegionSpecQueue` and spawns one `ForbiddenRegion` entity per placed region.
///
/// Hard-constrained placement enforces C1 (capital sanctuary), C2 (capital
/// escape), C3 (connectivity) and C4 (no large orphan clusters). Violators
/// are shrunk or dropped — region count is best-effort, not guaranteed.
pub fn place_forbidden_regions(
    mut commands: Commands,
    stars: Query<(&StarSystem, &Position)>,
    region_types: Res<super::region::RegionTypeRegistry>,
    mut region_specs: ResMut<super::region::RegionSpecQueue>,
    rng_seed: Option<Res<crate::observer::RngSeed>>,
) {
    use super::region::{PlacementInputs, place_regions};

    if region_specs.specs.is_empty() {
        return;
    }
    if region_types.types.is_empty() {
        warn!("place_forbidden_regions: specs queued but no region types registered");
        region_specs.specs.clear();
        return;
    }

    // Snapshot system positions + find the capital.
    let mut system_positions: Vec<[f64; 3]> = Vec::new();
    let mut capital_idx = 0;
    for (i, (star, pos)) in stars.iter().enumerate() {
        if star.is_capital {
            capital_idx = i;
        }
        system_positions.push(pos.as_array());
    }
    if system_positions.is_empty() {
        region_specs.specs.clear();
        return;
    }

    // Galaxy radius approximation: max distance from origin.
    let galaxy_radius = system_positions
        .iter()
        .map(|p| (p[0] * p[0] + p[1] * p[1]).sqrt())
        .fold(0.0_f64, f64::max)
        .max(20.0);

    let inputs = PlacementInputs::new(&system_positions, capital_idx, galaxy_radius);
    let specs = std::mem::take(&mut region_specs.specs);
    // Derive a stable-but-distinct RNG for region placement. Using the seed
    // +1 means changing galaxy seeds also shifts region placement, while
    // seeded runs remain deterministic.
    let mut rng: rand::rngs::StdRng = match rng_seed.as_deref().and_then(|s| s.0) {
        Some(seed) => rand::rngs::StdRng::seed_from_u64(seed.wrapping_add(1)),
        None => rand::rngs::StdRng::from_os_rng(),
    };

    let output = place_regions(&mut rng, &inputs, &region_types.types, &specs);
    let count = output.regions.len();

    for region in output.regions {
        commands.spawn(region);
    }

    info!("Placed {} forbidden regions", count);
}
