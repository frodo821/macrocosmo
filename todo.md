# TODO — open issue 棚卸し (2026-04-17 時点)

対象: open issues で `priority:icebox` 以外。
前回 (2026-04-14) から大量 close: ESC epic (#326) 全完了、ScriptableKnowledge (#349) 全完了、gamestate pivot (#332) 完了、Faction epic (#163) close、Fleet epic (#286) close、Phase 1 基盤 (#296/#297/#289) 全完了、bug 2 件 (#364/#365) 修正済。

## マイルストーン概況

| ms | open | closed | 備考 |
|---|---|---|---|
| 0.3.0 | 6 | 40 | Colony Hub + AI + 静的防御 Port + Courier |
| 0.4.0 | 2 | 0 | 静的防御 epic (#213/#220) |
| 1.0.0 | 11 | 0 | 後回し固定 (visuals / endgame) |
| no-ms | 18 | — | Sovereignty Phase 2 + Diplomacy v2 + misc |

## 分類

### 1) Epics (umbrella)

| # | title | pri | ms | 備考 |
|---|---|---|---|---|
| #292 | Sovereignty Phase 2 | high | - | S-1〜S-3 完了済、S-4〜S-11 残 |
| #211 | 戦闘 epic | high | - | 静的防御 (#213/#219/#220) + 地上戦 (#184) |
| #189 | AI epic | high | 0.3.0 | NpcPolicy hook 設置済、実 AI 未着手 |

### 2) Sovereignty Phase 2 sub-issues (S-4〜S-11)

S-1 (#295) / S-2 (#297) / S-3 (#296) は完了済。残 8 件:

| # | title | pri | deps (open) | 備考 |
|---|---|---|---|---|
| #298 | S-4 Conquered state mechanic | medium | - | S-3 done → unblocked |
| #299 | S-5 Core auto-spawn + settle gate | medium | - | S-3 done → unblocked |
| #300 | S-6 Defense Fleet 自動組成 | medium | - | independent |
| #301 | S-7 Port migration | low | - | schema refactor |
| #303 | S-10 on_sovereignty_changed hook | medium | - | cascade 挙動広め |
| #302 | S-8 DiplomaticOption framework | medium | - | Diplomacy v2 基盤 |
| #304 | S-9 Diplomacy UI | medium | #302 | 4 panel |
| #305 | S-11 Casus Belli system | medium | #298 | end_scenarios + auto_war |

### 3) Diplomacy v2 (foundation → mechanic → UI)

全件 no-ms。#302 (S-8) と scope overlap あり。

| # | title | pri | deps | 備考 |
|---|---|---|---|---|
| #321 | define_negotiation_item_kind Lua API | medium | - | 8 kind 定義 |
| #322 | Condition atom 拡張 (9 種) | medium | - | UI walk 前提 |
| #323 | faction_type → instance preset 化 | low | - | runtime type 排除 |
| #325 | DiplomaticAction enum 廃止 | medium | #302 | breaking change |
| #324 | Annihilation handling | medium | - | Extinct + history |

### 4) 0.3.0 milestone 残

| # | title | pri | 備考 |
|---|---|---|---|
| **#280** | Colony Hub Phase 1 | **high** | 機構 + content |
| #268 | Courier opportunistic relay | medium | command ID dedup |
| #219 | Port 戦闘参加 (D-1) | medium | 静的防御 |
| #190 | AI 戦闘投影モデル | medium | Nash feasibility core |
| #204 | FleetCombatCapability | medium | AI capability 実証 |

### 5) misc (no-ms)

| # | title | pri | 備考 |
|---|---|---|---|
| #347 | In-game keybinding manager | medium | ESC/Lua console 向け |
| #291 | fleet_system_entered/left event | medium | 新 event 定義 |
| #290 | building_lost typed EventContext | low | 軽微 migration |
| #310 | Lua コンソール | low | egui floating panel |

### 6) 1.0.0 行き (後回し固定)

#184 地上戦、#174 外交 UI (旧 v1)、#143 UI icons、#139 軌道爆撃、
#120/#218 wake 系、#121 Interdictor、#135 テクスチャ、#157 Lua UI パネル、
#140 惑星破壊、#61 バランス調整

### 7) 0.4.0

| # | title | pri | 備考 |
|---|---|---|---|
| #213 | 静的防御 implementation epic | medium | port 武装 + 防衛施設 |
| #220 | 防衛プラットフォーム (D-2) | medium | 新規 structure |

---

## 着手順 (並列 sprint 単位)

### Phase 2a — Sovereignty content 並列 + Colony Hub (全 unblocked)

Sovereignty (4 並列):
- #298 S-4 Conquered state mechanic
- #299 S-5 Core auto-spawn + settle gate
- #300 S-6 Defense Fleet 自動組成
- #303 S-10 on_sovereignty_changed hook

独立:
- **#280 Colony Hub Phase 1** (high、0.3.0)

隙間 pickup:
- #291 fleet events
- #290 building_lost migration

### Phase 2b — Sovereignty mechanic + port (Phase 2a land 後)

- #301 S-7 Port migration (independent)
- #305 S-11 Casus Belli (requires #298 S-4)

### Phase 3 — Diplomacy v2 sprint (foundation → mechanic → UI)

**Phase 3a (foundation、並列 kick)**:
- #302 S-8 DiplomaticOption framework + Inbox dispatch
- #321 define_negotiation_item_kind + 8 kind 定義
- #322 Condition atom 拡張 (9 種)
- #323 faction_type → instance preset 化

**Phase 3b (mechanic、3a land 後)**:
- #305 S-11 Casus Belli (if not done in 2b)
- #325 DiplomaticAction enum 廃止 (#302 後)
- #324 Annihilation handling

**Phase 3c (UI、3b land 後)**:
- #304 S-9 Diplomacy UI 4 panel

### Phase 4 — AI / combat 深化

- #190 AI 戦闘投影 → #204 FleetCombatCapability (#189 epic 内、順序依存)
- 並列: #219 Port 戦闘参加、#268 Courier relay

### Phase 5 — misc + 0.4.0

- #347 keybinding manager (独立)
- #310 Lua console (独立)
- 0.4.0: #213 / #220 (静的防御)

### Phase 6 — 1.0.0 行き

全部後回し。

---

## 完了済 epic / 大物 (2026-04-15〜04-17)

- ✅ **#326 ESC epic** — #344 framework + #345 Notifications + #346 ongoing tabs 全 close
- ✅ **#349 ScriptableKnowledge epic** — K-1〜K-5 (#350〜#354) 全 close、K-6 は #345 に吸収
- ✅ **#332 gamestate pivot** — Phase A/B 両 land (#337/#338)
- ✅ **#163 Faction epic** — close (#174 は 1.0.0 へ移籍済)
- ✅ **#286 Fleet epic** — close
- ✅ #296 S-3 / #297 S-2 / #289 β View types / #284 profiling
- ✅ #334 Command dispatch refactor
- ✅ #335 Biome / #336 Colony.Owner
- ✅ #364 ship regression / #365 resource trends bug

---

## 着手推奨

1. Phase 2a の **#298 / #299 / #300 / #303** を 4 worktree agent で同時 kick
2. **#280 Colony Hub** を並走
3. Phase 2b → 3 → 4 は Phase 2a 成果物のマージ状況を見て次 sprint を組む

---

## 関連 docs

- `docs/diplomacy-design.md` — diplomacy v2 spec (#302/#304/#305/#321-325 全 reference)
- `docs/plan-296-infrastructure-core-deliverable.md` — S-3 計画 (S-4/S-5 の前提設計)
