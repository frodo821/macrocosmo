# 勢力 (Faction) システム + 外交設計

親 issue #163 の設計ドキュメント。issue クローズに伴いここに保存。
個別の実装作業は #167〜#174 の各サブ issue を参照。

## 概要

プレイヤー帝国・NPC 帝国・宇宙生物を統一的に扱う勢力 (Faction) システム。
FactionRelations は非対称 — 光速遅延により A→B と B→A の認識がズレうる。

## 設計要点

### 非対称 FactionRelations
- `(from, to) → FactionView` の非対称ペア
- A が B に宣戦布告 → A 側は即 War、B 側は光速遅延後に War
- 奇襲が成立（宣戦布告より艦隊が先に着く場合）

### RelationState

| State | 意味 |
|-------|------|
| Neutral | 外交関係なし。standing < 0 + ROE=Aggressive で交戦可 |
| Peace | 不可侵。宣戦布告なしに攻撃不可 |
| War | 交戦中 |
| Alliance | 同盟 |

### Standing
- -100 〜 +100 の連続値
- Neutral + standing < 0 = 事実上の敵対（宇宙生物等）
- state 遷移は外交アクションで行い、光速遅延が適用される

## サブ issue

| # | タイトル | 優先度 | 依存 |
|---|---------|--------|------|
| #167 | FactionRelations + 非対称 Standing | medium | #165 ✅ |
| #168 | HostilePresence → Faction 移行 | medium | #167, #170 |
| #169 | ROE を Standing ベースに | medium | #167 |
| #170 | define_faction_type Lua API | medium | #165 ✅ |
| #171 | 外交コマンドの光速遅延伝搬 | low | #167, #170 |
| #172 | define_diplomatic_action Lua API | low | #171, #153 ✅ |
| #173 | NPC 帝国生成 + 基本 AI | low | #167, #170, #171, #166 ✅ |
| #174 | 外交 UI パネル | low | #172, #173 |

## 依存グラフ

```
#165 Faction基盤 ✅
 ├── #167 FactionRelations ──┬── #168 HostilePresence移行
 │                           ├── #169 ROE拡張
 │                           ├── #171 外交遅延 ── #172 diplomatic_action ── #174 外交UI
 │                           └── #173 NPC AI ─────────────────────────────────┘
 └── #170 define_faction_type ─┤
                               └── #168
```
