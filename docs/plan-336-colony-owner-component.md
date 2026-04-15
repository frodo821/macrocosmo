# Plan #336 — Colony.Owner component 導入 (planet→system→Sovereignty chain 間接の解消)

**Status:** Plan-only / pre-implementation draft
**Scope verdict:** **Case A — refactor-only (no new component).** Issue 本文は PR #330 (#297) 以降の現実に追いついていない。必要なのは `ColonyView.owner_empire_id` を既存 `FactionOwner` 直引きに書き換える微小な refactor のみ。
**Estimated size:** 1 commit / +20 / -15 行 (テスト込み 1 commit / +80 / -15 行)

---

## §1. 現状棚卸し

### 1.1 Colony 周辺の Component 一覧

`macrocosmo/src/colony/mod.rs:113-118` (Colony 本体):
```rust
#[derive(Component)]
pub struct Colony {
    pub planet: Entity,
    pub population: f64,
    pub growth_rate: f64,
}
```

Colony が同居する Component 群 (spawn bundle 実測 @ `colony/colonization.rs:99-132`, `setup/mod.rs:362-394`, `ship/settlement.rs:182-215`):

- `Colony` (上記)
- `Production` (`ModifiedValue<Amt>` x 4)
- `BuildQueue` / `BuildingQueue`
- `Buildings` (slots)
- `ProductionFocus`
- `MaintenanceCost`
- `FoodConsumption`
- `ColonyPopulation` / `ColonyJobs` / `ColonyJobRates`
- **`FactionOwner(Entity)`** ← #297 で追加済み。condition 付きで insert されるので Colony ではない bare test entity 以外は常に存在する。

### 1.2 `FactionOwner` が Colony に付与される経路 (#297 PR #330)

`FactionOwner` は4箇所すべての colony spawn path で insert される:

| # | Location | File:Line | Owner source |
|---|----------|-----------|--------------|
| 1 | 首都 colony spawn | `macrocosmo/src/colony/colonization.rs:99-156` (esp. L154) | `PlayerEmpire` query |
| 2 | Colonization queue tick (同星系拡張) | `macrocosmo/src/colony/colonization.rs:240-288` (esp. L245-246, L286-288) | `source_colony` の `FactionOwner` を継承 |
| 3 | Faction on_game_start helper | `macrocosmo/src/setup/mod.rs:349-401` (esp. L395-398) | `apply_game_start_actions` で解決された faction entity |
| 4 | Colony ship settling | `macrocosmo/src/ship/settlement.rs:165-220` (esp. L216-220) | `ship_faction_owner` → `Ship.owner::Empire(e)` fallback |

regression coverage: **`macrocosmo/tests/faction_owner_unification.rs`**
- L107-114 capital spawn
- L205-260 colonization inheritance
- L260-312 settling
- L330-355 neutral ship negative case (FactionOwner が付かない)

### 1.3 現行 `build_colony_view` の owner 解決 (chain lookup)

`macrocosmo/src/scripting/gamestate_scope.rs:635-701`, 特に **L649-661**:

```rust
let mut colony_system: Option<Entity> = None;
if let Some(planet) = world.get::<Planet>(colony.planet) {
    colony_system = Some(planet.system);
    t.set("system_id", planet.system.to_bits())?;
    t.set("planet_name", planet.name.as_str())?;
}
if let Some(sys_entity) = colony_system {
    if let Some(sov) = world.get::<Sovereignty>(sys_entity) {
        if let Some(Owner::Empire(e)) = sov.owner {
            t.set("owner_empire_id", e.to_bits())?;
        }
    }
}
```

つまり **colony → planet → system → Sovereignty.owner** の3段間接。
対照的に `build_ship_view` (L716-724) は `ship.owner` 直読み、`build_system_view` (L602-607) は `system.Sovereignty` 直読み。
**ColonyView だけが間接 path を使っている**。

### 1.4 `system_owner` / `entity_owner` helper (#297 S-2)

`macrocosmo/src/faction/mod.rs:954-992`:

