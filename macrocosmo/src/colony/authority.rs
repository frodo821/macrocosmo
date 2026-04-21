use std::borrow::Cow;
use std::fmt;

use bevy::prelude::*;
use mlua::prelude::*;

use crate::amount::Amt;
use crate::event_system::EventContext;
use crate::faction::{FactionOwner, system_owner};
use crate::galaxy::{AtSystem, Planet, Sovereignty, StarSystem};
use crate::modifier::ModifiedValue;
use crate::ship::Owner;
use crate::time_system::GameClock;

use super::{Colony, LastProductionTick, ResourceCapacity, ResourceStockpile};

/// Default authority produced per hexady by the capital colony.
/// #160: canonical value is `GameBalance.base_authority_per_hexadies`.
pub const BASE_AUTHORITY_PER_HEXADIES: Amt = Amt::units(1);

/// Default authority cost per hexady for each non-capital colony.
/// #160: canonical value is `GameBalance.authority_cost_per_colony`.
pub const AUTHORITY_COST_PER_COLONY: Amt = Amt::new(0, 500);

/// Production efficiency multiplier applied to non-capital colonies when
/// the capital's authority stockpile is depleted.
/// 0.5 as fixed-point: Amt(500) means ×0.500
pub const AUTHORITY_DEFICIT_PENALTY: Amt = Amt::new(0, 500);

/// Configurable authority parameters. Tech effects can push modifiers to
/// adjust authority production or cost scaling.
#[derive(Resource, Component)]
pub struct AuthorityParams {
    /// Authority produced per hexady by the capital colony. Base = 1.0
    pub production: ModifiedValue,
    /// Authority cost per hexady per non-capital colony. Base = 0.5
    pub cost_per_colony: ModifiedValue,
}

impl Default for AuthorityParams {
    fn default() -> Self {
        Self {
            production: ModifiedValue::new(BASE_AUTHORITY_PER_HEXADIES),
            cost_per_colony: ModifiedValue::new(AUTHORITY_COST_PER_COLONY),
        }
    }
}

/// #303 (S-10): After [`update_sovereignty`] detects owner changes and writes
/// them to [`PendingSovereigntyChanges`], this system cascades the new owner
/// to child entities:
///
/// - **Colony:** update `FactionOwner` on each Colony whose planet belongs to
///   the changed system.
/// - **SystemBuildings:** the StarSystem entity itself carries `FactionOwner`;
///   update it to the new sovereign.
/// - **InSystem ships:** only `ShipState::InSystem { system }` ships transfer.
///   In-transit / loitering ships retain their original owner.
/// - **DeepSpaceStructure:** structures `With<AtSystem>` matching the system
///   get their `FactionOwner` updated. (Currently no DSS use `AtSystem`;
///   included for forward-compat.)
///
/// **Abandonment special case:** When `new_owner` is `None`, child entities
/// keep their previous `FactionOwner`. Abandoning a system does not
/// magically transfer infrastructure to nobody -- entities just lose
/// sovereignty protection.
pub fn cascade_sovereignty_changes(
    mut pending: ResMut<PendingSovereigntyChanges>,
    colonies: Query<(Entity, &Colony)>,
    planets: Query<&Planet>,
    mut faction_owners: Query<&mut FactionOwner>,
    mut ships: Query<(&crate::ship::ShipState, Entity, &mut crate::ship::Ship)>,
    dss_at_system: Query<(Entity, &AtSystem), With<crate::deep_space::DeepSpaceStructure>>,
) {
    let changes: Vec<SovereigntyChangedContext> = pending.changes.drain(..).collect();
    for change in &changes {
        let Some(new_faction) = change.new_owner else {
            // Abandonment: leave FactionOwner as-is on children.
            continue;
        };

        let system = change.system;

        // 1. Update FactionOwner on the StarSystem entity itself (SystemBuildings).
        if let Ok(mut fo) = faction_owners.get_mut(system) {
            fo.0 = new_faction;
        }

        // 2. Update FactionOwner on Colony entities whose planet is in the system.
        for (colony_entity, colony) in &colonies {
            if colony.system(&planets) == Some(system) {
                if let Ok(mut fo) = faction_owners.get_mut(colony_entity) {
                    fo.0 = new_faction;
                }
            }
        }

        // 3. Update docked ships: only ShipState::InSystem { system } transfers.
        for (state, ship_entity, mut ship) in &mut ships {
            if let crate::ship::ShipState::InSystem { system: docked_sys } = state {
                if *docked_sys == system {
                    // Dual-write: FactionOwner component + Ship.owner field.
                    if let Ok(mut fo) = faction_owners.get_mut(ship_entity) {
                        fo.0 = new_faction;
                    }
                    ship.owner = Owner::Empire(new_faction);
                }
            }
        }

        // 4. DeepSpaceStructure entities with AtSystem matching the system.
        for (dss_entity, at_sys) in &dss_at_system {
            if at_sys.0 == system {
                if let Ok(mut fo) = faction_owners.get_mut(dss_entity) {
                    fo.0 = new_faction;
                }
            }
        }
    }

    // Re-fill pending so the downstream fire system can read the changes.
    pending.changes = changes;
}

