# Macrocosmo

[![Latest Release](https://img.shields.io/github/v/release/frodo821/macrocosmo?label=latest)](https://github.com/frodo821/macrocosmo/releases/latest)

A space 4X strategy game where **the speed of light is the core game mechanic** — built with Rust + Bevy.

光速通信の遅延をゲームメカニクスの中核に据えた宇宙 4X 戦略ゲーム。

> **⚠ Pre-alpha** — 仕様・内容は大きく変化します。

---

## プレイヤー向け

### どんなゲームか

Stellaris ライクな宇宙 4X ストラテジーですが、**光速を超える情報伝達が (デフォルトでは) できません**。

- 遠方の星系の情報は、距離に応じた分だけ**過去の姿**しか見えない
- 遠隔地への命令は光速で伝わるまで**届かない**
- プレイヤーは物理的な位置を持ち、**その場に居るときだけ即座に命令できる**
- 遠隔地は自律 AI に「意図 (intent)」を委任し、AI が現地判断で執行する

→ プレイヤーは**不確実性の中で委任と計画を組み立てる**ことを強いられます。直接管理するために物理的に遠征する価値が自然に生まれ、委任と直接指揮のトレードオフがゲームの軸になります。

### 特徴

- **FTL は弾道飛行**: ジャンプ中は観測・通信ともにブラックアウト
- **情報は必ず古い**: 数十光年離れた星系の現在は、10 年後にしか知れない
- **Hexadies 時間系**: 1 hexadies = 6 日、内部は整数ゲーム時間
- **命令価値システム**: 遠隔地への命令は遅延に応じて価値が減衰し、同一対象への競合命令は最新のものが勝つ
- **全データは Lua スクリプト定義**: 技術・建造物・艦船・星系タイプ・イベントなど、ゲーム内容の大半は `scripts/` 配下の Lua で定義されており、Rust は純粋なエンジン層

### 現在遊べる要素 (v0.1.0)

- 銀河生成 (渦巻き 150 星系、3D 座標)
- 探索・測量・入植
- コロニー経営 (鉱物 / エネルギー / 研究)
- 技術研究 (4 ブランチ)
- 艦船設計 (hull + module 組み合わせ)
- 艦隊編成・FTL / 亜光速移動
- 星間アノマリー発見
- Faction 外交データモデル (対戦相手はまだ未実装)

### まだ無いもの

- NPC 帝国 (AI、0.3.0 予定)
- 外交 UI (0.3.0 予定)
- 地上戦闘 / 軌道爆撃 (1.0.0 予定)
- セーブ / ロード (未実装)

### プレイ方法

#### バイナリをダウンロード

[Releases ページ](https://github.com/frodo821/macrocosmo/releases/latest) から OS に合わせたビルドをダウンロードできます:

| プラットフォーム | ファイル |
|--------------|---------|
| macOS (Apple Silicon) | `macrocosmo-macos-aarch64.tar.gz` |
| Windows (x86_64) | `macrocosmo-windows-x86_64.zip` |
| Linux (x86_64) | `macrocosmo-linux-x86_64.tar.gz` |

展開して `macrocosmo` (Windows は `macrocosmo.exe`) を実行するだけで動きます。`scripts/` と `assets/` はバイナリと同じ階層に同梱されています。

#### ソースからビルド

```bash
git clone https://github.com/frodo821/macrocosmo.git
cd macrocosmo
cargo run --release -p macrocosmo
```

Rust (edition 2024 対応) と、luajit をビルドできる C コンパイラ (Xcode Command Line Tools / gcc 等) が必要です。Linux では追加で `libasound2-dev libudev-dev libwayland-dev libxkbcommon-dev libvulkan-dev` が必要です。

### 操作方法

| カテゴリ | キー | 動作 |
|---------|------|------|
| カメラ | W/A/S/D または 矢印キー | パン |
| カメラ | マウスホイール | ズーム |
| カメラ | Home | 中心に戻る |
| 時間 | Space | ポーズ / 再開 |
| 時間 | `=` / `-` | ゲーム速度 倍 / 半 |
| 選択 | マウス左クリック | 星系・艦船を選択 |
| 選択 | Esc | 選択解除 |
| 入植 | C | 選択中の入植船から入植 |
| 情報 | I | プレイヤー位置パネル |

より詳しい遊び方は [`docs/game-design.md`](docs/game-design.md) を参照。

---

## コントリビュータ向け

### 技術スタック

- **Rust** (edition 2024)
- **Bevy 0.18.1** — ECS ゲームエンジン
- **bevy_egui 0.39.1** — UI
- **mlua 0.11** (luajit, vendored) — ゲームデータ定義の Lua スクリプティング
- **rand 0.9**

### ワークスペース構成

```
.
├── macrocosmo/         # メインゲーム crate (bin)
│   ├── src/            # Rust ソース (ECS systems, gameplay logic)
│   ├── scripts/        # Lua ゲームデータ定義 (tech / ships / buildings ...)
│   ├── assets/         # シェーダ・画像
│   └── tests/          # 統合テスト
├── macrocosmo-ai/      # AI サブシステム crate (0.3.0 で本格開発)
├── docs/               # 設計ドキュメント
│   ├── game-design.md  # ゲームデザイン全体
│   ├── diplomacy-design.md
│   └── handoff-*.md    # セッション引き継ぎ
└── CLAUDE.md           # アーキテクチャ要点 + コーディング規約
```

### ビルド・テスト・実行

```bash
# ビルド
cargo build --release

# テスト (460+ 件、並列実行)
cargo test

# フラッキーなテスト (async route timing) を避ける場合
cargo test -- --test-threads=1

# 実行
cargo run --release -p macrocosmo
```

### アーキテクチャ概要

詳細は [`CLAUDE.md`](CLAUDE.md) を参照。要点:

- **Bevy ECS** + Plugin 構成、システムは `Update` / `EguiPrimaryContextPass` に登録
- **3 フェーズ銀河生成** (empty → capitals → init)
- **Capability-based 定義**: 建造物・構造物・艦船設計は Lua の `capabilities` テーブル
- **Scoped Conditions / Effects**: 技術やイベントの条件式を Lua 関数 / テーブルで記述可能
- **KnowledgeStore**: 遠隔地の情報は光速遅延付きスナップショットとして保持され、UI / visualization はこれを参照
- **Async A\* routing**: FTL / 亜光速ハイブリッド経路は `AsyncComputeTaskPool` で計算し、ゲーム時間を伸縮させてフレームペースを維持

### Lua モッディング層

`scripts/` 配下で**技術・建造物・艦船・星系タイプ・イベント・種族・Faction** などが定義されています。Rust コード変更なしで:

- 技術ツリーの変更
- 新しい艦船モジュール / 建造物の追加
- Faction タイプ・外交アクションの定義
- イベント・アノマリーの追加

が可能です。`scripts/init.lua` が単一エントリポイントで、`require()` で各サブシステムをロードします。

### 開発フロー

**Issue 管理**:
- [GitHub Issues](https://github.com/frodo821/macrocosmo/issues) + milestone 管理
- 現在のロードマップ: **0.2.0** (光速制約完成) → **0.3.0** (NPC AI) → **1.0.0** (軍事拡張 + polish)
- Label 軸: `priority:*` (urgent/high/medium/low/icebox)、`theme:*` (ai/diplomacy/deep-space/military/modding/polish)、`epic` (umbrella issue)
- Project ボード: [Macrocosmo Roadmap](https://github.com/users/frodo821/projects/1)

**並行開発**:
- 独立な issue は **git worktree + Claude Code agent** で並行実装
- 完了後にマージ agent が main に統合

**コミット前チェック**:
- `cargo test` で全テスト通過 (特に `all_systems_no_query_conflict` は Bevy の B0001 クエリ競合を検出)
- `cargo fmt` / `cargo clippy -- -D warnings`

**バグ修正には必ず回帰テストを追加**。詳細は [`CLAUDE.md`](CLAUDE.md) の "Common Pitfalls" 参照。

### リリース

リリースは `release` ブランチへの push で自動化されています ([`.github/workflows/release.yml`](.github/workflows/release.yml)):

1. `Cargo.toml` の `[workspace.package] version` を次バージョンに bump
2. `main` から `release` ブランチへマージ (PR)
3. `v{version}` タグ作成・macOS / Windows / Linux 向けバイナリビルド・GitHub Release 発行が自動で走る
4. Build provenance attestation も自動付与

同じバージョンの tag が既存の場合 preflight が失敗するため、**必ずバージョンを上げてから release へマージ**してください。

### ドキュメント

| ファイル | 内容 |
|---------|------|
| [`CLAUDE.md`](CLAUDE.md) | アーキテクチャ要点・コーディング規約・Common Pitfalls |
| [`docs/game-design.md`](docs/game-design.md) | ゲームデザイン全体仕様 |
| [`docs/diplomacy-design.md`](docs/diplomacy-design.md) | Faction / 外交システム設計 |
| `docs/handoff-*.md` | 過去セッションの引き継ぎ記録 |

### ロードマップ

| milestone | テーマ | 主要 issue |
|-----------|--------|-----------|
| **0.2.0** | 光速制約の完成 + モッド基盤 | #119 FTL Comm Relay / #145 FTL 不可領域 / #152 選択肢 UI / #160 バランス定数 Lua 化 / #182 マップタイプ API / #199 connectivity bridge |
| **0.3.0** | NPC AI + 外交完結 | #189 AI umbrella + サブ #190-198 / #174 外交 UI |
| **1.0.0** | 軍事拡張 + ビジュアル仕上げ | #139 軌道爆撃 / #140 惑星破壊兵器 / #184 地上戦闘 / #143 UI アイコン ほか |

詳細は [milestones 一覧](https://github.com/frodo821/macrocosmo/milestones) を参照。

### ライセンス

[BSD 3-Clause License](LICENSE) — Copyright (c) 2026, frodo821

---

## Credits

- Engine: [Bevy](https://bevyengine.org/)
- Scripting: [mlua](https://github.com/mlua-rs/mlua) / [LuaJIT](https://luajit.org/)
- UI: [egui](https://github.com/emilk/egui) via [bevy_egui](https://github.com/vladbat00/bevy_egui)
