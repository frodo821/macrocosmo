use bevy::prelude::*;
use rand::Rng;

use crate::events::{GameEvent, GameEventKind};
use crate::galaxy::{Anomalies, Anomaly, ResourceLevel, SystemAttributes};

use super::{Ship, ShipHitpoints};

/// Result of an exploration event rolled when a survey completes.
#[derive(Clone, Debug)]
pub enum ExplorationEvent {
    ResourceBonus { resource: String, old_level: String, new_level: String },
    AncientRuins { research_bonus: f64 },
    Danger { description: String },
    Special { description: String },
    Nothing,
}

/// Roll a random exploration event.
///
/// Probabilities: 60% Nothing, 15% ResourceBonus, 10% AncientRuins, 10% Danger, 5% Special.
pub fn roll_exploration_event(rng: &mut impl Rng) -> ExplorationEvent {
    let roll: f64 = rng.random_range(0.0..1.0);
    if roll < 0.60 {
        ExplorationEvent::Nothing
    } else if roll < 0.75 {
        ExplorationEvent::ResourceBonus {
            resource: String::new(),
            old_level: String::new(),
            new_level: String::new(),
        }
    } else if roll < 0.85 {
        ExplorationEvent::AncientRuins { research_bonus: 0.0 }
    } else if roll < 0.95 {
        ExplorationEvent::Danger { description: String::new() }
    } else {
        ExplorationEvent::Special { description: String::new() }
    }
}

/// Attempt to upgrade a ResourceLevel one tier.
/// Returns the new level, or None if already Rich.
pub fn upgrade_resource_level(level: ResourceLevel) -> Option<ResourceLevel> {
    match level {
        ResourceLevel::None => Some(ResourceLevel::Poor),
        ResourceLevel::Poor => Some(ResourceLevel::Moderate),
        ResourceLevel::Moderate => Some(ResourceLevel::Rich),
        ResourceLevel::Rich => None,
    }
}

pub(crate) fn resource_level_name(level: ResourceLevel) -> &'static str {
    match level {
        ResourceLevel::Rich => "Rich",
        ResourceLevel::Moderate => "Moderate",
        ResourceLevel::Poor => "Poor",
        ResourceLevel::None => "None",
    }
}

/// Apply an exploration event's effects and log it.
pub(crate) fn apply_exploration_event(
    event: &ExplorationEvent,
    system_name: &str,
    ship: &Ship,
    ship_hp: &mut ShipHitpoints,
    attrs: Option<Mut<SystemAttributes>>,
    rng: &mut impl Rng,
    timestamp: i64,
    target_system: Entity,
    events: &mut MessageWriter<GameEvent>,
) {
    match event {
        ExplorationEvent::Nothing => {}
        ExplorationEvent::ResourceBonus { .. } => {
            if let Some(mut attrs) = attrs {
                let field = rng.random_range(0u8..3);
                let (name, old_level) = match field {
                    0 => ("minerals", attrs.mineral_richness),
                    1 => ("energy", attrs.energy_potential),
                    _ => ("research", attrs.research_potential),
                };

                if let Some(new_level) = upgrade_resource_level(old_level) {
                    match field {
                        0 => attrs.mineral_richness = new_level,
                        1 => attrs.energy_potential = new_level,
                        _ => attrs.research_potential = new_level,
                    }
                    events.write(GameEvent {
                        timestamp,
                        kind: GameEventKind::SurveyDiscovery,
                        description: format!(
                            "Survey of {} discovered rich {} deposits! {} -> {}",
                            system_name,
                            name,
                            resource_level_name(old_level),
                            resource_level_name(new_level),
                        ),
                        related_system: Some(target_system),
                    });
                } else {
                    events.write(GameEvent {
                        timestamp,
                        kind: GameEventKind::SurveyDiscovery,
                        description: format!(
                            "Survey of {} found {} deposits already at maximum level",
                            system_name, name,
                        ),
                        related_system: Some(target_system),
                    });
                }
            }
        }
        ExplorationEvent::AncientRuins { .. } => {
            let bonus = rng.random_range(50.0..200.0);
            events.write(GameEvent {
                timestamp,
                kind: GameEventKind::SurveyDiscovery,
                description: format!(
                    "Ancient ruins discovered at {}! Research bonus: {:.0} RP",
                    system_name, bonus,
                ),
                related_system: Some(target_system),
            });
        }
        ExplorationEvent::Danger { .. } => {
            let damage_pct = rng.random_range(0.20..0.50);
            let damage = ship_hp.hull_max * damage_pct;
            ship_hp.hull = (ship_hp.hull - damage).max(1.0);
            events.write(GameEvent {
                timestamp,
                kind: GameEventKind::SurveyDiscovery,
                description: format!(
                    "Danger at {}! Ship {} took {:.0} damage ({:.0}% hull) from hazardous anomaly",
                    system_name, ship.name, damage, damage_pct * 100.0,
                ),
                related_system: Some(target_system),
            });
        }
        ExplorationEvent::Special { .. } => {
            if let Some(mut attrs) = attrs {
                let extra_slots = rng.random_range(1u8..=2);
                attrs.max_building_slots += extra_slots;
                events.write(GameEvent {
                    timestamp,
                    kind: GameEventKind::SurveyDiscovery,
                    description: format!(
                        "Special discovery at {}! Found {} additional building site(s)",
                        system_name, extra_slots,
                    ),
                    related_system: Some(target_system),
                });
            }
        }
    }
}

