# TODO — open issue 棚卸し (2026-04-14 時点、diplomacy v2 spec 確定後)

対象: open issues で `priority:icebox` 以外。本版で diplomacy v2 関連 5 issue (#321〜#325) と ESC umbrella (#326) を追加、#302/#304/#305 を v2 spec に改訂。

## 分類

### 1) 影響範囲が広い (foundation / schema refactor / cross-cutting)

| #        | title                                                    | pri    | ms  | 備考                               |
| -------- | -------------------------------------------------------- | ------ | --- | ---------------------------------- |
| #297     | S-2 FactionOwner 統一付与                                | medium | -   | 全 entity 型に波及                 |
| #301     | S-7 Port migration (SystemBuilding→Core module)          | low    | -   | schema refactor                    |
| #303     | S-10 on_sovereignty_changed Lua hook + cascade           | medium | -   | cascade 挙動広め                   |
| #289     | β Lua View types (SystemView/ColonyView/...)             | medium | -   | modding 基盤、#310 と相乗          |
| **#321** | Diplomacy v2: define_negotiation_item_kind Lua API       | medium | -   | 8 kind 定義、merge/validate/apply  |
| **#322** | Diplomacy v2: Condition atom 拡張 (9 種、Rust hardcoded) | medium | -   | UI walk による未充足理由表示の前提 |
| **#323** | Diplomacy v2: faction_type → instance preset 化 refactor | low    | -   | runtime コードから type 参照排除   |
| **#325** | Diplomacy v2: 既存 DiplomaticAction enum 廃止 migration  | medium | -   | breaking change、#302 と同 PR 推奨 |

### 2) 実装量が多い (epic / 大きめ単発)

| #        | title                                   | pri      | ms    | 備考                                                   |
| -------- | --------------------------------------- | -------- | ----- | ------------------------------------------------------ |
| #292     | Sovereignty Phase 2 epic                | high     | -     | S-1〜S-11 umbrella                                     |
| #211     | 戦闘 epic                               | high     | -     | umbrella                                               |
| #189     | AI epic                                 | high     | 0.3.0 | umbrella                                               |
| #163     | Faction epic (再分類後)                 | low      | 0.3.0 | 残 UI / NPC AI                                         |
| #213     | 静的防御 implementation epic            | medium   | 0.4.0 | 中 epic                                                |
| **#326** | Empire Situation Center (ESC) epic      | medium   | -     | 5 section、egui floating panel、diplomacy 独立         |
| **#332** | gamestate pivot → pure scoped closures  | medium   | -     | #263/#320/#328 obsolete、live read/write + unsafe なし |
| #302     | S-8 DiplomaticOption framework (改訂後) | medium   | -     | Inbox dispatch / Faction.allowed_diplomatic_options    |
| #190     | AI 戦闘投影モデル                       | medium   | 0.3.0 | AI core                                                |
| #280     | Colony Hub Phase 1                      | **high** | 0.3.0 | 機構 + content                                         |
| #268     | Courier opportunistic relay             | medium   | 0.3.0 | command ID dedup 含む                                  |
| #184     | 地上戦闘                                | medium   | 1.0.0 | 新規 system                                            |
| #310     | Lua コンソール                          | low      | -     | 新規 UI + LogBuffer                                    |

### 3) 軽微な修正 (narrow patch)

| #    | title                                      | pri    | ms  | 備考               |
| ---- | ------------------------------------------ | ------ | --- | ------------------ |
| #284 | profiling feature (trace_tracy)            | low    | -   | cargo feature 追加 |
| #290 | building_lost typed EventContext migration | low    | -   | 軽微               |
| #291 | fleet_system_entered/left event            | medium | -   | 新 event 定義      |

### 4) コンテンツ追加 (新 entity / 新 mechanic / UI piece)

| #        | title                                                   | pri    | ms    |
| -------- | ------------------------------------------------------- | ------ | ----- |
| #296     | S-3 Infrastructure Core Deliverable                     | medium | -     |
| #298     | S-4 Conquered state mechanic                            | medium | -     |
| #299     | S-5 Core auto-spawn + settle gate                       | medium | -     |
| #300     | S-6 Defense Fleet 自動組成                              | medium | -     |
| #304     | S-9 Diplomacy UI (改訂後、Inbox 除く)                   | medium | -     |
| #305     | S-11 Casus Belli system (改訂後)                        | medium | -     |
| **#324** | Diplomacy v2: Annihilation handling (Extinct + history) | medium | -     |
| #219     | Port 戦闘参加 (D-1)                                     | medium | 0.3.0 |
| #220     | 防衛プラットフォーム (D-2)                              | medium | 0.4.0 |
| #204     | FleetCombatCapability (初 AI capability)                | medium | 0.3.0 |
| #174     | 外交 UI panel (旧 v1)                                   | low    | 1.0.0 |
| #143     | UI アイコン仮画像                                       | medium | 1.0.0 |
| #139     | 軌道爆撃                                                | low    | 1.0.0 |
| #121     | Interdictor                                             | low    | 1.0.0 |
| #120     | FTL wake detection                                      | low    | 1.0.0 |
| #218     | FTL wake signature                                      | low    | 1.0.0 |

---

## 着手順 (並列 sprint 単位、優先度より並列実行可能性を優先)

### Phase 1 — schema 波及 + modding 基盤
- #297 S-2 FactionOwner 統一付与
- #289 β Lua View types
- 並列: #296 S-3 Infrastructure Core Deliverable (requires #297)

隙間 pickup:
- #284 profiling feature

### Phase 2 — content / mechanic ラッシュ + ESC framework (並列多数)

Sovereignty content (#296 後に全部並列可):
- #298 S-4、#299 S-5、#300 S-6、#301 S-7 (independent)、#303 S-10

Modding follow-ups (#289 後):
- #291 fleet events
- #290 building_lost typed EventContext migration

UX foundation (independent、early 着手推奨):
- **#326 ESC framework + 5 section** ← diplomacy 完全独立、UX 改善幅大 (Phase 3 の Diplomacy UI 前に必須)
- **#310 Lua console** ← independent、ESC と egui floating panel pattern 共有

Combat / infra 並列:
- **#280 Colony Hub Phase 1** ← high、独立に並走
- #219 Port 戦闘参加

### Phase 3 — Diplomacy v2 sprint (foundation → mechanic → UI)

**Phase 3a (foundation 並列、#302/#321/#322/#323 を同時 kick)**:
- #302 DiplomaticOption framework + Inbox dispatch (基盤)
- #321 define_negotiation_item_kind + 8 kind 定義
- #322 Condition atom 拡張 (9 種)
- #323 faction_type → instance preset 化

**Phase 3b (mechanic、3a land 後)**:
- #305 Casus Belli + end_scenarios (requires #302 / #321 / #322)
- #325 DiplomaticAction enum 廃止 migration (#302 と同 PR or 直後 PR)
- #324 Annihilation handling (Extinct component)
- 並列: #163 残

**Phase 3c (UI、3b land 後)**:
- #304 Diplomacy UI 4 panel (Diplomacy / Negotiation modal / End-of-War picker / Casus Belli viewer)
- ESC に Inbox section 追加 (別 sub-issue、#326 land 済前提)

### Phase 4 — AI / combat 深化
- #190 AI 戦闘投影 → #204 FleetCombatCapability (#189 epic 内、順序依存)
- 並列: #220 防衛プラットフォーム、#213 静的防御 epic まとめ、#268 Courier relay

### Phase 5 — 1.0.0 行き (後回し固定)
#184 地上戦、#139 軌道爆撃、#120 / #218 wake 系、#121 Interdictor、#143 UI icons

---

## 繰り上げ判断 (priority:low だが早期並列可)

- **#310 Lua コンソール** — 独立、Phase 1 / 3 でいつでも挿入可、ESC と並走推奨
- **#284 profiling** — trivial、Phase 1 で隙間投入
- **#290 building_lost migration** — #288 直後、軽微
- **#301 S-7 Port migration** — 並列枠があれば Phase 2〜3 いつでも
- **#323 faction_type preset 化** — low だが diplomacy v2 sprint 並走必須

---

## 着手推奨

1. **#309** → 即刻 (数時間)
2. Phase 1 の **#295 / #288 / #247** を 3 worktree agent で同時 kick
3. 隙間に **#239 / #284 / #310** を pickup
4. Phase 2 以降は Phase 1 成果物のマージ状況を見て次 sprint を組む
5. Phase 3 で **#326 ESC framework** を早めに kick (Phase 4 の Diplomacy UI 前に land 必須)
6. Phase 4 は 4a → 4b → 4c の段階。4a は 4 issue 並列 worktree 推奨

---

## 関連 docs
- `docs/diplomacy-design.md` — diplomacy v2 spec (#302/#304/#305/#321/#322/#323/#324/#325 全部の reference)
