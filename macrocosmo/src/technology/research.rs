use std::collections::HashSet;

use bevy::prelude::*;

use crate::amount::Amt;
use crate::colony::{Colony, Production, ProductionFocus};
use crate::components::Position;
use crate::faction::FactionOwner;
use crate::galaxy::{HomeSystem, StarSystem};
use crate::physics;
use crate::time_system::GameClock;

use super::tree::{TechId, TechTree};

/// Current research target and accumulated points.
#[derive(Resource, Component, Default, Reflect)]
#[reflect(Component, Resource)]
pub struct ResearchQueue {
    pub current: Option<TechId>,
    pub accumulated: f64,
    pub blocked: bool,
}

impl ResearchQueue {
    pub fn start_research(&mut self, tech_id: TechId) {
        self.current = Some(tech_id);
        self.accumulated = 0.0;
        self.blocked = false;
    }

    pub fn cancel_research(&mut self) {
        self.current = None;
        self.accumulated = 0.0;
    }

    pub fn block(&mut self) {
        self.blocked = true;
    }

    pub fn unblock(&mut self) {
        self.blocked = false;
    }

    pub fn add_progress(&mut self, amount: f64) {
        self.accumulated += amount;
    }
}

/// Global research points pool (accumulated from colonies).
#[derive(Resource, Component, Default, Reflect)]
#[reflect(Component, Resource)]
pub struct ResearchPool {
    pub points: f64,
}

/// Tracks the last game tick at which research was collected, to compute delta.
#[derive(Resource, Reflect)]
#[reflect(Resource)]
pub struct LastResearchTick(pub i64);

/// A research packet in transit from a colony to the capital at light speed.
///
/// `owner` is the empire entity that produced the research; `receive_research`
/// uses it to credit only that empire's `ResearchPool`, preventing one empire's
/// colony output from leaking into other empires' pools (#458).
#[derive(Component, Reflect)]
#[reflect(Component)]
pub struct PendingResearch {
    pub owner: Entity,
    pub amount: f64,
    pub arrives_at: i64,
}

/// Tracks which technologies a star system "knows about".
/// Tech effects only apply to colonies in systems that have received the knowledge.
#[derive(Component, Default, Debug, Reflect)]
#[reflect(Component)]
pub struct TechKnowledge {
    pub known_techs: HashSet<TechId>,
}

/// Techs that were just researched this tick, to be propagated to systems.
#[derive(Resource, Component, Default, Reflect)]
#[reflect(Component, Resource)]
pub struct RecentlyResearched {
    pub techs: Vec<TechId>,
}

/// A technology propagating from the capital to a target system at light speed.
///
/// `owner` is the empire entity that researched the tech; the propagation is
/// scoped to that empire's territory (#458 — parallel to `PendingResearch`).
#[derive(Component, Reflect)]
#[reflect(Component)]
pub struct PendingKnowledgePropagation {
    pub owner: Entity,
    pub tech_id: TechId,
    pub target_system: Entity,
    pub arrives_at: i64,
}

/// Each tick, colonies emit research points as `PendingResearch` entities that
/// travel at light speed to the **owner empire's** capital (`HomeSystem`).
/// Same-system colonies contribute instantly.
///
/// The packet carries `owner` so `receive_research` credits only that empire's
/// `ResearchPool`. Colonies without `FactionOwner`, or empires without a
/// resolvable `HomeSystem`, are skipped (no packet emitted) — their research is
/// effectively dropped, which is preferable to leaking points across empires.
pub fn emit_research(
    mut commands: Commands,
    clock: Res<GameClock>,
    last_tick: Res<LastResearchTick>,
    colonies: Query<(
        &Colony,
        &Production,
        Option<&ProductionFocus>,
        &FactionOwner,
    )>,
    home_systems: Query<&HomeSystem>,
    positions: Query<&Position>,
    planets: Query<&crate::galaxy::Planet>,
) {
    let delta = clock.elapsed - last_tick.0;
    if delta <= 0 {
        return;
    }
    let d = delta as f64;

    for (colony, prod, focus, owner) in &colonies {
        let rw = match focus {
            Some(f) => f.research_weight,
            None => Amt::units(1),
        };
        let d_amt = Amt::units(d as u64);
        // Building bonuses are already included via modifiers on Production
        let amount = prod
            .research_per_hexadies
            .final_value()
            .mul_amt(rw)
            .mul_amt(d_amt)
            .to_f64();
        if amount <= 0.0 {
            continue;
        }

        // Owner empire reference system (capital). Without it we can't
        // anchor the light-delay calculation, so the packet is skipped.
        let Ok(home) = home_systems.get(owner.0) else {
            continue;
        };
        let capital_system = home.0;

        let colony_sys = colony.system(&planets);

        // Calculate light delay from colony to capital
        let delay = if colony_sys == Some(capital_system) {
            0
        } else if let (Some(sys), Ok(cap_pos)) = (colony_sys, positions.get(capital_system)) {
            if let Ok(colony_pos) = positions.get(sys) {
                let dist = physics::distance_ly(colony_pos, cap_pos);
                physics::light_delay_hexadies(dist)
            } else {
                0
            }
        } else {
            0
        };

        commands.spawn(PendingResearch {
            owner: owner.0,
            amount,
            arrives_at: clock.elapsed + delay,
        });
    }
}