/// #127: Roll for anomaly discovery using the AnomalyRegistry, apply effects,
/// record in Anomalies component, and fire events. Falls back to legacy
/// ExplorationEvent if no anomaly registry is available.
/// Returns the anomaly ID if one was discovered (for deferred delivery via SurveyData).
pub(crate) fn roll_and_apply_anomaly(
    anomaly_registry: &Option<Res<crate::scripting::anomaly_api::AnomalyRegistry>>,
    rng: &mut impl Rng,
    system_name: &str,
    ship: &Ship,
    ship_hp: &mut ShipHitpoints,
    mut attrs: Option<Mut<SystemAttributes>>,
    anomalies: Option<Mut<Anomalies>>,
    timestamp: i64,
    target_system: Entity,
    events: &mut MessageWriter<GameEvent>,
) -> Option<String> {
    use crate::scripting::anomaly_api::AnomalyEffectDef;

    if let Some(registry) = anomaly_registry {
        if let Some(anomaly_def) = registry.roll_discovery(rng) {
            let anomaly_id = anomaly_def.id.clone();
            let anomaly_name = anomaly_def.name.clone();
            let anomaly_desc = anomaly_def.description.clone();

            // Record in Anomalies component
            if let Some(mut anomalies) = anomalies {
                anomalies.discoveries.push(Anomaly {
                    id: anomaly_id.clone(),
                    name: anomaly_name.clone(),
                    description: anomaly_desc.clone(),
                    discovered_at: timestamp,
                });
            }

            // Apply effects
            for effect in &anomaly_def.effects {
                match effect {
                    AnomalyEffectDef::ResourceBonus { resource } => {
                        if let Some(ref mut attrs) = attrs {
                            let (name, old_level) = match resource.as_str() {
                                "minerals" => ("minerals", attrs.mineral_richness),
                                "energy" => ("energy", attrs.energy_potential),
                                _ => ("research", attrs.research_potential),
                            };
                            if let Some(new_level) = upgrade_resource_level(old_level) {
                                match resource.as_str() {
                                    "minerals" => attrs.mineral_richness = new_level,
                                    "energy" => attrs.energy_potential = new_level,
                                    _ => attrs.research_potential = new_level,
                                }
                                events.write(GameEvent {
                                    timestamp,
                                    kind: GameEventKind::AnomalyDiscovered,
                                    description: format!(
                                        "{}: {} — {} deposits upgraded ({} -> {})",
                                        system_name, anomaly_name, name,
                                        resource_level_name(old_level),
                                        resource_level_name(new_level),
                                    ),
                                    related_system: Some(target_system),
                                });
                            }
                        }
                    }
                    AnomalyEffectDef::ResearchBonus { amount } => {
                        events.write(GameEvent {
                            timestamp,
                            kind: GameEventKind::AnomalyDiscovered,
                            description: format!(
                                "{}: {} — Research bonus: {:.0} RP",
                                system_name, anomaly_name, amount,
                            ),
                            related_system: Some(target_system),
                        });
                    }
                    AnomalyEffectDef::BuildingSlots { extra } => {
                        if let Some(ref mut attrs) = attrs {
                            attrs.max_building_slots += extra;
                            events.write(GameEvent {
                                timestamp,
                                kind: GameEventKind::AnomalyDiscovered,
                                description: format!(
                                    "{}: {} — {} additional building site(s)",
                                    system_name, anomaly_name, extra,
                                ),
                                related_system: Some(target_system),
                            });
                        }
                    }
                    AnomalyEffectDef::Hazard { damage_percent } => {
                        let damage_frac = damage_percent / 100.0;
                        let damage = ship_hp.hull_max * damage_frac;
                        ship_hp.hull = (ship_hp.hull - damage).max(1.0);
                        events.write(GameEvent {
                            timestamp,
                            kind: GameEventKind::AnomalyDiscovered,
                            description: format!(
                                "{}: {} — Ship {} took {:.0} damage ({:.0}% hull)",
                                system_name, anomaly_name, ship.name, damage, damage_percent,
                            ),
                            related_system: Some(target_system),
                        });
                    }
                }
            }

            return Some(anomaly_id);
        }
        // No anomaly discovered
        return None;
    }

    // Fallback: no anomaly registry available, use legacy exploration events
    let event = roll_exploration_event(rng);
    apply_exploration_event(&event, system_name, ship, ship_hp, attrs, rng, timestamp, target_system, events);
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::galaxy::ResourceLevel;

    #[test]
    fn test_roll_exploration_event_does_not_panic() {
        let mut rng = rand::rng();
        for _ in 0..1000 {
            let event = roll_exploration_event(&mut rng);
            match event {
                ExplorationEvent::Nothing
                | ExplorationEvent::ResourceBonus { .. }
                | ExplorationEvent::AncientRuins { .. }
                | ExplorationEvent::Danger { .. }
                | ExplorationEvent::Special { .. } => {}
            }
        }
    }

    #[test]
    fn test_upgrade_resource_level() {
        assert_eq!(upgrade_resource_level(ResourceLevel::None), Some(ResourceLevel::Poor));
        assert_eq!(upgrade_resource_level(ResourceLevel::Poor), Some(ResourceLevel::Moderate));
        assert_eq!(upgrade_resource_level(ResourceLevel::Moderate), Some(ResourceLevel::Rich));
        assert_eq!(upgrade_resource_level(ResourceLevel::Rich), None);
    }

    #[test]
    fn test_resource_level_name() {
        assert_eq!(resource_level_name(ResourceLevel::Rich), "Rich");
        assert_eq!(resource_level_name(ResourceLevel::Moderate), "Moderate");
        assert_eq!(resource_level_name(ResourceLevel::Poor), "Poor");
        assert_eq!(resource_level_name(ResourceLevel::None), "None");
    }

    #[test]
    fn test_roll_distribution_roughly_correct() {
        let mut rng = rand::rng();
        let mut nothing = 0u32;
        let mut resource = 0u32;
        let mut ruins = 0u32;
        let mut danger = 0u32;
        let mut special = 0u32;

        let n = 10_000;
        for _ in 0..n {
            match roll_exploration_event(&mut rng) {
                ExplorationEvent::Nothing => nothing += 1,
                ExplorationEvent::ResourceBonus { .. } => resource += 1,
                ExplorationEvent::AncientRuins { .. } => ruins += 1,
                ExplorationEvent::Danger { .. } => danger += 1,
                ExplorationEvent::Special { .. } => special += 1,
            }
        }

        assert!(nothing > 0, "Nothing should appear");
        assert!(resource > 0, "ResourceBonus should appear");
        assert!(ruins > 0, "AncientRuins should appear");
        assert!(danger > 0, "Danger should appear");
        assert!(special > 0, "Special should appear");

        assert!(nothing > resource, "Nothing should be more common than ResourceBonus");
        assert!(nothing > ruins, "Nothing should be more common than AncientRuins");
    }
}
