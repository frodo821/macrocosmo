# UI DSL Resume Prompt

Macrocosmo の Lua UI DSL 作業を再開する。まず `git status --short` を確認し、既存の未コミット変更をユーザー変更として扱うこと。UI DSL と crate 分割に関係しない変更は戻さない。

## Current State

- 設計メモは `docs/ui_dsl/README.md` と `docs/ui_dsl/noop-fragment-pass.md` にある。
- `docs/ui_dsl/README.md` には V1 contract と implementation risks / limits の追記がある。
- `docs/ui_dsl/noop-fragment-pass.md` には no-op Lua fragment pass の gap / implementation notes がある。
- `macrocosmo/scripts/ui/init.lua` に既存 UI の一部を Lua DSL で表現した no-op fragment がある。現時点では描画には wire しない方針。
- UI DSL は `macrocosmo-ui-dsl` crate に分割済み。
- `macrocosmo-ui-dsl` の direct dependencies は `egui` と `mlua` のみ。
- `macrocosmo-ui-dsl/src/runtime.rs` は Bevy に依存しない純粋な runtime / reconciliation / state update のテスト対象。
- `macrocosmo-ui-dsl/src/lua.rs` は Lua primitive helper と `define_ui_fragment` accumulator の境界。
- `macrocosmo/src/ui/dsl.rs` は `macrocosmo_ui_dsl::*` の compatibility re-export。
- `macrocosmo/src/scripting/ui_dsl_api.rs` は `macrocosmo_ui_dsl::lua::*` の compatibility re-export と既存 Lua accumulator test。

## Key Decisions

- まだ UI への wiring はしない。
- DSL crate は game / Bevy / simulation type に依存させない。host 側で opaque id に変換する。
- runtime 側の entity は `EntityRef = u64` として扱う。
- V1 は host-owned mounted fragment tree を扱い、child-node diff までは持ち込まない。
- state は同一 `instance_id` かつ同一 `fragment_id` のときだけ保持する。
- `fragment_id` が変わる場合は state / descriptor cache を reset する。
- `context_hash` が変わる場合は state は保持し、descriptor cache を dirty にする。
- desired fragment の duplicate `instance_id` は fail closed にする。
- state update batch は invalid update が含まれていたら state を変更しない。
- descriptor cache は実際に state / context が変わったときだけ dirty にする。
- Lua から出る descriptor は data-only table とし、host object / closure / callback を直接保持しない。
- action / capability / context type は host が validate する前提で、Lua DSL 自体は権限境界にしない。

## Important Files

- `Cargo.toml`
- `Cargo.lock`
- `macrocosmo/Cargo.toml`
- `macrocosmo-ui-dsl/Cargo.toml`
- `macrocosmo-ui-dsl/src/lib.rs`
- `macrocosmo-ui-dsl/src/runtime.rs`
- `macrocosmo-ui-dsl/src/lua.rs`
- `macrocosmo/src/ui/dsl.rs`
- `macrocosmo/src/ui/mod.rs`
- `macrocosmo/src/scripting/ui_dsl_api.rs`
- `macrocosmo/src/scripting/globals.rs`
- `macrocosmo/scripts/init.lua`
- `macrocosmo/scripts/ui/init.lua`
- `docs/ui_dsl/README.md`
- `docs/ui_dsl/noop-fragment-pass.md`

## Tests Last Run

After `target` was cleared, these passed:

```sh
cargo fmt --check
cargo test -p macrocosmo-ui-dsl
cargo test -p macrocosmo --lib ui_dsl_api::tests::define_ui_fragment_accumulates_tables
```

Notes:

- `cargo test -p macrocosmo-ui-dsl` had 15 passing tests.
- The targeted `macrocosmo` Lua accumulator test passed.
- Avoid running multiple cargo builds in parallel on a freshly cleared `target`; an earlier parallel run hit an sccache missing-rmeta race.
- A previous full `cargo test -p macrocosmo` failed due to disk exhaustion before `target` was cleared, not due to a known test failure.

## Runtime Test Coverage Already Added

- fragment matching by labels / mode / host capability
- forbidden labels
- registry match ordering by order then id
- labels any / all / forbidden combinations
- building a new mounted fragment tree
- preserving state and descriptor cache on unchanged fragment
- preserving keyed state across reorder
- resetting state on `fragment_id` change
- invalidating descriptor cache on context hash change while preserving state
- rejecting duplicate desired `instance_id`
- marking descriptor dirty only on actual state changes
- rejecting unknown state keys and type-changing updates
- deterministic batched state updates
- atomic invalid batch behavior
- Lua primitive helpers returning descriptor tables

## Useful Next Steps

1. Add source metadata to Lua fragment definitions, such as file/module and registration order.
2. Implement parsing from `define_ui_fragment(...)` accumulator into registry definitions.
3. Add typed context spec and context value type checking.
4. Add declared `needs` and host constraint / capability matching.
5. Add descriptor parser validation, including unknown node kind handling and cycle rejection.
6. Add a read-only ESC host integration later, before enabling UI actions.
7. Keep action dispatch disabled until descriptor validation and host capability checks are explicit.

## Caveats

- `egui` is currently a direct dependency of `macrocosmo-ui-dsl` but not yet used by code. This is intentional for the requested crate boundary.
- The workspace has unrelated dirty files. Do not revert or normalize them while continuing UI DSL work.
- Prefer focused tests while iterating:

```sh
cargo test -p macrocosmo-ui-dsl
cargo test -p macrocosmo --lib ui_dsl_api::tests::define_ui_fragment_accumulates_tables
```

