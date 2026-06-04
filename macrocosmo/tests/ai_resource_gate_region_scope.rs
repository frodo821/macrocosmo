//! PR #531 Codex review fold-in regression tests — multi-region
//! scoping of the AI resource gate.
//!
//! Two scoping bugs were flagged by reviewer (frodo821) on the
//! `fix/hotfix-3-resource-gate` branch:
//!
//! 1. **Pending subtraction not region-scoped.** `npc_decision_tick`
//!    sums `current_minerals/energy` from the MidAgent's
//!    `member_systems`, but pre-fold it then walked **every** colony
//!    `BuildQueue` owned by the empire and subtracted those pending
//!    orders from the per-region stockpile sum. With multiple regions,
//!    a pending ship in region B would erroneously reduce region A's
//!    headroom and could incorrectly block Rule 6 / Rule 3.5 / shipyard
//!    decisions.
//!
//! 2. **`ShortAgentTickInputs` keyed by empire, not region.** The
//!    scratch map populated each tick was keyed by empire entity, so a
//!    multi-MidAgent empire saw later MidAgent inserts overwrite
//!    earlier ones; `run_short_agents` then read whichever region was
//!    inserted last, gating colony ShortAgents against another
//!    region's stockpile.
//!
//! These tests pin the per-region invariants after the fix:
//! * `colony_pending_outside_region_not_subtracted_from_other_region`
//! * `multi_midagent_inputs_keyed_by_region_not_overwritten`
//!
//! The 2-region setup mirrors `ai_short_agent_per_region_routing.rs`
//! (manual `Region` + `MidAgent` splice — the production spawn pipeline
//! only emits one Region per empire today).

mod common;

use bevy::prelude::*;

use macrocosmo::ai::AiPlayerMode;
use macrocosmo::ai::npc_decision::ShortAgentTickInputs;
use macrocosmo::ai::{MidAgent, core::MidTermState};
use macrocosmo::colony::ResourceStockpile;
use macrocosmo::colony::building_queue::{BuildKind, BuildOrder, BuildQueue};
use macrocosmo::faction::FactionOwner;
use macrocosmo::galaxy::HomeSystem;
use macrocosmo::knowledge::{
    KnowledgeStore, ObservationSource, SystemKnowledge, SystemSnapshot, SystemVisibilityMap,
    SystemVisibilityTier,
};
use macrocosmo::player::{Empire, Faction, PlayerEmpire};
use macrocosmo::region::{Region, RegionMembership, RegionRegistry, spawn_initial_region};
use macrocosmo_core::amount::Amt;

use common::{advance_time, spawn_test_colony, spawn_test_ruler, spawn_test_system, test_app};

/// Layout of the synthetic 2-region empire used by both tests.
#[derive(Debug, Clone, Copy)]
struct TwoRegionLayout {
    empire: Entity,
    region_a: Entity,
    region_b: Entity,
    home_a: Entity,
    home_b: Entity,
    colony_a: Entity,
    colony_b: Entity,
}

