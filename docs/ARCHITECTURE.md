# Architecture

## Guiding principle

**cc-autobahn is NOT a token meter: it's an instrument cluster.**
Computing consumption, pricing, and billing windows is a problem already solved
by [`ccusage`](https://ccusage.com). We don't reimplement it or fork it
— we consume it as a data source. All the value of this project is in the
**visual layer** (Mercedes W203 skin) and the **per-response** `tok/s` calculation
(D8), which no existing tool offers.

```
┌──────────────────────────────────────────────────────────────┐
│                        cc-autobahn                           │
│                                                              │
│  ┌────────────┐   IPC (Tauri commands)   ┌────────────────┐  │
│  │  Frontend  │ <──────────────────────> │  Backend Rust  │  │
│  │  (webview) │                          │  (src-tauri)   │  │
│  │            │                          │                │  │
│  │  · amber   │                          │  · exec ccusage│  │
│  │    skin    │                          │  · tail JSONL  │  │
│  │  · needles/│                          │  · detect eng. │  │
│  │    bars    │                          │  · timers      │  │
│  └────────────┘                          └───────┬────────┘  │
└──────────────────────────────────────────────────┼──────────┘
                                                    │
                        ┌───────────────────────────┼───────────────┐
                        │                           │               │
                 ┌──────▼──────┐          ┌─────────▼────────┐  ┌───▼──────────┐
                 │   ccusage    │          │ ~/.claude/**.jsonl│  │ statusline   │
                 │  --json      │          │  (tail → tok/s)   │  │ JSON (rate_  │
                 │ (data engine)│          │                   │  │ limits)      │
                 └──────────────┘          └───────────────────┘  └──────────────┘
```

## Layers

### 1. Rust backend (`src-tauri/`)
Responsible for **all I/O**. Never blocks the UI.

- **Subprocess execution**: `std::process::Command` from Rust (D16). No
  `tauri-plugin-shell` — that plugin is for exec from the frontend JS; our I/O is
  trusted backend code. The engine runs on a dedicated `std::thread` (no async framework).
- **Engine detection** (`engine::detect`): walks the `$PATH` looking for `ccusage`
  global → `npx` → `bunx` → none. See [DATA-ENGINE.md](./DATA-ENGINE.md).
- **ccusage poll** (`engine::poll_once`): runs `ccusage blocks --active --json`
  every **15 s** (D13, 10–30 s window), parses with `serde_json`, emits `blocks-update`
  / `blocks-idle` / `engine-error` to the frontend.
- **JSONL tail** (`engine::burn`): follows the active session log, computes `tok/s`
  **per response** (`Δoutput / Δt_turn`) when each turn completes. This is the data
  ccusage doesn't provide — but it's **not instantaneous**: the JSONL only reports when the
  turn finishes (see D8/DATA-ENGINE §Source 2).
- **Statusline sensor** (`engine::sensor`): installs cc-autobahn as the
  `statusLine` command in `~/.claude/settings.json` (consent + backup + rollback, D12)
  and tails the socket where its binary dumps the official JSON (`rate_limits`, model,
  effort, cost).
- **History** (`engine::history`): `ccusage daily|monthly --json` on demand.
- **Window / tray**: icon in the macOS menu bar (`TrayIconBuilder`, no
  new plugin, D24), no Dock or Cmd+Tab (`ActivationPolicy::Accessory`). The
  icon itself **is not a static PNG**: it's a progress ring (% of the
  remaining 5h window) redrawn at runtime pixel by pixel by
  `tray_icon.rs`, updated from `engine::poll` and `sensor::tail` on each
  new data point (D30). Left click shows/hides the panel, anchored right
  below the icon (position computed from `TrayIconEvent::rect`); clicking
  outside hides it (hide-on-blur via `WindowEvent::Focused(false)`, with a
  300 ms anti-race guard, except when the PIN button is active, D26); right click
  opens a menu with "Quit". The window itself remains frameless, transparent
  (requires `macOSPrivateApi`, D14), `alwaysOnTop`, with native rounded
  corners via `CALayer` (D25). No longer draggable (supersedes D6). Config
  in `tauri.conf.json`; permissions in `capabilities/default.json` (trimmed to
  just `core:default` + `core:event:default` — window control happens
  100% in Rust, not via IPC).

