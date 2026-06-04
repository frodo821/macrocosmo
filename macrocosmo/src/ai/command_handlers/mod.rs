//! AI command handlers invoked by `command_consumer`.

pub(crate) mod build;
pub(crate) mod military;
pub(crate) mod research;

use bevy::prelude::*;

use crate::ai::convert::to_ai_faction;
use crate::player::{Empire, Faction};

pub(super) fn find_empire_entity(
    issuer: &macrocosmo_ai::FactionId,
    empires: &Query<(Entity, &Faction), With<Empire>>,
) -> Option<Entity> {
    empires
        .iter()
        .find_map(|(entity, _faction)| (to_ai_faction(entity) == *issuer).then_some(entity))
}
