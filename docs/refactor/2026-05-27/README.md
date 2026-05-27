# Refactor Worklog: 2026-05-27

## Purpose

描画・入力・UI の変更がゲームループを壊し、逆にゲームループ変更が描画を壊す現状を解消するための作業ディレクトリ。

この日の結論は、crate 分割そのものより先に、同一 crate 内で `simulation` と `interactions` の内部境界を作ること。

## Documents

- `plan.md`: SRP / DRY / crate 境界を含む詳細計画。現在の主計画。
- `evidence.md`: 調査コマンド、観察結果、分離優先の根拠。
- `decision-log.md`: 判断の変遷と採用した方針。
- `remaining-work.md`: 実装後の残作業、PR 分割、検証コマンド。

## Current Recommendation

最初のPRは crate 追加ではなく、既存 `macrocosmo` crate 内に `SimulationPlugin` と `InteractionsPlugin` の境界を作る。

Target order:

1. `SimulationPlugin` を導入し、authoritative game loop を UI/描画/input なしで起動可能にする。
2. `InteractionsPlugin` を導入し、UI / visualization / input / remote / observer controls をそこに寄せる。
3. `time_system`, `observer`, `notifications`, `choice` の混在責務を先に分ける。
4. Headless simulation smoke test を追加する。
5. 境界が安定してから `macrocosmo-core`, `macrocosmo-simulation`, `macrocosmo-interactions` の crate 分割へ進む。

## Success Criteria

- `SimulationPlugin` が `InteractionsPlugin` なしで少なくとも1 update進む。
- simulation 側 production code が `bevy_egui`, `egui::`, `crate::ui`, `crate::visualization`, `crate::input`, `KeyCode`, `ButtonInput`, `Window` を import しない。
- interactions 側は simulation state を読むか simulation command を送るだけで、ゲームループの lifecycle owner にならない。
- desktop app は `SimulationPlugin + InteractionsPlugin` の合成として動く。