/// #303 (S-10): Fire sovereignty-changed events through `EventSystem` so the
/// standard `dispatch_event_handlers` loop delivers them to Lua handlers.
///
/// Runs **after** [`cascade_sovereignty_changes`] so that Lua handlers
/// observe the post-cascade world state. Queue-only — never calls into Lua
/// directly (see `feedback_rust_no_lua_callback.md`).
pub fn fire_sovereignty_events(
    clock: Res<GameClock>,
    mut pending: ResMut<PendingSovereigntyChanges>,
    mut event_system: ResMut<crate::event_system::EventSystem>,
) {
    for ctx in pending.changes.drain(..) {
        event_system.fire_event_with_payload(Some(ctx.system), clock.elapsed, Box::new(ctx));
    }
}

/// #73: Authority production and empire-scale consumption.
///
/// - The capital colony produces `BASE_AUTHORITY_PER_HEXADIES` authority per hexady.
/// - Each non-capital colony costs `AUTHORITY_COST_PER_COLONY` authority per hexady,
///   deducted from the capital's stockpile.
/// - When the capital's authority reaches 0, non-capital colonies suffer a production
///   efficiency penalty (applied in `tick_production`).
///
/// NOTE: Remote command costs (one-time authority cost when issuing commands to
/// distant colonies) are not implemented here -- they belong in the communication
/// module and will be handled separately.
pub fn tick_authority(
    clock: Res<GameClock>,
    last_tick: Res<LastProductionTick>,
    empire_authority_q: Query<(Entity, &AuthorityParams), With<crate::player::Empire>>,
    colonies: Query<(&Colony, &crate::faction::FactionOwner)>,
    mut stockpiles: Query<(&mut ResourceStockpile, Option<&ResourceCapacity>), With<StarSystem>>,
    stars: Query<&StarSystem>,
    planets: Query<&Planet>,
) {
    let delta = clock.elapsed - last_tick.0;
    if delta <= 0 {
        return;
    }
    let d = delta as u64;

    for (empire_entity, authority_params) in &empire_authority_q {
        // First pass: find capital system and count non-capital colonies for this empire
        let mut capital_system: Option<Entity> = None;
        let mut non_capital_count: u64 = 0;
        for (colony, faction_owner) in &colonies {
            if faction_owner.0 != empire_entity {
                continue;
            }
            if let Some(sys) = colony.system(&planets) {
                if let Ok(star) = stars.get(sys) {
                    if star.is_capital {
                        capital_system = Some(sys);
                    } else {
                        non_capital_count += 1;
                    }
                } else {
                    non_capital_count += 1;
                }
            } else {
                non_capital_count += 1;
            }
        }

        let Some(cap_sys) = capital_system else {
            continue; // No capital found for this empire
        };

        // TODO (#76): Scale authority cost by light-speed distance from capital to each colony.
        // Distant colonies should cost more authority to maintain due to communication delay.
        // This should be its own issue — requires per-colony distance calculation and
        // Position queries which aren't currently available in this system.

        // Produce authority at capital system and deduct empire scale cost
        let auth_production = authority_params.production.final_value();
        let auth_cost_per_colony = authority_params.cost_per_colony.final_value();
        if let Ok((mut stockpile, capacity)) = stockpiles.get_mut(cap_sys) {
            // Capital produces authority
            stockpile.authority = stockpile.authority.add(auth_production.mul_u64(d));

            // Deduct empire scale cost for non-capital colonies
            let scale_cost = auth_cost_per_colony.mul_u64(non_capital_count).mul_u64(d);
            stockpile.authority = stockpile.authority.sub(scale_cost);

            // Clamp authority to capacity
            if let Some(cap) = capacity {
                stockpile.authority = stockpile.authority.min(cap.authority);
            }
        }
    }
}

// =============================================================================
// #303 (S-10): Sovereignty change detection + cascade
// =============================================================================

/// Event id fired when sovereignty of a star system changes. Lua scripts
/// register via `on("macrocosmo:sovereignty_changed", fn)`.
pub const SOVEREIGNTY_CHANGED_EVENT: &str = "macrocosmo:sovereignty_changed";

/// Reason for a sovereignty change. Carried on [`SovereigntyChangedContext`]
/// so Lua handlers can distinguish initial claims from conquests.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(dead_code)] // Cession, Secession reserved for future #305 / secession mechanics
pub enum SovereigntyChangeReason {
    /// Enemy Core deployed / conquered existing Core.
    Conquest,
    /// Diplomatic transfer (future #305).
    Cession,
    /// Core withdrawn / destroyed — owner becomes None.
    Abandonment,
    /// Rebel faction takes over (future).
    Secession,
    /// Game start / first Core deployment in unclaimed system.
    Initial,
}

