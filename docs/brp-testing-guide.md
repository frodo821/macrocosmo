# BRP Testing Guide

Macrocosmo exposes a JSON-RPC 2.0 testing interface via Bevy Remote Protocol (BRP).
This guide covers how to enable it, what methods are available, and how to use
them for automated and manual testing.

## Enabling BRP

Build and run with the `remote` feature:

```bash
cargo run -p macrocosmo --features remote
```

The BRP HTTP server listens on **`localhost:15702`** (Bevy default).

All custom methods are gated behind `#[cfg(feature = "remote")]` in
`macrocosmo/src/remote.rs`. The feature pulls in `bevy/bevy_remote`, `base64`,
and `image` as dependencies.

## Protocol basics

Every request is a JSON-RPC 2.0 POST to `http://localhost:15702`:

```bash
curl -s -X POST http://localhost:15702 \
  -H "Content-Type: application/json" \
  -d '{"jsonrpc":"2.0","id":1,"method":"<METHOD>","params":{...}}'
```

Responses contain either `"result"` or `"error"`.

## Available methods

### Built-in Bevy methods

| Method | Description |
|---|---|
| `bevy/list` | List all registered BRP methods |
| `bevy/query` | Query entities by component type |
| `bevy/get` | Get components from a specific entity |
| `bevy/insert` | Insert components on an entity |
| `bevy/remove` | Remove components from an entity |
| `bevy/spawn` | Spawn a new entity |
| `bevy/despawn` | Despawn an entity |

### Custom game methods

| Method | Params | Response |
|---|---|---|
| `macrocosmo/entity_screen_pos` | `{ entity: u64 }` | `{ x, y, visible }` |
| `macrocosmo/advance_time` | `{ hexadies: i64 }` | `{ elapsed }` |
| `macrocosmo/eval_lua` | `{ code: string }` | `{ result: string }` |
| `macrocosmo/click` | `{ x, y, button? }` | `{ status, x, y, button }` |
| `macrocosmo/key_press` | `{ key: string }` | `{ status }` |
| `macrocosmo/hover` | `{ x, y }` | `{ status, x, y }` |
| `macrocosmo/screenshot` | `{}` | `{ base64, width, height }` |

#### Key names for `key_press`

Letters (`A`-`Z`, case-insensitive), digits (`0`-`9`), function keys (`F1`-`F12`),
arrows (`ArrowUp`/`Up`, etc.), modifiers (`ShiftLeft`, `ControlLeft`/`CtrlLeft`,
`AltLeft`, `SuperLeft`/`MetaLeft`/`CmdLeft`), and common punctuation
(`Escape`/`Esc`, `Space`, `Enter`/`Return`, `Tab`, `Backspace`, `Delete`, etc.).

#### Mouse buttons for `click`

`"left"` (default), `"right"`, `"middle"`.

#### Screenshot two-call pattern

The first `macrocosmo/screenshot` call triggers a capture and returns an error
asking the client to retry. The screenshot is buffered after the next rendered
frame. A second call returns the base64-encoded PNG:

```bash
# Trigger capture (ignore the error)
curl -s -X POST localhost:15702 \
  -d '{"jsonrpc":"2.0","id":1,"method":"macrocosmo/screenshot","params":{}}' > /dev/null
sleep 0.5
# Retrieve the buffered image
curl -s -X POST localhost:15702 \
  -d '{"jsonrpc":"2.0","id":2,"method":"macrocosmo/screenshot","params":{}}' \
  | jq -r .result.base64 | base64 -d > /tmp/game.png
```

## Example workflows

### Verify ship spawn

```bash
curl -s -X POST localhost:15702 \
  -d '{"jsonrpc":"2.0","id":1,"method":"bevy/query","params":{"data":{"components":["macrocosmo::ship::Ship"]}}}'
# Expect: non-empty result array
```

### Advance time and check clock

```bash
curl -s -X POST localhost:15702 \
  -d '{"jsonrpc":"2.0","id":1,"method":"macrocosmo/advance_time","params":{"hexadies":60}}'
# Returns { "elapsed": <new_total> }
```

### Evaluate Lua expression

```bash
curl -s -X POST localhost:15702 \
  -d '{"jsonrpc":"2.0","id":1,"method":"macrocosmo/eval_lua","params":{"code":"return 2 + 2"}}'
# Returns { "result": "4" }
```

### UI interaction via input injection

```bash
# Press Escape to close any open panel
curl -s -X POST localhost:15702 \
  -d '{"jsonrpc":"2.0","id":1,"method":"macrocosmo/key_press","params":{"key":"Escape"}}'

# Click a screen coordinate
curl -s -X POST localhost:15702 \
  -d '{"jsonrpc":"2.0","id":2,"method":"macrocosmo/click","params":{"x":640,"y":360}}'

# Right-click to open context menu
curl -s -X POST localhost:15702 \
  -d '{"jsonrpc":"2.0","id":3,"method":"macrocosmo/click","params":{"x":640,"y":360,"button":"right"}}'
```

### Screenshot for visual regression

```bash
# Capture and decode
curl -s -X POST localhost:15702 \
  -d '{"jsonrpc":"2.0","id":1,"method":"macrocosmo/screenshot","params":{}}' > /dev/null
sleep 0.5
curl -s -X POST localhost:15702 \
  -d '{"jsonrpc":"2.0","id":2,"method":"macrocosmo/screenshot","params":{}}' \
  | jq -r .result.base64 | base64 -d > /tmp/screenshot.png
```

## Running the smoke test

A ready-made smoke test exercises all BRP methods end-to-end:

```bash
./scripts/tests/smoke_test.sh
```

The script starts the game, waits for BRP, runs assertions, and reports
pass/fail counts. Use `timeout` for CI:

```bash
timeout 120 ./scripts/tests/smoke_test.sh
```

## CI integration notes

- The game requires a display. On headless Linux CI, use `xvfb-run`:
  ```bash
  xvfb-run --auto-servernum timeout 120 ./scripts/tests/smoke_test.sh
  ```
- On macOS CI runners, a display is normally available.
- Screenshots can be saved as CI artifacts and compared with vision-based
  assertions (e.g., Claude Code image analysis).
- The smoke test exits with code 1 on any failure, which CI treats as a
  failed step.

## Source reference

All custom BRP handlers live in `macrocosmo/src/remote.rs`. The feature gate
and plugin registration happen in `macrocosmo/src/main.rs`.
