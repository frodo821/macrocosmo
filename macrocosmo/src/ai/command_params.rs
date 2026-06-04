//! Typed accessors for AI command parameter maps.
//!
//! The AI bus carries engine-agnostic [`macrocosmo_ai::CommandValue`]
//! values keyed by schema strings. This module is the single place in
//! `macrocosmo` that translates those wire keys into Bevy-side entities and
//! typed values for command consumers.

use bevy::prelude::Entity;

use macrocosmo_ai::{CommandParams, CommandValue};

use crate::ai::convert::{from_ai_entity, from_ai_system};

pub(crate) const BUILDING_ID: &str = "building_id";
pub(crate) const DEFINITION_ID: &str = "definition_id";
pub(crate) const DESIGN_ID: &str = "design_id";
pub(crate) const SHIP_COUNT: &str = "ship_count";
pub(crate) const TARGET_SYSTEM: &str = "target_system";
pub(crate) const TECH_ID: &str = "tech_id";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CommandParamError {
    MissingOrWrongType { key: &'static str },
}

pub(crate) fn required_str<'a>(
    params: &'a CommandParams,
    key: &'static str,
) -> Result<&'a str, CommandParamError> {
    match params.get(key) {
        Some(CommandValue::Str(value)) => Ok(value),
        _ => Err(CommandParamError::MissingOrWrongType { key }),
    }
}

pub(crate) fn optional_system(params: &CommandParams, key: &'static str) -> Option<Entity> {
    match params.get(key) {
        Some(CommandValue::System(system_ref)) => Some(from_ai_system(*system_ref)),
        _ => None,
    }
}

pub(crate) fn target_system(params: &CommandParams) -> Option<Entity> {
    optional_system(params, TARGET_SYSTEM)
}

/// Extract ship entity list from indexed command params (`ship_count`,
/// `ship_0`, `ship_1`, ...).
pub(crate) fn ship_list(params: &CommandParams) -> Vec<Entity> {
    let count = match params.get(SHIP_COUNT) {
        Some(CommandValue::I64(n)) => *n as usize,
        _ => return Vec::new(),
    };
    (0..count)
        .filter_map(|i| {
            let key = format!("ship_{i}");
            match params.get(key.as_str()) {
                Some(CommandValue::Entity(entity_ref)) => Some(from_ai_entity(*entity_ref)),
                _ => None,
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use bevy::ecs::world::World;
    use macrocosmo_ai::CommandValue;

    use super::*;
    use crate::ai::convert::{to_ai_entity, to_ai_system};

    fn key(value: &'static str) -> Arc<str> {
        Arc::from(value)
    }

    #[test]
    fn required_str_rejects_missing_or_wrong_type() {
        let mut params = CommandParams::default();
        assert_eq!(
            required_str(&params, DESIGN_ID),
            Err(CommandParamError::MissingOrWrongType { key: DESIGN_ID })
        );

        params.insert(key(DESIGN_ID), CommandValue::I64(7));
        assert_eq!(
            required_str(&params, DESIGN_ID),
            Err(CommandParamError::MissingOrWrongType { key: DESIGN_ID })
        );

        params.insert(key(DESIGN_ID), CommandValue::from("scout"));
        assert_eq!(required_str(&params, DESIGN_ID), Ok("scout"));
    }

    #[test]
    fn target_system_converts_system_ref() {
        let mut world = World::new();
        let system = world.spawn_empty().id();
        let mut params = CommandParams::default();
        params.insert(
            key(TARGET_SYSTEM),
            CommandValue::System(to_ai_system(system)),
        );

        assert_eq!(target_system(&params), Some(system));
    }

    #[test]
    fn ship_list_reads_indexed_entity_params() {
        let mut world = World::new();
        let first = world.spawn_empty().id();
        let second = world.spawn_empty().id();
        let mut params = CommandParams::default();
        params.insert(key(SHIP_COUNT), CommandValue::I64(3));
        params.insert(key("ship_0"), CommandValue::Entity(to_ai_entity(first)));
        params.insert(key("ship_1"), CommandValue::Bool(false));
        params.insert(key("ship_2"), CommandValue::Entity(to_ai_entity(second)));

        assert_eq!(ship_list(&params), vec![first, second]);
    }
}