```rust
pub fn system_owner(
    system: Entity,
    at_system: &Query<(&AtSystem, &FactionOwner), With<CoreShip>>,
) -> Option<Entity>                  // Core ship 在星で決まる (Sovereignty 根拠)

pub fn entity_owner(world: &World, entity: Entity) -> Option<Entity> {
    // 1. FactionOwner 直読み (colony/ship/system/structure 全部 OK)
    // 2. Ship.owner = Owner::Empire fallback
    // 3. それ以外は None
}
```

`entity_owner(world, colony_entity)` は **FactionOwner が付与された Colony にはそのまま正しい Empire entity を返す**。現行 ColonyView はこの helper を使っていない。

### 1.5 Savebag / SAVE_VERSION 現状

- `SAVE_VERSION = 2` (`macrocosmo/src/persistence/save.rs:78`)
- `SavedComponentBag` に `pub faction_owner: Option<SavedFactionOwner>` が **すでに存在** (`macrocosmo/src/persistence/savebag.rs:4318`)
- load 側 (`macrocosmo/src/persistence/load.rs:257-259`) は Colony の `FactionOwner` を round-trip する
- save 側 (`macrocosmo/src/persistence/save.rs:362-363`) は Colony を含むすべての entity から `FactionOwner` を回収する

**→ fixture (`minimal_game.bin`, 732 B) の再生成は不要。** 本 issue の refactor は wire format を変えない。

---

## §2. 本 Issue の真の Scope — 判断結果

### 2.1 Issue 文言と現実の差分

Issue は "Colony 自体に Empire ownership を持たないため planet → system → Sovereignty の chain lookup" と主張しているが、この前提は **#297 (PR #330) マージ後に古くなっている**。

- ECS 上 Colony → Empire の直接 link は **FactionOwner という形で既に存在** (§1.2)
- chain lookup が残っているのは `ColonyView.owner_empire_id` ただ1箇所 (§1.3)
- 実コード側 (authority.rs / outline UI / savebag / tests) はすべて `FactionOwner` 直読みで動いている

### 2.2 3 パターン分岐

| パターン | 判定 | 根拠 |
|---|---|---|
| **A. 新 component 必要** | ✗ 不要 | `FactionOwner` の semantic (= 行政上の所有者 = 管理 empire) は issue が求める "Colony.Owner: Entity" と完全に一致。plan-297 §2C/§2D 参照。別 component を増やすと 2 source of truth になり #297 の成果を台無しにする。 |
| **B. refactor-only** | ✓ **採用** | `ColonyView.owner_empire_id` を `FactionOwner` 直引きに書き換えるだけで issue の完了条件 (chain lookup 解消, lookup cost 削減) を満たす。`_default_` Sovereignty edge case も自動解消 (FactionOwner は Sovereignty.owner とは独立)。 |
| **C. redundant / close** | △ 近いが不十分 | chain lookup 自体はまだ残存しているので "何もしなくて良い" とは言えない。refactor commit 1 本で閉じるのが正しい。 |

### 2.3 Issue の "完了条件" マッピング

Issue 完了条件 → refactor-only での対応:

