# Session Handoff 2026-04-20

## 成果サマリ

### Close した issue (本セッション追加分: 10件、通算 24件)

| # | タイトル |
|---|---------|
| #398 | Observer mode (banner + faction selector + read-only UI) |
| #403 | Module size variants (S/M/L Lua 定義) |
| #406 | Outline station 視覚区別 (teal + anchor icon) |
| #408 | Outline context menu (ship/station 右クリック) |
| #368 | ダブルクリック cycle (overlapping entities) |
| #404 | Break Alliance バグ (builtin option filter) |
| #405 | 未発見 faction 表示バグ (KnownFactions) |
| #204 | FleetCombatCapability + evidence emission |
| #189 | AI システム umbrella (Phase 1 complete) |
| #268 | Courier relay (opportunistic command carry + dedup) |
| #310 | Lua コンソール (Alt+F2 + LogBuffer) |

### 起票した issue
| # | タイトル | Milestone |
|---|---------|-----------|
| #409 | ship 破壊通知 + 光速遅延 + 推定位置 | 0.3.0 |
| #410 | FTL routing bug (unsurveyed→unsurveyed) | 0.3.0 |
| #411 | 戦闘レポート + アノマリー調査統合 | 0.4.0 |
| #412 | バランス定数再設計 (duration→speed + system difficulty) | 0.4.0 |
| #157 更新 | Lua UI フレームワーク full spec | 0.4.0 |

## 0.3.0 残り (3件)

| # | タイトル | 規模 | メモ |
|---|---------|------|------|
| #410 | FTL routing bug | 中 | 再現条件不明、要デバッグ |
| #409 | ship 破壊通知 + 光速遅延 | 大 | #392 と同根 |
| #392 | visibility tier | 大 | 光速制約の根幹、KnowledgeStore ベースの表示切り替え |

## 主要な新機能/基盤 (本セッション)

### AI パイプライン完全完成
- Evidence emission (direct_attack, hostile_engagement, fleet_loss) → standing → threat_level → feasibility 全パイプライン稼働
- #189 umbrella close。Intent-based delegation は 0.4.0

### Observer mode
- `--observer` CLI flag (implies `--ai-player`)
- Faction selector ComboBox + read-only UI + ground-truth resource display

### Lua コンソール
- Alt+F2 toggle
- LogBuffer with SharedPrintBuffer (Arc<Mutex>) bridge
- print redirect: tee to stdout + buffer
- Expression-first eval + history navigation

### Courier relay
- CommandId dedup (同一命令は 1 回だけ apply)
- Directional pickup (dot product > 0)
- Closest-approach waypoint release
- PendingCommand 永続化対応

### UI 改善
- ダブルクリック cycle (CycleSelection + SelectionState)
- Outline station 区別 (teal + anchor/sword icons)
- Outline context menu (right-click on ship/station)
- KnownFactions (未発見 faction 非表示)
- Break Alliance builtin option filter

## 設計上の判断・議論メモ

### Lua UI フレームワーク (#157)
- **UI = 関数**: contract (入力契約) を持つ compiled layout
- **define 時に compile**: layout tree → Vec<CompiledWidget> (egui 命令列)、毎フレームは slot fill のみ
- **Condition requires**: ボタンの disabled 理由を tooltip 表示
- **action = event 発火**: fire_event 基盤 + syntactic sugar (build_ship, move_to 等)
- DOM 方式は不採用（egui が即時モードなので retained tree と相性悪い）

### バランス定数再設計 (#412)
- survey_duration → survey_speed × survey_difficulty (system 属性)
- 初期能力を base=0 + initial tech で表現（将来）
- 「いつ帰ってくるかわからない」= 光速制約ゲームの面白さ

### 戦闘残骸 = アノマリー (#411)
- アノマリーを「未調査の情報断片」として汎用化
- 発見 → 名前のみ → 調査 → 詳細解放
- 全滅 ship の戦闘ログ復元もアノマリー調査として統合

### BRP ドキュメント更新
- remote.rs ヘッダー: 全 9 custom methods を記載
- CLAUDE.md: BRP セクション追加
- Memory: reference_egui_testing.md 更新

## 次セッションの優先事項

1. **BRP e2e テスト実行** — `--observer --features remote` で AI 動作を外部から監視 + 異常検出
2. **#410 FTL routing bug デバッグ** — plan_ftl_route に trace 仕込み
3. **#392 + #409** — visibility tier + 光速遅延表示（0.3.0 最後の大物）