### 2. Frontend (webview, `index.html` + `src/`)
**Presentation only**. No system I/O; receives data via IPC/events.

- `index.html`: cluster structure (display + PRND selector + PIN button,
  D26 + sensor consent overlay).
- `src/style.css`: amber VFD W203 skin (see [DESIGN.md](./DESIGN.md)).
- `src/main.js`: rendering — speedometer with physical spring (D18), segment
  bar/autonomy (estimated `EST` or official with priority, frozen on
  momentary disconnection, D23/D28), PRND selector (D7, no kickdown,
  D29), toggleable PACE/AUTO footer (D28, persisted in `localStorage`).

## Data flow

1. On startup, the backend detects the engine. If missing → `engine-missing`
   event (or the `engine_status` command, pull, for the first render without
   depending on winning the race against the event) → frontend shows the
   "CHECK ENGINE" overlay with an "Install engine" button (`engine::install_bun`:
   installs official Bun, updates the process `PATH`, and relaunches the engine
   without restarting the app, D9/Phase 4). It also offers to connect the
   statusline sensor (D12) if not installed.
2. Backend timer **every 10–30 s** (D13) → `ccusage blocks --active --json` → `blocks-update`
   event with average burn, projection, cost.
3. JSONL tail in parallel → when a turn completes, `burn-tick` event with `tok/s`
   **per response** → needle that jumps and decays (not instantaneous, D8).
4. Statusline sensor (push) → `sensor-update` event with `rate_limits.five_hour`
   (**official** autonomy), `seven_day` (border tint at 80%), `model.id`
   (PRND selector), cost. `effort.level` arrives in the payload but is no longer
   rendered (kickdown removed, D29).
5. Frontend renders: speedometer, segment bar, trip, model selector, PACE/AUTO
   footer. In parallel, the tray icon receives the same remaining-autonomy %
   and redraws its progress ring (D30) — this doesn't go through the frontend,
   it's computed directly in Rust at the point where each event is emitted.

## Why Tauri (not Electron)

- OS webview → ~5 MB binary vs ~150 MB for Electron.
- Native Rust backend for exec/tail with no overhead.
- `always-on-top` + frameless + transparent + native tray/menu-bar (D24).
- Real cross-OS support (macOS / Windows / Linux).

## Current status

**Phases 0–5 done** (see the actual checklist in [ROADMAP.md](./ROADMAP.md); only
the optional Phase 6 remains). The backend starts hidden behind the tray icon
(D24) and runs three sensors on dedicated threads: `engine` (ccusage `blocks
--active --json` every 15 s → cost/projection), `burn` (tail of the active JSONL
→ `tok/s` per response → `burn-tick`, D17, with a partial tick per intermediate
message and a 200 ms cadence, D27), and `sensor` (tail of the file the
statusline dumps to → **official** `rate_limits` data → `sensor-update`, D12). The
frontend renders the speedometer with a physical spring (D18), a segment bar
(estimated `blocks` marked "EST", or official `sensor` with priority and
frozen on momentary disconnection, D23/D28), the PRND selector (D7), and the
toggleable PACE/AUTO footer (D28). The same binary is the `statusLine` command
(dual mode, early-return, D19) with previous-statusLine chaining (D21) and
consent/backup/rollback auto-installation (D20/D22). The always-visible
floating window was replaced with a menu-bar icon with an on-demand panel
(D24, macOS only for now), with native rounded corners (D25) and a PIN
button to pin it (D26). That tray icon is now a progress ring redrawn at
runtime, not a static PNG (D30). Kickdown (the effort indicator) was
implemented and later removed for not adding visual value (D29).
