# TODO — open issue 棚卸し (2026-04-18 時点)

前回 (2026-04-17) から大量 close: #372 SystemBuilding Ship 統一 epic 全完了 (A-H)、#390 BRP test framework 完了、Sovereignty Phase 2 sub-issue S-4〜S-7 + S-10 全完了、Colony Hub #280 完了、#369/#370 bug 修正済。

## マイルストーン概況

| ms | open | closed | 備考 |
|---|---|---|---|
| 0.3.0 | 20 | 47 | Diplomacy v2 + AI + Sovereignty 残 + visibility |
| 0.4.0 | 3 | 0 | 静的防御 + keybinding |
| 1.0.0 | 11 | 0 | 後回し固定 |
| no-ms | 1 | — | 戦闘 epic umbrella のみ |

## 0.3.0 残 issue (20 件)

### AI system (#189 epic)

| # | title | pri | deps | 備考 |
|---|---|---|---|---|
| #189 | ゲーム AI システム (umbrella) | high | Diplomacy + Combat + Visibility | epic |
| #190 | AI 戦闘投影モデル | medium | — | Nash feasibility core |
| #204 | FleetCombatCapability | medium | #190 | Phase 3 実証 |

### Sovereignty Phase 2 (#292 epic) 残 3 件

S-1〜S-7 + S-10 完了済。残:

| # | title | pri | deps | 備考 |
|---|---|---|---|---|
| #292 | Sovereignty Phase 2 epic | high | #302/#305 | S-8〜S-11 完了で close |
| #302 | S-8 DiplomaticOption framework | medium | #321/#322 | Inbox dispatch |
| #304 | S-9 Diplomacy UI | medium | #302 | 4 panel |
| #305 | S-11 Casus Belli system | medium | #302/#321/#322 | end_scenarios + auto_war |

### Diplomacy v2 foundation (5 件)

| # | title | pri | deps | 備考 |
|---|---|---|---|---|
| #321 | define_negotiation_item_kind Lua API | medium | — | 8 kind 定義 |
| #322 | Condition atom 拡張 (9 種) | medium | — | UI walk 前提 |
| #323 | faction_type → instance preset 化 | low | — | runtime type 排除 |
| #324 | Annihilation handling | medium | — | Extinct + history |
| #325 | DiplomaticAction enum 廃止 | medium | #302 | breaking change |

### Visibility

| # | title | pri | 備考 |
|---|---|---|---|
| #392 | connection-based visibility tier | high | Catalogued/Surveyed/Connected/Local、relay 既存 |

### Military / Deep space

| # | title | pri | 備考 |
|---|---|---|---|
| #219 | Port 戦闘参加 (D-1) | medium | #372 で Port Ship 化済、combat 参加の wire 残? 要確認 |
| #268 | Courier opportunistic relay | medium | command ID dedup |

### Modding / UX

| # | title | pri | 備考 |
|---|---|---|---|
| #290 | building_lost typed EventContext | low | 軽微 migration |
| #291 | fleet_system_entered/left event | medium | 新 event 定義 |
| #310 | Lua コンソール | low | egui floating panel |
| #368 | ダブルクリック cycle 選択 | medium | UX polish |
| #391 | ModifiedValue tooltip (modifier breakdown) | medium | UX polish |

## 0.4.0 (3 件)

| # | title | pri | 備考 |
|---|---|---|---|
| #213 | 静的防御 implementation epic | medium | port 武装 + 防衛施設 |
| #220 | 防衛プラットフォーム (D-2) | medium | 新 hull 定義 (Ship 化済) |
| #347 | keybinding manager | medium | ESC / Lua console 向け |

## no-ms

| # | title | pri | 備考 |
|---|---|---|---|
| #211 | 戦闘 epic (umbrella) | high | 静的防御 + 地上戦 |

## 1.0.0 (後回し固定)

#184 地上戦、#174 外交 UI (旧)、#143 UI icons、#139 軌道爆撃、
#120/#218 wake 系、#121 Interdictor、#135 テクスチャ、#157 Lua UI パネル、
#140 惑星破壊、#61 バランス調整

---

## 着手順 (並列 sprint 単位)

### Sprint A — Diplomacy v2 foundation + Visibility (並列)

Diplomacy foundation (4 並列 kick):
- #321 define_negotiation_item_kind
- #322 Condition atom 拡張
- #323 faction_type preset 化
- #324 Annihilation handling

独立:
- **#392 Visibility tier** (high、独立に着手可)
- #268 Courier relay (独立)

隙間:
- #291 fleet events
- #290 building_lost migration
- #368 ダブルクリック cycle
- #391 modifier tooltip

### Sprint B — Diplomacy v2 mechanic (Sprint A land 後)

- #302 S-8 DiplomaticOption framework (requires #321/#322)
- #325 DiplomaticAction enum 廃止 (#302 後)

### Sprint C — Diplomacy v2 UI + Casus Belli (Sprint B land 後)

- #305 S-11 Casus Belli (requires #302/#321/#322)
- #304 S-9 Diplomacy UI 4 panel (requires #302)
- → #292 Sovereignty Phase 2 epic close

### Sprint D — AI

- #190 AI 戦闘投影 → #204 FleetCombatCapability
- → #189 AI epic 着手可
- 並列: #219 Port 戦闘参加 (要確認: #372 で解決済?)

---

## 完了済 epic / 大物 (直近)

- ✅ **#372 SystemBuilding Ship 統一** — A-H 全 8 sub-issue close (harbour + DockedAt + modifier routing)
- ✅ **#390 BRP test framework** — bevy_remote + custom RPC
- ✅ **#280 Colony Hub Phase 1**
- ✅ Sovereignty S-4 (#298) / S-5 (#299) / S-6 (#300) / S-7 (#301) / S-10 (#303)
- ✅ #369 Core 表示 / #370 Core 建造 gate / #371 hull category

## 0.3.0 膨張に関する所感

0.3.0 は 20 open / 47 closed と大規模。依存チェーンが Diplomacy v2 → Sovereignty → AI と深い。milestone 分割の検討余地あり:
- 0.3.0: Diplomacy v2 + Visibility + Sovereignty 残 (Sprint A-C)
- 0.4.0 へ繰延: AI 本体 (#189/#190/#204) — 前提全部揃ってから
- 現 0.4.0 (静的防御 + keybinding) は 0.5.0 へ

---

## 関連 docs

- `docs/diplomacy-design.md` — diplomacy v2 spec (#302/#304/#305/#321-325 全 reference)
- `docs/plan-296-infrastructure-core-deliverable.md` — S-3 計画 (完了済)