/// Build a single empire with **two** distinct regions, each containing
/// one colonised system. Resource stockpiles + KnowledgeStore are
/// preseeded so `npc_decision_tick` produces a `RegionShortInputs` row
/// for each MidAgent.
///
/// Cost-control note: we set `AiPlayerMode(true)` and tag the empire
/// `PlayerEmpire` so `mark_player_ai_controlled` flips `AiControlled`
/// for us — without that the MidAgent loop in `npc_decision_tick`
/// skips the empire (`auto_managed = false` for un-AI player empires).
fn build_two_region_layout(app: &mut App) -> TwoRegionLayout {
    app.insert_resource(AiPlayerMode(true));
    let world = app.world_mut();
    if world.get_resource::<RegionRegistry>().is_none() {
        world.insert_resource(RegionRegistry::default());
    }

    let empire = world
        .spawn((
            Empire {
                name: "MultiRegion".into(),
            },
            PlayerEmpire,
            Faction {
                id: "multi_region".into(),
                name: "MultiRegion".into(),
                can_diplomacy: false,
                allowed_diplomatic_options: Default::default(),
            },
            KnowledgeStore::default(),
            SystemVisibilityMap::default(),
        ))
        .id();

    let home_a = spawn_test_system(world, "HomeA", [0.0, 0.0, 0.0], 1.0, true, true);
    let home_b = spawn_test_system(world, "HomeB", [100.0, 0.0, 0.0], 1.0, true, true);
    world.entity_mut(empire).insert(HomeSystem(home_a));

    spawn_test_ruler(world, empire, home_a);

    // Mark both systems Surveyed in the empire's vis map + record their
    // snapshot in KnowledgeStore so the MidAgent rule pipeline sees
    // them as catalogued (otherwise `colonizable_systems` collapses).
    {
        let mut em = world.entity_mut(empire);
        let mut vis = em.get_mut::<SystemVisibilityMap>().unwrap();
        for sys in [home_a, home_b] {
            vis.set(sys, SystemVisibilityTier::Surveyed);
        }
    }
    {
        let mut em = world.entity_mut(empire);
        let mut store = em.get_mut::<KnowledgeStore>().unwrap();
        let mut record = |sys: Entity, name: &str, pos: [f64; 3]| {
            store.update(SystemKnowledge {
                system: sys,
                observed_at: 0,
                received_at: 0,
                data: SystemSnapshot {
                    name: name.into(),
                    position: pos,
                    surveyed: true,
                    colonized: true,
                    ..Default::default()
                },
                source: ObservationSource::Direct,
            });
        };
        record(home_a, "HomeA", [0.0, 0.0, 0.0]);
        record(home_b, "HomeB", [100.0, 0.0, 0.0]);
    }

    // Region A: spawned via the production helper (member = home_a only).
    let region_a = spawn_initial_region(world, empire, home_a);

    // Region B: hand-spawned (multi-region splits are not yet emitted
    // by the production spawn pipeline; matches the pattern used in
    // `ai_short_agent_per_region_routing.rs`).
    let region_b = world
        .spawn(Region {
            empire,
            member_systems: vec![home_b],
            capital_system: home_b,
            mid_agent: None,
        })
        .id();
    world
        .entity_mut(home_b)
        .insert(RegionMembership { region: region_b });
    world
        .resource_mut::<RegionRegistry>()
        .by_empire
        .entry(empire)
        .or_default()
        .push(region_b);

    // One MidAgent per Region.
    let mid_a = world
        .spawn(MidAgent {
            region: region_a,
            state: MidTermState::default(),
            auto_managed: true,
        })
        .id();
    let mid_b = world
        .spawn(MidAgent {
            region: region_b,
            state: MidTermState::default(),
            auto_managed: true,
        })
        .id();
    world.get_mut::<Region>(region_a).unwrap().mid_agent = Some(mid_a);
    world.get_mut::<Region>(region_b).unwrap().mid_agent = Some(mid_b);

    // Colonies — distinct stockpiles per system so the two
    // `RegionShortInputs` rows are observably different. Helper
    // attaches `ResourceStockpile` to the StarSystem on first call.
    //
    // Stockpile A: minerals=10_000, energy=5_000
    // Stockpile B: minerals=  500, energy=  250
    //
    // Region A is intentionally much richer than Region B so the
    // overwrite-style bug (= last-write wins on the empire key) would
    // collapse both rows to the same value and the test would catch it.
    let colony_a = spawn_test_colony(
        world,
        home_a,
        Amt::units(10_000),
        Amt::units(5_000),
        vec![None, None, None, None],
    );
    world.entity_mut(colony_a).insert(FactionOwner(empire));
    let colony_b = spawn_test_colony(
        world,
        home_b,
        Amt::units(500),
        Amt::units(250),
        vec![None, None, None, None],
    );
    world.entity_mut(colony_b).insert(FactionOwner(empire));

    TwoRegionLayout {
        empire,
        region_a,
        region_b,
        home_a,
        home_b,
        colony_a,
        colony_b,
    }
}

