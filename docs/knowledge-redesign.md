# Knowledge Redesign

Status: draft design.

Implementation plan: `docs/knowledge-redesign-implementation-plan.md`.

## Problem

The current `KnowledgeStore` is doing too many jobs:

- It is the light-delayed read model for an empire.
- It stores own-ship projections and intended outcomes.
- AI uses separate `PendingAssignment` markers for commitments that overlap with projections.
- Some UI and visualization paths fall back to realtime ECS state when no `KnowledgeStore` entry exists.
- The component currently lives on `Empire`, even though ships, fleets, regions, and colonies can also become knowledge holders.

This creates two classes of bugs:

- **semantic conflicts**: commitment, observation, and belief are represented by different systems that can disagree.
- **ground-truth leaks**: consumers bypass knowledge and read authoritative ECS state outside explicit dev/omniscient modes.

The redesign treats knowledge as subject-local epistemic state, not as a single global store.

## Core Rule

Authoritative ECS state is the simulation truth.

`KnowledgeState` is what one subject currently knows, believes, and has committed to, based on observations delivered to that subject and commands issued by that subject.

Normal UI, AI, and empire-view visualization must read through a subject's `KnowledgeState` or a facade derived from it. They must not read realtime ECS state except through an explicit `Omniscient` or simulation-producer path.

## Subjects

Knowledge is owned by a subject, not necessarily an empire.

```rust
pub enum KnowledgeSubject {
    Empire(Entity),
    Region(Entity),
    Fleet(Entity),
    Ship(Entity),
    Colony(Entity),
}
```

Implementation should avoid hard-coding `Empire` into APIs. A Bevy component can attach knowledge to any entity:

```rust
#[derive(Component)]
pub struct KnowledgeNode {
    pub subject: KnowledgeSubject,
    pub parent: Option<Entity>,
    pub scope: KnowledgeScope,
}

#[derive(Component)]
pub struct KnowledgeState {
    pub subject: KnowledgeSubject,
    pub observations: ObservationSet,
    pub commitments: CommitmentLedger,
    pub beliefs: BeliefCache,
    pub inbox: KnowledgeInbox,
}
```

Initial implementation can attach `KnowledgeState` only to empires, but all query/helper APIs should be written in terms of `KnowledgeNode` / subject identity so ship/fleet/region stores do not require another redesign.

## Scope

Scope describes what a knowledge subject is responsible for reasoning about.

```rust
pub enum KnowledgeScope {
    GlobalEmpire,
    Region(Entity),
    Fleet(Entity),
    LocalSystem(Entity),
    ShipSelf(Entity),
}
```

Examples:

- Empire: strategic knowledge and high-level commitments.
- Region: sector governor knowledge and local build/economic commitments.
- Fleet: operational knowledge for orders and nearby threats.
- Ship: local observations, current mission, self-state.
- Colony: local economy and construction state.

Scope is not a visibility filter by itself. It is the responsibility boundary for belief materialization and decision making.

## Observation, Commitment, Belief

The state has three semantic layers.

### Observations

Observations are delivered facts about the world.

```rust
pub struct ObservationStamp {
    pub observed_at: Tick,
    pub received_at: Tick,
    pub source: KnowledgeSource,
}

pub struct Observation<T> {
    pub id: ObservationId,
    pub subject: KnowledgeSubject,
    pub value: T,
    pub stamp: ObservationStamp,
    pub confidence: Confidence,
}
```

`observed_at` is when the target was actually in that state. `received_at` is when this subject learned it. These must not be collapsed.

Existing `SystemSnapshot` and `ShipSnapshot` belong in `ObservationSet`.

### Commitments

Commitments are decisions already issued by the subject.

```rust
pub struct Commitment {
    pub id: CommitmentId,
    pub subject: KnowledgeSubject,
    pub actor: Option<Entity>,
    pub kind: CommitmentKind,
    pub target: CommitmentTarget,
    pub issued_at: Tick,
    pub expected_effect_at: Option<Tick>,
    pub expected_resolution_at: Option<Tick>,
    pub status: CommitmentStatus,
    pub based_on: BeliefStamp,
}
```

This is the unified home for concepts currently split between `PendingAssignment`, `AiCommandOutbox`, `PendingAiShipCommand`, and `ShipProjection.intended_*`.

Handler-side marker components can remain as plumbing, but AI decision memory should live in the `CommitmentLedger`.

### Beliefs

Beliefs are derived views from observations plus commitments.

```rust
pub struct BeliefStamp {
    pub based_on_observed_at: Tick,
    pub inferred_at: Tick,
    pub valid_from: Tick,
    pub valid_until: Option<Tick>,
}

pub struct Belief<T> {
    pub value: T,
    pub stamp: BeliefStamp,
    pub confidence: Confidence,
    pub provenance: Vec<ObservationId>,
    pub commitments: Vec<CommitmentId>,
}
```

