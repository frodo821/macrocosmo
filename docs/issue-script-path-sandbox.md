# スクリプトパス解決の堅牢化 + Lua サンドボックス

**Status:** ✅ Implemented in #130 (PR #144)

## 1. アセットバンドル時のパス解決 ✅

`resolve_scripts_dir()` が以下の優先順位でスクリプトディレクトリを探索:
1. 実行ファイルと同階層の `scripts/`
2. CWD からの `scripts/`
3. `CARGO_MANIFEST_DIR` の `scripts/`（開発時フォールバック）

解決された絶対パスに基づいて `package.path` を設定。

## 2. Lua サンドボックス ✅

`Lua::new_with()` で安全なライブラリのみロード:
- 許可: `table`, `string`, `math`, `package`（require 用）, `bit`
- 禁止: `io`, `os`, `debug`, `ffi`
- `loadfile`, `dofile` は nil に設定
- `package.cpath` は空文字列（C モジュール無効化）

## 将来の課題

- **`require` のホワイトリスト化**: 現在は `package.path` で `scripts/` 配下に限定しているが、パストラバーサルの防止は未実装
- **メモリ・実行時間制限**: mod 対応時に `mlua` の `set_memory_limit` / hook による制限を検討
- **Bevy AssetServer 統合**: 現在は独自のファイルI/O。将来的に Bevy のアセットパイプラインとの統合を検討

## 関連ファイル

- `src/scripting/mod.rs` — `ScriptEngine::new()`, `resolve_scripts_dir()`, `setup_globals()`, `load_all_scripts()`
- `scripts/init.lua` — エントリポイント