/// Snapshot the per-region `current_minerals / current_energy` after
/// the Mid pipeline has run. Panics if either region's row is missing
/// — the test exists precisely to ensure both rows are populated.
fn snapshot_per_region(app: &App, region: Entity) -> (Amt, Amt) {
    let inputs = app.world().resource::<ShortAgentTickInputs>();
    let row = inputs
        .per_region
        .get(&region)
        .unwrap_or_else(|| panic!("RegionShortInputs missing for region {:?}", region));
    (row.current_minerals, row.current_energy)
}

/// Finding 1 regression: a pending build order in **region B** must not
/// reduce **region A's** published `current_minerals`. Pre-fold-in, the
/// pending-subtraction walk in `npc_decision_tick` iterated every
/// colony `BuildQueue` owned by the empire regardless of which region
/// the colony's host system belonged to — so a pending order in B
/// silently subtracted from A's headroom.
#[test]
fn colony_pending_outside_region_not_subtracted_from_other_region() {
    let mut app = test_app();
    let layout = build_two_region_layout(&mut app);

    // Prime: Startup tick declares AI schema + populates the first
    // `ShortAgentTickInputs` rows.
    advance_time(&mut app, 1);
    let (m_a_before, e_a_before) = snapshot_per_region(&app, layout.region_a);
    let _ = layout.home_a;
    let _ = layout.home_b;

    // Push a pending ship order onto the colony living in **region B**.
    {
        let mut queue = app
            .world_mut()
            .get_mut::<BuildQueue>(layout.colony_b)
            .expect("colony_b should carry BuildQueue (spawn_test_colony seeds it)");
        queue.queue.push(BuildOrder {
            order_id: 42,
            kind: BuildKind::Ship,
            design_id: "fake_pending_region_b".into(),
            display_name: "Fake Pending (Region B)".into(),
            minerals_cost: Amt::units(80),
            minerals_invested: Amt::ZERO,
            energy_cost: Amt::units(50),
            energy_invested: Amt::ZERO,
            build_time_total: 30,
            build_time_remaining: 30,
        });
    }

    // Re-tick so `npc_decision_tick` recomputes the per-region sums.
    advance_time(&mut app, 1);
    let (m_a_after, e_a_after) = snapshot_per_region(&app, layout.region_a);
    let (m_b_after, e_b_after) = snapshot_per_region(&app, layout.region_b);

    // Region A must be untouched by the pending order in B. We allow
    // a tiny tolerance for production/maintenance drift between the
    // two ticks (the `+ 5` minerals/energy seeded by `spawn_test_colony`
    // production); the pending-order delta (80 / 50) is an order of
    // magnitude larger than typical drift, so the assertion would
    // trivially fail under the pre-fold-in code.
    let region_a_minerals_drift = if m_a_before > m_a_after {
        m_a_before.sub(m_a_after)
    } else {
        m_a_after.sub(m_a_before)
    };
    let region_a_energy_drift = if e_a_before > e_a_after {
        e_a_before.sub(e_a_after)
    } else {
        e_a_after.sub(e_a_before)
    };
    assert!(
        region_a_minerals_drift < Amt::units(50),
        "region A minerals drift = {:?} (before {:?}, after {:?}); a region-B pending order \
         must NOT subtract from region A's per_region.current_minerals",
        region_a_minerals_drift,
        m_a_before,
        m_a_after,
    );
    assert!(
        region_a_energy_drift < Amt::units(30),
        "region A energy drift = {:?} (before {:?}, after {:?}); a region-B pending order \
         must NOT subtract from region A's per_region.current_energy",
        region_a_energy_drift,
        e_a_before,
        e_a_after,
    );

    // Region B must show the subtraction (= invariant in the inverse
    // direction — the pending order belongs to B so B's row should
    // reflect it). This is the same assertion shape as
    // `resource_gate_subtracts_pending_orders_from_stockpile` in
    // `ai_resource_gate_hotfix.rs`, restated against the per-region
    // row.
    assert!(
        m_b_after <= Amt::units(500).sub(Amt::units(80)).add(Amt::units(20)),
        "region B per_region.current_minerals = {:?}; should be <= 500 - 80 + production drift \
         (the pending order's 80 minerals must be subtracted from region B's row)",
        m_b_after,
    );
    assert!(
        e_b_after <= Amt::units(250).sub(Amt::units(50)).add(Amt::units(20)),
        "region B per_region.current_energy = {:?}; should be <= 250 - 50 + production drift",
        e_b_after,
    );
}

