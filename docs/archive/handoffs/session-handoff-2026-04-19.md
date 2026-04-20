# Session Handoff 2026-04-19/20

## 成果サマリ

### Close した issue (14件)
| # | タイトル |
|---|---------|
| #393 | territory visualization 表示されない (shader import fix) |
| #396 | installation hull 通常建造制限 (is_direct_buildable) |
| #395 | station ship UI 分離 (immobile ships) |
| #391 | ModifiedValue tooltip (ship panel) |
| #291 | fleet_system_entered/left Lua events |
| #399 | range-aware combat sim + ECS bridge |
| #190 | AI combat projection Phase 1 |
| #138 | slot types + power budget + weapon size |
| #397 | AI debug log (JSONL, feature-gated) |
| #402 | combat retreat (ROE-based, distance disengage) |
| #290 | building_lost typed EventContext |
| #219 | Port static defense (point_defense_turret) |
| #403 | module size variants (S/M/L) |
| #398 | Observer mode (banner + faction selector + read-only) |
| #407 | Fleet ops + multi-select + fleet hierarchy |

### 起票した issue (8件)
| # | タイトル | Milestone |
|---|---------|-----------|
| #401 | 戦闘バランス (closed by user) | 0.3.0 |
| #402 | 撤退メカニクス (closed) | 0.3.0 |
| #403 | module size variants (closed) | 0.3.0 |
| #404 | Break Alliance バグ | - |
| #405 | 未発見 faction 表示バグ | - |
| #406 | outline station 視覚区別 | 0.3.0 |
| #408 | outline context menu | 0.3.0 |
| #409 | ship 破壊通知 + 光速遅延 + 推定位置 | 0.3.0 |
| #410 | FTL routing bug (unsurveyed→unsurveyed) | - |
| #411 | 戦闘レポート + アノマリー調査統合 | 0.4.0 |

### 主要な新機能/基盤
- **AI パイプライン end-to-end**: emitters (38 military + 28 economic) → bus → SimpleNpcPolicy → CommandDrain → ship movement
- **Combat sim**: pure function, range-aware, weakest-first targeting, shield regen delay, retreat mechanics, 22-scenario analysis
- **Fleet ops**: SelectedShips multi-select, Form/Merge/Dissolve Fleet, fleet-level MoveTo, outline hierarchy
- **Observer mode**: --observer CLI, faction selector, read-only UI, ground-truth resources
- **AI debug log**: --features ai-log, JSONL 2-stream (decision + world state)
- **--ai-player**: player empire AI-controlled

## 未完了 / 次セッションで対応

### #368 ダブルクリック cycle (実装済み、merge conflict)
agent が実装完了したが visualization/mod.rs が #398 (observer) の変更と conflict。新しい main で再実行が必要。

### #406 + #408 outline UX (実装済み、merge conflict)  
outline.rs が #407 (fleet hierarchy) の変更と conflict。1回目も2回目もタイムアウト/conflict。新しい main で再実行。

### 0.3.0 残り issue
| # | タイトル | 状態 |
|---|---------|------|
| #368 | ダブルクリック cycle | 再実行必要 |
| #406 | outline station 区別 | 再実行必要 |
| #408 | outline context menu | 再実行必要 |
| #409 | ship 破壊通知 + 光速遅延 | 未着手 |
| #410 | FTL routing bug | 未着手 |
| #404 | Break Alliance バグ | 未着手 |
| #405 | 未発見 faction 表示 | 未着手 |
| #392 | visibility tier | 未着手 (大) |
| #310 | Lua コンソール | 未着手 |
| #268 | Courier relay | 未着手 |
| #204 | FleetCombatCapability | 未着手 |
| #189 | AI umbrella | 子 issue 進行中 |

## 設計上の判断・議論メモ

### 戦闘投影 = simulate_combat の dry-run
- 抽象化 DPS profile ではなく、実際の simulate_combat を Monte Carlo で回す方針
- パフォーマンスは 1 回 μs オーダーで問題なし (combat_sim_analysis で検証済み)

### 光速制約と ship 表示
- ship は「命令キューから推定した位置」で ghost 表示すべき
- 破壊 = 「帰ってこない」→ 行方不明 → 偵察/光速で確定
- 戦闘残骸をアノマリーとして統合 (#411)

### power を ModifiedValue 化 (#403 follow-up)
- power_output / power_cost を ScopedModifiers にして tech/event で buff 可能に
- 余剰 power ボーナス（shield_regen 等）は将来拡張

### 戦闘バランス
- combat_speed 非線形化 `v/(1-v)` は別 issue で
- distance_step_factor を下げて「射程内だけど距離制御に時間がかかる」
- railgun DPS 不足問題は #401 (closed) + module size variants (#403) で改善方向