/// Receives PendingResearch entities that have arrived and adds them to the
/// pool of the **owning** empire only. Packets whose owner empire was
/// despawned (or otherwise lacks a `ResearchPool`) are dropped with a warning.
pub fn receive_research(
    mut commands: Commands,
    clock: Res<GameClock>,
    mut empire_q: Query<&mut ResearchPool, With<crate::player::Empire>>,
    pending: Query<(Entity, &PendingResearch)>,
) {
    for (entity, pr) in pending.iter() {
        if clock.elapsed < pr.arrives_at {
            continue;
        }
        match empire_q.get_mut(pr.owner) {
            Ok(mut pool) => pool.points += pr.amount,
            Err(_) => {
                warn!(
                    "PendingResearch arrived for missing empire {:?}; dropping {} points",
                    pr.owner, pr.amount
                );
            }
        }
        commands.entity(entity).despawn();
    }
}

/// Processes research each tick: transfers points from pool to current project.
/// When research completes, the tech is marked as researched. The on_researched
/// Lua callback will be invoked separately by the scripting system.
pub fn tick_research(
    clock: Res<GameClock>,
    mut last_tick: ResMut<LastResearchTick>,
    mut empire_q: Query<
        (
            &mut TechTree,
            &mut ResearchQueue,
            &mut ResearchPool,
            &mut RecentlyResearched,
        ),
        With<crate::player::Empire>,
    >,
) {
    let delta = clock.elapsed - last_tick.0;
    if delta <= 0 {
        return;
    }
    last_tick.0 = clock.elapsed;

    for (mut tech_tree, mut queue, mut pool, mut recently_researched) in &mut empire_q {
        let Some(ref current_tech_id) = queue.current else {
            continue;
        };

        // Skip progress if research is blocked
        if queue.blocked {
            continue;
        }

        let current_tech_id = current_tech_id.clone();

        let research_cost = {
            let Some(tech) = tech_tree.technologies.get(&current_tech_id) else {
                queue.current = None;
                continue;
            };
            tech.cost.research.to_f64()
        };

        // Transfer available research points from pool
        let needed = research_cost - queue.accumulated;
        if needed > 0.0 {
            let transfer = pool.points.min(needed);
            if transfer > 0.0 {
                pool.points -= transfer;
                queue.accumulated += transfer;
            }
        }

        // Check completion
        if queue.accumulated >= research_cost {
            let tech_name = tech_tree
                .technologies
                .get(&current_tech_id)
                .map(|t| t.name.clone())
                .unwrap_or_default();

            tech_tree.complete_research(current_tech_id.clone());
            recently_researched.techs.push(current_tech_id);

            queue.current = None;
            queue.accumulated = 0.0;
            info!("Research complete: {}", tech_name);
        }
    }
}

/// Flush unused research points at the end of each tick (use it or lose it).
pub fn flush_research(mut empire_q: Query<&mut ResearchPool, With<crate::player::Empire>>) {
    for mut pool in &mut empire_q {
        pool.points = 0.0;
    }
}

/// When techs are recently researched, propagate knowledge to all colonized systems.
/// The capital gets the tech immediately; remote colonies receive it after light delay.
pub fn propagate_tech_knowledge(
    mut commands: Commands,
    clock: Res<GameClock>,
    mut empire_q: Query<(Entity, &mut RecentlyResearched), With<crate::player::Empire>>,
    colonies: Query<&Colony>,
    stars: Query<(Entity, &StarSystem, &Position)>,
    mut tech_knowledge: Query<&mut TechKnowledge>,
    planets: Query<&crate::galaxy::Planet>,
) {
    for (empire_entity, mut recently_researched) in &mut empire_q {
        if recently_researched.techs.is_empty() {
            continue;
        }

        // Find capital system
        // TODO(#418): capital should be per-empire, not a global flag on StarSystem.
        // For #458 we just stamp `owner` on the spawned packet so the field is
        // available; per-empire capital resolution is a separate fix.
        let capital = stars.iter().find(|(_, s, _)| s.is_capital);
        let Some((capital_entity, _, capital_pos)) = capital else {
            recently_researched.techs.clear();
            continue;
        };
        let capital_pos = *capital_pos;

        // Collect colonized system entities
        let colonized_systems: HashSet<Entity> =
            colonies.iter().filter_map(|c| c.system(&planets)).collect();

        for tech_id in recently_researched.techs.drain(..) {
            // Capital gets it immediately
            if let Ok(mut knowledge) = tech_knowledge.get_mut(capital_entity) {
                knowledge.known_techs.insert(tech_id.clone());
            }

            // Other colonized systems get it after light delay
            for (sys_entity, _, sys_pos) in stars.iter() {
                if sys_entity == capital_entity {
                    continue;
                }
                if !colonized_systems.contains(&sys_entity) {
                    continue;
                }
                let distance = physics::distance_ly(&capital_pos, sys_pos);
                let delay = physics::light_delay_hexadies(distance);
                commands.spawn(PendingKnowledgePropagation {
                    owner: empire_entity,
                    tech_id: tech_id.clone(),
                    target_system: sys_entity,
                    arrives_at: clock.elapsed + delay,
                });
            }
        }
    }
}

/// Receive pending knowledge propagations that have arrived.
pub fn receive_tech_knowledge(
    mut commands: Commands,
    clock: Res<GameClock>,
    pending: Query<(Entity, &PendingKnowledgePropagation)>,
    mut tech_knowledge: Query<&mut TechKnowledge>,
) {
    for (entity, prop) in pending.iter() {
        if clock.elapsed >= prop.arrives_at {
            if let Ok(mut knowledge) = tech_knowledge.get_mut(prop.target_system) {
                knowledge.known_techs.insert(prop.tech_id.clone());
            }
            commands.entity(entity).despawn();
        }
    }
}