/// Finding 2 regression: an empire with **two** MidAgents must produce
/// two **distinct** `RegionShortInputs` rows — one per region, with
/// the region-specific stockpile sum. Pre-fold-in, both MidAgents'
/// inserts collided on the same empire key and the second overwrote
/// the first.
///
/// The setup gives region A 10_000 minerals and region B 500 — the
/// gap is large enough that the legacy empire-keyed map would have
/// returned the same value for both lookups, immediately failing the
/// `!=` assertion. With per-region keying both rows are preserved.
#[test]
fn multi_midagent_inputs_keyed_by_region_not_overwritten() {
    let mut app = test_app();
    let layout = build_two_region_layout(&mut app);

    advance_time(&mut app, 1);

    let inputs = app.world().resource::<ShortAgentTickInputs>();
    assert!(
        inputs.per_region.contains_key(&layout.region_a),
        "ShortAgentTickInputs.per_region must contain region_a after one Mid tick",
    );
    assert!(
        inputs.per_region.contains_key(&layout.region_b),
        "ShortAgentTickInputs.per_region must contain region_b after one Mid tick",
    );
    assert!(
        inputs.per_region.len() >= 2,
        "ShortAgentTickInputs.per_region must hold at least 2 rows for a 2-MidAgent empire; got {}",
        inputs.per_region.len(),
    );

    let (m_a, e_a) = snapshot_per_region(&app, layout.region_a);
    let (m_b, e_b) = snapshot_per_region(&app, layout.region_b);

    // Region A stockpile is order-of-magnitude larger than region B's
    // (10_000 vs 500). With per-empire keying the second MidAgent
    // would overwrite the first (or vice versa) and both lookups
    // would return the same value — fail.
    assert_ne!(
        m_a, m_b,
        "per_region[region_a].current_minerals ({:?}) must differ from \
         per_region[region_b].current_minerals ({:?}); equal values indicate the legacy \
         empire-keyed scratch map (per-MidAgent overwrites)",
        m_a, m_b,
    );
    assert_ne!(
        e_a, e_b,
        "per_region[region_a].current_energy ({:?}) must differ from \
         per_region[region_b].current_energy ({:?})",
        e_a, e_b,
    );

    // Sanity: region A row reflects the richer stockpile.
    assert!(
        m_a > m_b,
        "region A is seeded with 10_000 minerals and region B with 500; per_region[A] ({:?}) \
         should exceed per_region[B] ({:?})",
        m_a,
        m_b,
    );

    // Sanity (`ResourceStockpile` on the StarSystem entity stays the
    // source of truth — the `current_*` fields are derived from it).
    let stockpile_a = app
        .world()
        .get::<ResourceStockpile>(layout.home_a)
        .expect("home_a should carry ResourceStockpile");
    let stockpile_b = app
        .world()
        .get::<ResourceStockpile>(layout.home_b)
        .expect("home_b should carry ResourceStockpile");
    assert!(
        stockpile_a.minerals > stockpile_b.minerals,
        "underlying stockpile A should still exceed stockpile B (regression guard against \
         spawn_test_colony reshuffling resources between systems)",
    );

    // Make sure the empire entity is still distinct from both region
    // entities — defends against accidental aliasing in case Bevy
    // recycles ids in some future refactor (`region_a == empire`
    // would mask the keying-by-region check).
    assert_ne!(layout.empire, layout.region_a);
    assert_ne!(layout.empire, layout.region_b);
}
