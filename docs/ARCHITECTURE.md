# Architecture

## Guiding principle

**cc-autobahn does not reimplement a token meter: it is an instrument cluster.**
Computing consumption, pricing, and billing windows is a problem already solved
by [`ccusage`](https://ccusage.com). We don't reimplement it or fork it
— we consume it as a data source. The project owns the Mercedes W203 visual
layer, the **per-response** `tok/s` calculation (D8), native tray/window
behavior, and an opt-in permission-decision bridge.

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

The diagram above shows the Claude data path only. Codex mirrors it with its
own adapters instead of ccusage/JSONL/statusline: a rollout JSONL tail under
`CODEX_HOME` (`providers/codex/rollout.rs`, D46) and an owned
`codex app-server --stdio` child for official account/rate-limit data
(`providers/codex/app_server.rs`, D48). Both providers feed the same
normalized contracts in `providers/mod.rs` (D45) before reaching the frontend.

The permission path is intentionally separate from telemetry: Claude Code or
Codex starts `cc-autobahn permission-hook <provider>`, that short-lived process
connects to `~/.cc-autobahn/permission.sock`, and the GUI's Rust listener
queues the provider-tagged request until the user approves or denies it.

## Layers

### 1. Rust backend (`src-tauri/`)
Responsible for **all I/O**. Never blocks the UI.

- **Subprocess execution**: `std::process::Command` from Rust (D16). No
  `tauri-plugin-shell` — that plugin is for exec from the frontend JS; our I/O is
  trusted backend code. The engine runs on a dedicated `std::thread` (no async framework).
- **PATH hardening** (`pathfix::apply`, D36): runs once at GUI startup (not
  statusline mode), prepends `/opt/homebrew/bin`, `/usr/local/bin`,
  `~/.bun/bin`, `~/.local/bin` to the process `PATH` when they exist on disk.
  A Finder/Dock launch inherits launchd's bare `PATH`, which hides an
  already-installed engine that `npm run tauri dev` never surfaces (it
  inherits the terminal's `PATH` instead).
- **Engine detection** (`engine::detect`): walks the `$PATH` looking for `ccusage`
  global → `npx` → `bunx` → none. See [DATA-ENGINE.md](./DATA-ENGINE.md).
- **ccusage poll** (`engine::blocks::poll_once`, called from `engine::start`): runs
  `ccusage blocks --active --json` every **15 s** (D13, 10–30 s window), parses with
  `serde_json`, emits `blocks-update` / `blocks-idle` / `app-engine-error` to the frontend.
  `engine::install` holds the Bun auto-installer, which runs the actual
  `curl | bash` install on its own `std::thread` and reports progress/outcome
  via `install-progress`/`install-succeeded`/`install-failed` events — the
  `#[tauri::command]` handler itself must return fast, since a plain sync
  command runs on the thread that also pumps the webview's event loop (D36).
- **JSONL tail** (`burn::tail::TailSet`, driven by `burn::start`): discovers
  fresh session logs and follows them concurrently. `burn::parser` emits a
  partial `tok/s` tick on eligible intermediate assistant/tool-use writes and
  a final tick at turn closure. This is not token-stream telemetry: no new
  value exists between JSONL writes (D8/D27).
- **Statusline sensor** (`sensor::install`): installs cc-autobahn as the
  `statusLine` command in `~/.claude/settings.json` (consent + backup + rollback, D12).
  `sensor::statusline_bin` is the CLI entrypoint invoked as that command (reads stdin,
  chains the previous line, dumps the sensor file); `sensor::start` (in `sensor/mod.rs`)
  tails that file and emits the official JSON (`rate_limits`, model, effort, cost).
  `sensor::install::refresh_if_stale` runs on a background thread at every GUI
  startup and silently re-copies the binary in place (byte-for-byte diff
  against the running one) if already installed — the consent flow only ever
  runs once, so without this a copy from an old release would keep pointing
  `statusLine` at dead code forever (D36).
- **History** (`engine::history::history_daily`): `ccusage claude daily --json`
  (scoped to `claude` — the bare `ccusage daily` mixes in every agent ccusage
  detects on the machine), fetched **on demand** (D33's 4th cadence class,
  alongside D13's three) — only when the History/Limits MFD page opens, not
  on a timer, cached client-side (`history-data.js`) for a few minutes.
- **Permission hooks** (`permission/`, D42/Phase 5): `install.rs` and
  `codex_install.rs` independently merge/unmerge Claude `settings.json` and
  Codex user `hooks.json`; `hook_bin.rs` emits each provider's native decision;
  `mod.rs` owns the Unix listener and provider-namespaced FIFO queue;
  `always_allow.rs` owns Claude compatibility persistence plus exact
  provider/session memory. Codex trust inventory comes from the existing App
  Server's stable `hooks/list`. Hook processes block, never the UI thread, and
  fail open to each provider's native approval UI when the GUI is unavailable.
- **Window / tray**: split by concern — `window.rs` owns the native macOS
  `NSPanel` conversion/fullscreen-Space behavior, PIN state, hide-on-blur, and
  positioning under the icon; `tray.rs` owns the menu-bar icon
  (`TrayIconBuilder`, no new plugin, D24) + menu + click handler; `main.rs` just
  wires the two together in `.setup()`. No Dock or Cmd+Tab
  (`ActivationPolicy::Accessory`). The icon itself **is not a static PNG**: it's
  a progress ring (% of the remaining 5h window) redrawn at runtime pixel by
  pixel by `tray_icon.rs`, updated from `engine::start` and `sensor::start` on
  each new data point (D30). Left click shows/hides the panel, anchored right
  below the icon (position computed from `TrayIconEvent::rect` via
  `window::position_under_tray`); clicking outside hides it (hide-on-blur via
  `WindowEvent::Focused(false)` in `window::wire`, with a 300 ms anti-race guard
  in `tray.rs`, except when the PIN button is active, D26); right click
  opens a menu with "Reset position" and "Quit cc-autobahn". The window itself remains frameless, transparent
  (requires `macOSPrivateApi`, D14), `alwaysOnTop`, with native rounded
  corners via `CALayer` (D25). D41 restored dragging from the header and
  model-selector zones; `window-position.json` stores a manual override,
  clamps it after monitor changes, and Reset position returns to tray anchoring.
  Config lives in `tauri.conf.json`; `capabilities/default.json` includes
  `core:window:allow-start-dragging` for this narrow frontend bridge.

### 2. Frontend (webview, `index.html` + `src/`)
Presentation plus narrowly scoped native window commands; receives data via
IPC/events.

- `index.html`: cluster structure — header (dynamic nameplate, header-hint,
  PIN/MFD buttons) + a 4-page MFD (`.pages`, D33) + PRND selector +
  sensor/engine/permission consent and decision overlays.
- `src/style.css`: amber VFD W203 skin (see [DESIGN.md](./DESIGN.md)).
- `src/main.js`: thin entrypoint, wires the widget modules under `src/modules/`
  on `DOMContentLoaded`. `speedometer.js` — physical spring (D18); `trip-computer.js`
  — Page 0: segment bar/autonomy (estimated `EST` or official with priority, frozen on
  momentary disconnection, D23/D28) + PRND selector (D7, no kickdown, D29) +
  header-hint wiring for its own static glyphs; `footer-metric.js` — toggleable
  PACE/AUTO footer (D28, persisted in `localStorage`); `telemetry-state.js` holds
  the state shared between the two (`lastBlock`, sensor connection, PACE/AUTO
  buffers, `sevenDayPct`) to avoid a circular import. MFD pages (D33):
  `mfd-nav.js` (page-cycle button + state), `app-settings.js` (versioned schema-v2
  boundary and legacy migration), `mfd-settings.js` (schema-backed settings:
  default page, which pages are in the cycle), `history-data.js` (shared
  on-demand fetch, used by both Page 1 and Page 2), `history-page.js` (Page 1:
  cost sparkline), `limits-page.js` (Page 2: weekly window, cost/model, burn
  rate), `settings-page.js` (Page 3, incl. the custom dropdown replacing a
  native `<select>`). Additional modules own the D37 redline/tray alert,
  D41 window drag/reset, D42 permission gate/consent/sound, themes, and the
  synthetic VFD cursor. `header-hint.js` is the shared "what's under the
  cursor" line, replacing every native `title=` tooltip.

## Data flow

1. On startup, the backend detects the engine. If missing → `app-engine-missing`
   event (or the `engine_status` command, pull, for the first render without
   depending on winning the race against the event) → frontend shows the
   "CHECK ENGINE" overlay with an "Install engine" button (`engine::install::install_bun`:
   installs official Bun, updates the process `PATH`, and relaunches the engine
   without restarting the app, D9/Phase 4). It also offers to connect the
   statusline sensor (D12) if not installed.
2. Backend timer **every 10–30 s** (D13) → `ccusage blocks --active --json` → `blocks-update`
   event with average burn, projection, cost.
3. Provider-owned JSONL tails run in parallel. Claude eligible intermediate
   writes/final closures and Codex rollout `token_count` responses emit a
   discriminated `burn-tick` with `tok/s` **per response** → independent
   needles that jump and decay (not token-stream telemetry, D8/D27/D46).
   Codex `session_meta` + `turn_context` also emit thread/model activity.
4. Statusline sensor (push) → `sensor-update` event with `rate_limits.five_hour`
   (**official** autonomy), `seven_day` (border tint at 80%), `model.id`
   (PRND selector), cost. `effort.level` arrives in the payload but is no longer
   rendered (kickdown removed, D29).
5. Frontend renders: speedometer, segment bar, trip, model selector, PACE/AUTO
   footer. In parallel, the tray icon receives the same remaining-autonomy %
   and redraws its progress ring (D30) — this doesn't go through the frontend,
   it's computed directly in Rust at the point where each event is emitted.
6. **On demand only** (D33/D47): when the user cycles the MFD to History or
   Limits, the frontend calls provider-discriminated `history_daily`; native
   code runs `ccusage claude daily` or `ccusage codex daily --speed auto` and
   caches results/in-flight work separately for a few minutes. A normalized
   `history_sessions` command exists for both providers without exposing local
   project or rollout paths.
7. **Permission requests** (D42/Phase 5) use an independent synchronous route:
   Claude or Codex hook process → Unix socket → provider-namespaced FIFO queue
   → `permission-pending` → Approve/Deny/Always Allow command → provider-native
   socket response. A pending request auto-opens the panel, alerts the tray,
   and can play the configured sound.

## Why Tauri (not Electron)

- OS webview → ~5 MB binary vs ~150 MB for Electron.
- Native Rust backend for exec/tail with no overhead.
- `always-on-top` + frameless + transparent + native tray/menu-bar (D24).
- Cross-platform foundation. Current release/support is macOS; native panel
  behavior is macOS-specific, the permission transport is Unix-only, and
  Windows/Linux remain unvalidated.

## Current status

**Phases 0–7 done** (see [ROADMAP.md](./ROADMAP.md)). One executable dispatches
to three modes: GUI, `statusline`, or `permission-hook`. GUI mode starts hidden
behind the tray, runs the Claude engine/transcript/status sensors plus the Codex
rollout and official App Server account sensors and on-demand daily
history, and hosts the event-driven permission listener. The four-page MFD
contains Trip, History, Limits, and shared Settings; redline feedback, dynamic
tray states, themes, permission sound/consent, and manual position reset are
wired. Default placement remains under the tray, with D41's persisted drag
override available when wanted.

Current verified baseline: **140 Rust tests**, **59 frontend tests**, **45 visual
baselines**, Rustfmt check, strict Clippy (`-D warnings`), and the Vite
production build all pass.
Frontend linting is not yet configured. Future work is tracked in the roadmap:
Codex provider foundation and the complete dual-provider chassis are implemented;
local rollout speed/model/thread telemetry and local estimated history are
implemented; official App Server account data, provider-native permission
hooks, model presentation and conservative dual-provider tray summary are
implemented. Bun sidecar, Windows/Linux validation, and trusted cross-surface
release soak remain.