impl fmt::Display for SovereigntyChangeReason {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = match self {
            Self::Conquest => "conquest",
            Self::Cession => "cession",
            Self::Abandonment => "abandonment",
            Self::Secession => "secession",
            Self::Initial => "initial",
        };
        f.write_str(s)
    }
}

/// Typed [`EventContext`] payload for the `macrocosmo:sovereignty_changed`
/// event. Built by [`update_sovereignty`] when it detects an owner change,
/// then stored on [`PendingSovereigntyChanges`] for the cascade system and
/// eventually forwarded to `EventSystem::fire_event_with_payload`.
#[derive(Clone, Debug)]
pub struct SovereigntyChangedContext {
    pub system: Entity,
    pub system_name: String,
    pub previous_owner: Option<Entity>,
    pub new_owner: Option<Entity>,
    pub reason: SovereigntyChangeReason,
}

impl EventContext for SovereigntyChangedContext {
    fn event_id(&self) -> &str {
        SOVEREIGNTY_CHANGED_EVENT
    }

    fn to_lua_table(&self, lua: &Lua) -> mlua::Result<mlua::Table> {
        let t = lua.create_table()?;
        t.set("event_id", SOVEREIGNTY_CHANGED_EVENT)?;
        t.set("system_id", self.system.to_bits().to_string())?;
        t.set("system_name", self.system_name.as_str())?;
        if let Some(prev) = self.previous_owner {
            t.set("previous_owner_id", prev.to_bits().to_string())?;
        }
        if let Some(new) = self.new_owner {
            t.set("new_owner_id", new.to_bits().to_string())?;
        }
        t.set("reason", self.reason.to_string())?;
        Ok(t)
    }

    fn payload_get(&self, key: &str) -> Option<Cow<'_, str>> {
        match key {
            "system_id" => Some(Cow::Owned(self.system.to_bits().to_string())),
            "system_name" => Some(Cow::Borrowed(&self.system_name)),
            "previous_owner_id" => self
                .previous_owner
                .map(|e| Cow::Owned(e.to_bits().to_string())),
            "new_owner_id" => self.new_owner.map(|e| Cow::Owned(e.to_bits().to_string())),
            "reason" => Some(Cow::Owned(self.reason.to_string())),
            _ => None,
        }
    }
}

/// Resource that queues sovereignty changes detected by [`update_sovereignty`]
/// for the downstream [`cascade_sovereignty_changes`] system. Drained each
/// tick to avoid stale events.
#[derive(Resource, Default)]
pub struct PendingSovereigntyChanges {
    pub changes: Vec<SovereigntyChangedContext>,
}

/// #295 (S-1): Derive sovereignty of each star system from Core ship presence.
///
/// A system is sovereign to `faction` when (and only when) a Core ship owned by
/// `faction` is stationed in that system. Removing the Core ship removes
/// sovereignty — colony presence alone does not confer ownership.
///
/// `Sovereignty.owner` is retained here as a cached derived view so that
/// savebag / existing readers keep working without change. Readers that need
/// live owner data should call [`system_owner`] directly.
///
/// TODO(#298): `control_score` semantics are placeholder (1.0 when owned, 0.0
/// otherwise). Real control-score dynamics (population, garrison, distance
/// decay) come in S-4.
pub fn update_sovereignty(
    mut sovereignties: Query<(Entity, &mut Sovereignty, &StarSystem)>,
    at_system: Query<(&AtSystem, &FactionOwner), With<crate::ship::CoreShip>>,
    mut pending: ResMut<PendingSovereigntyChanges>,
) {
    for (entity, mut sov, star) in &mut sovereignties {
        let prev_owner = sov.owner;
        let new_faction = system_owner(entity, &at_system);
        let new_owner = new_faction.map(Owner::Empire);

        // Write the derived sovereignty regardless.
        match new_faction {
            Some(_) => {
                sov.owner = new_owner;
                sov.control_score = 1.0;
            }
            None => {
                sov.owner = None;
                sov.control_score = 0.0;
            }
        }

        // Detect change: compare previous vs new owner entity.
        let prev_faction = match prev_owner {
            Some(Owner::Empire(e)) => Some(e),
            _ => None,
        };
        if prev_faction == new_faction {
            continue;
        }

        // Determine reason.
        let reason = match (prev_faction, new_faction) {
            (None, Some(_)) => SovereigntyChangeReason::Initial,
            (Some(_), Some(_)) => SovereigntyChangeReason::Conquest,
            (Some(_), None) => SovereigntyChangeReason::Abandonment,
            (None, None) => unreachable!(), // filtered out above
        };

        pending.changes.push(SovereigntyChangedContext {
            system: entity,
            system_name: star.name.clone(),
            previous_owner: prev_faction,
            new_owner: new_faction,
            reason,
        });
    }
}