Beliefs are cacheable, but observations and commitments are the primary data. If a belief has no `BeliefStamp`, it is suitable only for presentation and must not drive AI or command validation.

Existing `ShipProjection` should move toward a materialized ship belief derived from:

- last observed ship state,
- active ship-related commitments,
- route/physics projection rules.

## Merge

Two knowledge states can merge when subjects communicate, dock, relay, report, or are administratively unified.

Merge must not be "newer `observed_at` wins" globally. It is a provenance-preserving reconcile:

```rust
pub struct MergeContext {
    pub at: Tick,
    pub from: KnowledgeSubject,
    pub to: KnowledgeSubject,
    pub channel: KnowledgeChannel,
}

pub struct MergeReport {
    pub inserted_observations: Vec<ObservationId>,
    pub updated_commitments: Vec<CommitmentId>,
    pub invalidated_beliefs: Vec<BeliefKey>,
}

pub fn merge_from(
    to: &mut KnowledgeState,
    from: &KnowledgeState,
    context: MergeContext,
) -> MergeReport;
```

Merge flow:

1. Import observations with provenance and receive time.
2. Import or reconcile commitments only when the receiving subject is allowed to inherit them.
3. Invalidate affected belief cache entries.
4. Re-materialize beliefs lazily or in a scheduled system.

Commitments are not blindly copied. A ship reporting to an empire can report "I am committed to survey X"; that does not necessarily mean the empire has issued a new empire-level commitment. Inheritance rules are domain-specific.

## Read Facade

Consumers should not receive `Option<&KnowledgeStore>` and decide whether to fall back to realtime state.

Use a facade that encodes the mode:

```rust
pub enum PerceptionMode {
    Subject(KnowledgeSubject),
    Omniscient,
}

pub struct Perception<'a> {
    pub mode: PerceptionMode,
    pub knowledge: Option<&'a KnowledgeState>,
}
```

Rules:

- `PerceptionMode::Subject` with missing knowledge returns `Unknown` / `None`.
- `PerceptionMode::Omniscient` may read realtime ECS state.
- AI never receives `Omniscient`.
- UI normal play and EmpireView use `Subject`.
- Dev/debug views must be explicit when using `Omniscient`.

This removes accidental ground-truth leaks from fallback behavior.

## Write Flow

Simulation systems remain authoritative and may read/write realtime ECS state.

Knowledge producers are the only systems that translate simulation truth into observations:

```text
simulation event / snapshot
  -> observation produced at origin subject or origin location
  -> delivery channel computes arrival
  -> target KnowledgeInbox
  -> ObservationSet
  -> BeliefCache invalidation
```

Commands produce commitments before or alongside delayed command dispatch:

```text
AI/UI decides command
  -> CommitmentLedger records issued commitment
  -> delayed command pipeline sends command to actor/target
  -> simulation applies command on arrival
  -> resulting observations eventually resolve/update commitment
```

This makes "I ordered ship A to survey X" locally known immediately without pretending that "ship A is actually surveying X" is already observed.

## Initial Migration

The first implementation should be small and behavior-preserving where possible, but it should introduce the new semantics.

1. Add `KnowledgeSubject`, `KnowledgeNode`, `KnowledgeState` as wrappers around the existing empire `KnowledgeStore` data.
2. Add `CommitmentLedger` and mirror existing `PendingAssignment` creation into it.
3. Add query helpers:
   - `commitments.has_active(subject, kind, target)`
   - `knowledge.subject_view(subject)`
   - `perception.ship_view(...)`
4. Move AI dedup reads from the manual union of `PendingAssignment` / outbox / pending ship command toward `CommitmentLedger`.
5. Change subject-mode ship views so missing knowledge means `Unknown`, not realtime fallback.
6. Keep Omniscient realtime paths explicit and named.
7. After semantics are stable, split modules by domain:
   - `subject`
   - `observation`
   - `commitment`
   - `belief`
   - `merge`
   - `perception`
   - `delivery`

## Non-Goals

- Do not extract a new crate yet.
- Do not add separate `FleetKnowledgeStore` / `RegionKnowledgeStore` types.
- Do not make `Empire` the permanent owner of all knowledge.
- Do not make `BeliefCache` the source of truth.

## Design Invariants

- Every normal consumer reads through a subject-scoped perception facade.
- Every observation has `observed_at` and `received_at`.
- Every AI-usable belief has a `BeliefStamp`.
- Every commitment records the belief it was based on.
- Merging knowledge preserves provenance and invalidates beliefs.
- Ground truth is only for simulation, knowledge production, and explicit omniscient/debug views.