- [x] `Colony.Owner` component 追加 → **FactionOwner で代替済み** (#297)
- [x] colonization flow で必ず設定 → **済み** (§1.2 の 4 経路)
- [ ] `ColonyView.owner_empire_id` が Colony.Owner 直引きに → **本 PR で実施**
- [ ] chain fallback の regression test → **本 PR で追加**
- [x] save/load 互換 → **savebag は既に FactionOwner を round-trip**

**5 完了条件のうち 3 は既に満たされており、残り 2 (View 書き換え + test) が本 PR の scope。**

---

## §3. Case A (refactor-only) 実装計画 — **採用案**

### 3.1 変更点

**File 1: `macrocosmo/src/scripting/gamestate_scope.rs`**

L649-661 (chain lookup) を `FactionOwner` 直引きに書き換える:

```rust
// before:
let mut colony_system: Option<Entity> = None;
if let Some(planet) = world.get::<Planet>(colony.planet) {
    colony_system = Some(planet.system);
    t.set("system_id", planet.system.to_bits())?;
    t.set("planet_name", planet.name.as_str())?;
}
if let Some(sys_entity) = colony_system {
    if let Some(sov) = world.get::<Sovereignty>(sys_entity) {
        if let Some(Owner::Empire(e)) = sov.owner {
            t.set("owner_empire_id", e.to_bits())?;
        }
    }
}

// after:
// planet → system の resolution は building view 用の system_id 公開に
// 必要なので残す。owner 解決だけを FactionOwner 直引きに差し替える
// (#336 / #297 S-2: Colony は FactionOwner を直接持つ).
if let Some(planet) = world.get::<Planet>(colony.planet) {
    t.set("system_id", planet.system.to_bits())?;
    t.set("planet_name", planet.name.as_str())?;
}
// Primary: FactionOwner component (colonization / settling / capital /
// faction on_game_start すべての spawn 経路で必ず付与される — #297).
if let Some(fo) = eref.get::<crate::faction::FactionOwner>() {
    t.set("owner_empire_id", fo.0.to_bits())?;
}
```

**備考:**

1. Sovereignty fallback は **意図的に外す**。issue 原文は "chain lookup を fallback として残す" と書くが、#297 以降 Colony には常に FactionOwner が付く (4 spawn 経路すべて) ため実質 dead code になる。旧 save (SAVE_VERSION=1) の migration path で colony に FactionOwner が無いケースのみ fallback が意味を持つが、現行 `SAVE_VERSION = 2` のみ受け入れ (`load.rs:499-503` で不一致は hard error) なので fallback 分岐は実行されない。
2. 本当に fallback を残したい場合は §4.4 参照 (defensive option)。
3. `use` 追加は不要 (`crate::faction::FactionOwner` は fully-qualified で 1 箇所使うだけ)。

**File 2: `macrocosmo/tests/faction_owner_unification.rs` or 新規 `tests/colony_view_owner.rs`**

regression test 追加:

```rust
#[test]
fn colony_view_owner_empire_id_reads_faction_owner_directly() {
    // Given: colony with FactionOwner but parent system has NO Sovereignty
    //        (the edge case where the old chain lookup would return nil)
    // When:  build_colony_view resolves owner_empire_id
    // Then:  returns the FactionOwner empire (NOT nil)
}

#[test]
fn colony_view_owner_nil_without_faction_owner() {
    // Given: colony WITHOUT FactionOwner (neutral / test spawn)
    // Then:  owner_empire_id is not set (Lua-side: nil)
}

#[test]
fn colony_view_owner_independent_of_sovereignty() {
    // Given: colony owned by empire A, system Sovereignty.owner = empire B
    //        (occurs transiently when empire B's Core ship enters A's system)
    // Then:  ColonyView.owner_empire_id == A  (FactionOwner wins over chain)
}
```

このうち 3 番目は **#292 Sovereignty Phase 2 との関係を test で pin する**。現行 chain lookup だと administrative owner が Core-ship presence で flip してしまっていた潜在バグ。

### 3.2 Lua API 影響

- `ColonyView.owner_empire_id` field **shape/name は不変**。Lua スクリプト側の既存 access (event callback etc.) に影響なし。
- Lua scripts 側で `owner_empire_id` を読んでいる箇所は **0 件** (grep `/macrocosmo/scripts` 実測結果: "No matches found")。既存 Lua consumer はそもそも居ない。
- `gamestate_scope.rs` の 1 テスト以外に rust-side consumer も無し (grep 実測: 参照 5 ファイル = gamestate_scope.rs + faction/mod.rs + plan-332 + plan-289 + CLAUDE.md)。

### 3.3 想定差分規模

- Code: `gamestate_scope.rs` -13 / +8 行
- Tests: +60-80 行 (3 test × ~25 行)
- Docs: CLAUDE.md §ColonyView の 1 行更新 (optional)
- Commit: 1 本 (`#336: Resolve ColonyView.owner_empire_id via FactionOwner directly`)

---

## §4. Case B (仮に新 component を導入する場合) — **採用しない**

この section は future reviewer が "本当に新 component いらないのか" を確認できるように残す。

### 4.1 導入する理由 (仮説)

`Colony.Owner` を `FactionOwner` と別 component にする唯一の動機は、
**行政上の所有者と軍事主権が意味的に分離する瞬間を型で表現したい場合**。
例: empire B が A の colony を Core ship 在星で軍事占領している時、
`Sovereignty.owner = B` / `FactionOwner = A` / `Colony.Owner = A` (占領下でも行政は継続) という3軸を型安全に分離する。

### 4.2 実装コスト (参考値)

- `Colony::Owner(Entity)` 新 component 定義: +10 行
- 4 spawn path migration (`spawn_capital_colony`, `tick_colonization_queue`, `spawn_colony_on_planet`, `process_settling`): +40 / -0 行
- Savebag field 追加 (`Option<SavedColonyOwner>`): +30 行、**SAVE_VERSION bump 必要**
- Fixture 再生成: `cargo test -p macrocosmo --test fixtures_smoke regenerate_minimal_game_fixture -- --ignored` + `minimal_game.bin` 差し替え
- 旧 save (version=2) 受け入れの backwards-compat: load_save に branch 追加。現状 `load.rs:499` は exact match なので migration logic が必要。
- 既存 `FactionOwner` との使い分けドキュメント化

### 4.3 Postcard 制約

`memory/project_save_format_postcard.md` より: postcard は sequential decode なので、`SavedComponentBag` に新 `Option<T>` field を追加する場合 **SAVE_VERSION を bump**して旧 blob を reject する必要あり。本 plan の採用案 (Case A) では bag 変更なし = bump 不要。

### 4.4 Defensive fallback option (refactor-only に Sovereignty fallback を残す亜種)

issue 文言 "migration: 既存 chain lookup は backwards-compat として残す" に字義通り応えたい場合:

```rust
if let Some(fo) = eref.get::<crate::faction::FactionOwner>() {
    t.set("owner_empire_id", fo.0.to_bits())?;
} else if let Some(planet) = world.get::<Planet>(colony.planet) {
    // Legacy path: pre-#297 saves where Colony lacks FactionOwner.
    // Currently unreachable under SAVE_VERSION == 2, but kept as a
    // safety net. Remove when confident no such entities can exist.
    if let Some(sov) = world.get::<Sovereignty>(planet.system) {
        if let Some(Owner::Empire(e)) = sov.owner {
            t.set("owner_empire_id", e.to_bits())?;
        }
    }
}
```

コスト: +8 行。merit: issue 文面への字義順守。demerit: dead code が増え、#292 Phase 2 で Sovereignty ≠ administrative が意図的に分かれた時に間違った fallback を返す。**推奨: 入れない**(採用案)。レビューで強く求められた場合のみ入れる。

---

## §5. ColonyView の最終 API

変更前後で `ColonyView` の Lua surface は **完全同一**:

```
ColonyView {
    id: u64,
    entity: u64,
    population: f64,
    growth_rate: f64,
    planet_id: u64,
    system_id?: u64,          -- planet lookup で設定
    planet_name?: string,     -- 同上
    owner_empire_id?: u64,    -- 実装が chain → FactionOwner に変わるが shape 同じ
    building_slots?: table,
    building_ids?: table,
    production?: { minerals_per_hexadies, energy_per_hexadies, research_per_hexadies, food_per_hexadies },
}
```

互換性: Lua scripts / tests で `owner_empire_id` を読む実装は現状 0 件 (`grep owner_empire_id scripts/` → no match)。破壊的変更なし。

---

## §6. Test 計画

### 6.1 新規 (本 PR で追加)

| Test | File | 目的 |
|---|---|---|
| `colony_view_owner_reads_faction_owner` | `tests/colony_view_owner.rs` (新規) or `faction_owner_unification.rs` 追補 | refactor 後の happy path — FactionOwner 付き Colony が Lua 側で正しい owner を返す |
| `colony_view_owner_missing_without_faction_owner` | 同上 | neutral / bare colony で `owner_empire_id` が unset であることを確認 |
| `colony_view_owner_ignores_sovereignty` | 同上 | **regression pin**: system Sovereignty が他 empire に flip しても ColonyView owner は FactionOwner を返す (= chain lookup 時代の latent bug の修正)|

### 6.2 既存で守るべきテスト

- `faction_owner_unification.rs::colonization_queue_inherits_faction_owner_from_source` (L205-260) — 継承 chain
- `faction_owner_unification.rs::settling_ship_produces_colony_with_faction_owner` (L260-312)
- `fixtures_smoke.rs::load_minimal_game_fixture_smoke` — wire format 不変の証明

### 6.3 Fixture 再生成の要否

**不要**。本 PR は `SavedComponentBag` を変更しない & `SAVE_VERSION` 据え置き。`tests/fixtures/minimal_game.bin` (732 B) はそのまま pass する。

### 6.4 test app helper

- `test_app()` で Colony + FactionOwner + Sovereignty を組み合わせた world を手組みで建てる必要あり。`faction_owner_unification.rs::spawn_helpers` (L150 付近) を参考に ad-hoc fixture を L-level で作る。

---

## §7. #292 Sovereignty Phase 2 との関係

#292 は "colony 単位主権を system 主権から分離する" epic。本 PR は:

- **#292 の scope 外**。colony の administrative owner (FactionOwner) と system の軍事主権 (Sovereignty) を **混同していた chain lookup を解体する** という意味で #292 の下準備になる
- 本 PR 後、`FactionOwner` (行政) と `Sovereignty` (軍事) は ColonyView 上で型として分離する (`owner_empire_id` は FactionOwner 由来、`Sovereignty.owner` は future で別 field に昇格可)
- **§6.1 test #3 (colony_view_owner_ignores_sovereignty)** が #292 Phase 2 の意味的前提を pin してくれる

将来 #292 で "colony-level sovereignty" が導入されるなら、それは `ColonySovereignty(Entity)` という別 component として追加 (FactionOwner と並走) し、`ColonyView.sovereign_empire_id` のような別 field で公開する。本 PR の FactionOwner 直引きはその時もそのまま有効。

---

## §8. Commit 分割案 + LoC 推定

単一 commit で十分:

| # | Title | Files | LoC |
|---|---|---|---|
| 1 | `#336: Resolve ColonyView.owner_empire_id via FactionOwner directly` | `scripting/gamestate_scope.rs` (-13 / +8), `tests/colony_view_owner.rs` (+80) | ~ +88 / -13 |

敢えて分けるなら:

- Commit 1: `gamestate_scope` 書き換え + 最小 test (1 test)
- Commit 2: edge-case test 2 本追加

程度だが、並行 PR (#334 / #335) との merge 衝突面積を減らす意味では **1 commit のほうが良い** (gamestate_scope.rs は #332/#334 系と conflict zone)。

---

## §9. リスク

### 9.1 Fixture 再生成

**Low.** 採用案 (Case A) では不要。Case B を選んだ場合のみ必要。

### 9.2 並行 PR との semantic conflict

**Medium.** `scripting/gamestate_scope.rs` は以下の active PR 予定 area と重なる:

- #332 (gamestate scoped closures, 着手中) — L151 周辺 `build_colony_view` 呼び出し元は触るが、`build_colony_view` の実装本体 (L635-701) は変更しない見込み
- #334 (予定) / #335 (予定) — 未定だが view 系は集中砲火になりやすい

mitigation:
- L649-661 のみピンポイント変更、ヘッダ行は触らない
- PR 着手前に `git log --oneline -- macrocosmo/src/scripting/gamestate_scope.rs` で最新状況を再確認
- 必要なら #332 merge 後に rebase、**merge 後必ず `cargo test`** (`memory/feedback_semantic_merge_conflict.md`)

### 9.3 Sovereignty fallback を外したことでの regression

**Low.** 4 spawn path すべてが FactionOwner を付与する (§1.2 + tests @ `faction_owner_unification.rs`)。bare test spawn (Colony 単独 `world.spawn(Colony { .. })`) のみ影響するが、その場合は旧実装でも Sovereignty が無く nil を返していたので semantic 不変。不安なら §4.4 の defensive fallback を採用。

### 9.4 Lua 側 breaking

**None.** `owner_empire_id` field は shape 不変、Lua 側 consumer は現状 0 件 (§3.2)。

### 9.5 #292 Phase 2 での reopen

**Low-Medium.** #292 で "colony 単位主権" が入る時、FactionOwner ≠ Sovereignty を明示的に分離する拡張が必要になるが、その時も `owner_empire_id` = FactionOwner という本 PR の決定はそのまま維持可。別 field (`sovereign_empire_id`) を足すだけ。

---

## §10. Critical Files for Implementation

Must-read / Must-edit:

| Role | File | Key lines |
|---|---|---|
| **EDIT** (primary refactor) | `macrocosmo/src/scripting/gamestate_scope.rs` | 635-701 (build_colony_view), esp. 649-661 |
| **EDIT** (tests) | `macrocosmo/tests/colony_view_owner.rs` (new) or `macrocosmo/tests/faction_owner_unification.rs` | append |
| **READ** (reference) | `macrocosmo/src/faction/mod.rs` | 954-1015 (`system_owner` / `entity_owner`) |
| **READ** (spawn paths — unchanged) | `macrocosmo/src/colony/colonization.rs` | 99-156, 240-288 |
| **READ** (spawn paths — unchanged) | `macrocosmo/src/setup/mod.rs` | 349-401, 540-596 |
| **READ** (spawn paths — unchanged) | `macrocosmo/src/ship/settlement.rs` | 165-253 |
| **READ** (savebag — unchanged) | `macrocosmo/src/persistence/savebag.rs` | 856-868 (`SavedFactionOwner`), 4318 (bag field) |
| **READ** (load — unchanged) | `macrocosmo/src/persistence/load.rs` | 257-259 |
| **READ** (context) | `docs/plan-297-faction-owner-unification.md` | §2C, §2D |
| **READ** (context) | `docs/plan-289-lua-view-types.md` | §2.4 (original compromise note), §11 |
| **FIXTURE** (no change) | `macrocosmo/tests/fixtures/minimal_game.bin` | — |

Related (only if Case B becomes necessary):

- `macrocosmo/src/persistence/save.rs` L362-363 (savebag bag writer) — adds colony owner field
- `macrocosmo/tests/fixtures_smoke.rs` L167 (`regenerate_minimal_game_fixture`) — fixture rebuild

---

## Appendix A. Open Questions for Review

1. **Sovereignty fallback を残すか外すか** (§3.1 vs §4.4)。採用案は外す。issue 文面尊重派なら残す。
2. **Test file を新規作成か既存追補か**。`faction_owner_unification.rs` は既に colony + FactionOwner の設定 fixture を持つので追補が軽い。view layer の test は別ファイルの方が scope 明確 — 推奨: 新規 `tests/colony_view_owner.rs`。
3. **Issue の close message** で "completed as refactor via FactionOwner, no new component introduced — see plan-336" と明記するか。merit: 未来の自分が issue タイトルに惑わされない。
4. **#292 Phase 2 で colony 単位主権を入れる時** の field 名は `sovereign_empire_id` で確定して良いか (本 PR の時点で単に Unknown のままにするのが safe)。

---

## Appendix B. 判断のエビデンス要約

| 主張 | エビデンス |
|---|---|
| Colony に FactionOwner が既に付く | `colonization.rs:154`, `colonization.rs:286-288`, `setup/mod.rs:395-398`, `settlement.rs:216-220` の 4 insert |
| chain lookup は gamestate_scope のみに残る | `gamestate_scope.rs:649-661`。他は `entity_owner` helper 経由 |
| Savebag 互換 | `savebag.rs:4318` に `faction_owner: Option<SavedFactionOwner>` 既存, `SAVE_VERSION = 2` |
| Lua 側 consumer なし | `grep owner_empire_id macrocosmo/scripts/` → no matches |
| Rust 側 consumer 極小 | grep で gamestate_scope.rs + faction/mod.rs + 3 doc のみ |
