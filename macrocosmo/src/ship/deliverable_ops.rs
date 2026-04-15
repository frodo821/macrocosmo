//! #223: Deliverable-side helpers.
//!
//! #334 Phase 2: the per-tick `process_deliverable_commands` loop has been
//! retired. All four deliverable command variants (`LoadDeliverable`,
//! `DeployDeliverable`, `TransferToStructure`, `LoadFromScrapyard`) are now
//! processed by the event-driven `handlers::deliverable_handler` pipeline.
//!
//! This module retains:
//! - [`DEPLOY_POSITION_EPSILON`] shared constant for co-location checks;
//! - [`dismantle_structure`] helper used by build-queue teardown.

use bevy::prelude::*;

use crate::amount::Amt;
use crate::deep_space::{
    ConstructionPlatform, DeepSpaceStructure, LifetimeCost, ResourceCost, Scrapyard,
    StructureRegistry,
};

/// Maximum position delta (in light-years) for a ship to be considered
/// "co-located" with a deep-space structure or deploy coordinate.
pub const DEPLOY_POSITION_EPSILON: f64 = 0.01;

/// #223: Dismantle a deep-space structure. Removes any existing
/// `ConstructionPlatform` (lost investment) and installs a `Scrapyard` whose
/// `remaining = lifetime_cost * scrap_refund`.
pub fn dismantle_structure(
    world: &mut World,
    structure: Entity,
) -> Result<(), &'static str> {
    // Gather what we need without the registry mutably borrowed.
    let (def_id, lifetime) = {
        let Some(ds) = world.get::<DeepSpaceStructure>(structure) else {
            return Err("entity is not a DeepSpaceStructure");
        };
        let lifetime = world
            .get::<LifetimeCost>(structure)
            .map(|lc| lc.0.clone())
            .unwrap_or_default();
        (ds.definition_id.clone(), lifetime)
    };
    let refund = {
        let registry = world.resource::<StructureRegistry>();
        registry
            .get(&def_id)
            .and_then(|d| d.deliverable.as_ref().map(|m| m.scrap_refund))
            .unwrap_or(0.0)
    };
    let remaining = lifetime.scale(refund);
    // Remove markers and install Scrapyard.
    world.entity_mut(structure).remove::<ConstructionPlatform>();
    world.entity_mut(structure).insert(Scrapyard {
        remaining,
        original_definition_id: def_id,
    });
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resource_cost_helpers() {
        let a = ResourceCost {
            minerals: Amt::units(100),
            energy: Amt::units(50),
        };
        assert!(!a.is_zero());
        let half = a.scale(0.5);
        assert_eq!(half.minerals, Amt::units(50));
        assert_eq!(half.energy, Amt::units(25));

        let zero = a.scale(0.0);
        assert!(zero.is_zero());

        assert!(a.covers(&half));
        assert!(!half.covers(&a));
    }
}
