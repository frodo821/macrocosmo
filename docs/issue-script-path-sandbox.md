# スクリプトパス解決の堅牢化 + Lua サンドボックス

**Labels:** `foundation`, `priority:medium`

## 概要

#130 で導入した `scripts/init.lua` による一括ロード方式にはパス解決とセキュリティの2つの課題がある。

## 1. アセットバンドル時のパス解決

### 現状の問題

```rust
// src/scripting/mod.rs
let init_path = Path::new("scripts/init.lua");
```

- CWD からの相対パスに依存しており、開発時（`cargo run`）は動作するが以下のケースで壊れる可能性がある:
  - ゲームをバイナリとして配布し、別ディレクトリから起動した場合
  - Bevy の asset system 経由でバンドルした場合
  - プラットフォーム固有のインストールパスに配置された場合

- `package.path` も同様に相対パス:
  ```rust
  package.set("path", "scripts/?.lua;scripts/?/init.lua")?;
  ```

### 対応案

- **Bevy AssetServer との統合**: `AssetServer::asset_path()` を使って scripts/ ディレクトリの絶対パスを解決する
- **複数パスの探索**: 以下の優先順位でスクリプトディレクトリを検索:
  1. ユーザー指定のモッドディレクトリ（将来のmod対応）
  2. 実行ファイルと同階層の `scripts/`
  3. Bevy のアセットディレクトリ内の `scripts/`
  4. CWD からの `scripts/`（開発時フォールバック）
- **`package.path` の動的設定**: 解決した絶対パスに基づいて `package.path` を設定する

## 2. Lua サンドボックス

### 現状の問題

- `Lua::new()` で全標準ライブラリがロードされており、Lua スクリプトから以下が可能:
  - `io.open()` / `io.popen()` によるファイルシステムアクセス・コマンド実行
  - `os.execute()` / `os.remove()` によるシステムコマンド実行
  - `loadfile()` / `dofile()` による任意ファイル実行
  - `debug` ライブラリによる内部状態の操作
- mod サポートを将来想定する場合、サードパーティスクリプトが任意コードを実行できるのは危険

### 対応案

- **不要なライブラリの無効化**: `Lua::new()` の代わりに `Lua::new_with()` で安全なライブラリのみロード:
  ```rust
  let lua = Lua::new_with(StdLib::TABLE | StdLib::STRING | StdLib::MATH | StdLib::PACKAGE, LuaOptions::default())?;
  ```
  - 許可: `table`, `string`, `math`, `package`（require 用）, `coroutine`（将来用）
  - 禁止: `io`, `os`, `debug`, `ffi`
- **`require` のホワイトリスト化**: `package.searchers` をカスタムサーチャーに置き換え、`scripts/` 配下のファイルのみロード可能にする（パストラバーサル防止）
- **グローバル関数のフィルタリング**: `loadfile`, `dofile`, `load`（文字列からコード生成）を nil に設定
- **メモリ・実行時間制限**: `mlua` の `set_memory_limit` / hook による CPU 時間制限（mod 対応時）

## 実装の優先度

1. **サンドボックス**（即時）: 不要なライブラリの無効化は低コストで効果が大きい
2. **パス解決**（配布前）: アセットバンドルの仕組みが固まってから対応

## 関連ファイル

- `src/scripting/mod.rs` — `ScriptEngine::new()`, `setup_globals()`, `load_all_scripts()`
- `scripts/init.lua` — エントリポイント

## 関連 issue

- #130 — Lua API 整理（本 issue の前提）
