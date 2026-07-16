# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## What it is

cc-autobahn is an **instrument cluster** (Tauri v2) styled after the amber VFD display of the Mercedes W203, showing Claude Code token consumption. It lives as an icon in the macOS menu bar (D24): left click shows/hides a frameless, transparent, always-on-top panel anchored under the icon; no Dock, no Cmd+Tab. It is no longer a draggable floating window (D6, superseded).

**Guiding principle: it's not a token meter, it's a visual skin.** Log parsing, pricing, and billing windows are delegated to [`ccusage`](https://ccusage.com), run as a child process via its `--json` output. It is not forked or reimplemented (see `docs/DECISIONS.md` D1–D3). The only calculation done in-house is `tok/s` **per response** (`Δoutput / Δt_turn` over the JSONL tail), which ccusage doesn't offer. **It is not instantaneous**: the JSONL only stamps `usage` when the turn ends (empirically validated, D8) — the needle jumps on completion and decays, it does not react mid-generation.

**Current state: Phases 0–5 done** (real checklist in `docs/ROADMAP.md`, the rationale for each piece in `docs/DECISIONS.md` D1–D30). Only Phase 6 remains (history, optional) plus two optional/future items (Bun sidecar, Windows/Linux). The three sensors (`engine`, `burn`, `sensor`) run on dedicated threads and are wired into the display: speedometer (physical spring, D18), segment bar (estimated `EST` or official data with priority, D23), PRND selector (D7), PACE/AUTO footer (D28), PIN button (D26). With no engine detected, the frontend paints the "CHECK ENGINE" overlay with a button that installs Bun on its own (`engine::install_bun`, D9) and relaunches the engine without restarting the app. The dual binary is also Claude Code's `statusLine` command (auto-installable, D19–D22). The tray icon (D24) is a progress ring redrawn at runtime, not a static PNG (D30). `cargo test` 26/26, `cargo clippy` clean.

## Commands

```bash
npm install          # Vite + Tauri CLI
npm run tauri dev    # builds Rust and opens the cluster (dev)
npm run tauri build  # release binary
npm run dev          # frontend only, Vite (port 1420, strictPort)
```

Regenerate icons:

```bash
node scripts/make-icon.mjs                        # zero-dep amber icon
npx @tauri-apps/cli icon scripts/source-icon.png  # derives all sizes
```

Backend: `cargo test` (26 tests, in `src-tauri/`) + `cargo clippy` clean. Frontend: no tests or linter configured.

## Architecture (two layers)

- **Rust backend (`src-tauri/`)** — responsible for **all I/O**, never blocks the UI. Each sensor is a directory module, split by concern: `engine/` (`mod.rs`: `detect` global ccusage → npx → bunx + poll loop; `blocks.rs`: `poll_once`/`ccusage blocks --active --json` at **slow cadence 15 s**, D13; `install.rs`: `install_bun`), `burn/` (`zulu.rs`: Zulu timestamp parsing; `parser.rs`: `TurnState`/`process_line`, pure turn-calc logic; `tail.rs`: JSONL file tail → `tok/s` per response, D17/D27), `sensor/` (`mod.rs`: shared paths + tail watcher; `statusline_bin.rs`: the `statusline` CLI entrypoint; `install.rs`: settings.json auto-install/uninstall), `tray_icon.rs` (tray icon progress ring, D30), `window.rs` (PIN state, hide-on-blur, panel positioning), `tray.rs` (menu-bar menu/icon/click). `engine::history` (`ccusage daily|monthly`) is still unimplemented (Phase 6).
- **Frontend (webview, `index.html` + `src/`)** — presentation only, no system I/O; receives data via Tauri IPC/events. `src/style.css` = amber skin; `src/main.js` is a thin entrypoint that wires the widget modules under `src/modules/` (clock, speedometer, trip-computer, footer-metric, engine-overlay, sensor-consent, pin-button, ipc-events, plus shared `format.js`/`telemetry-state.js`).

**Three sensors, three cadences (D13):** ccusage = slow poll (cost/projection); JSONL tail = event per turn (`tok/s`); statusline = push (official `rate_limits` data).

**Statusline sensor (D12) — how the official data arrives:** the statusline JSON is *push* (Claude Code passes it via stdin only to a configured script); an external window doesn't receive it passively. cc-autobahn **is** that script and self-installs: it writes `statusLine` into `~/.claude/settings.json` (consent + backup + rollback) pointing at its own binary, which emits the normal line to stdout **and** dumps the JSON to a socket that the window tails. It is the only source of `rate_limits.five_hour/seven_day` (**official** range).

Target flow: the backend emits events (`blocks-update`, `burn-tick`, `sensor-update`, `engine-missing`) that the frontend listens to and renders. Details in `docs/ARCHITECTURE.md`.

## Car → tokens mapping (domain language)

| W203 Element          | Claude Code Metric                       |
| -------------------- | ---------------------------------------- |
| Speedometer          | `tok/s` per response (in-house calc)     |
| Consumption (L/100 Km) | Average cost `$/Mtok`                  |
| Range / fuel tank     | 5 h window remaining (segment bar)      |
| "AFTER START" trip    | Tokens/time since last reset             |
| Odometer              | Total accumulated tokens                 |
| PRND selector         | Active model (O/S/H/F) lit up            |

## Conventions

- **Window config in `tauri.conf.json`; permissions in `capabilities/default.json`** (v2). The window has `label: "cluster"`, starts hidden (`visible:false`) — the capabilities are tied to that label and are trimmed down to `core:default`/`core:event:default` (all tray/window control happens in pure Rust, D24, never via IPC from JS). `app.macOSPrivateApi: true` is required for transparency on macOS (D14) — do not remove it.
- **Tray/menu-bar (D24)**: show/hide/position live in `src-tauri/src/window.rs`; the menu/icon/"Quit"/click-to-toggle live in `src-tauri/src/tray.rs` (`TrayIconBuilder`, `tray-icon` feature of the `tauri` crate itself, no new plugin); `main.rs` just wires the two together in `.setup()`. The **icon itself** is a progress ring redrawn at runtime by `tray_icon.rs` (D30), called from `engine/`/`sensor/` on every new data point — not a static PNG (that PNG, `icons/tray-icon-template.png`, only remains as the initial icon before the first redraw). **Always use `TrayIcon::set_icon_with_as_template()`, never plain `set_icon()`** — `set_icon()` doesn't preserve macOS's "template" flag across calls and the icon gets repainted as fixed black instead of adapting to light/dark mode (real bug, D30). `ActivationPolicy::Accessory` only on macOS (`#[cfg(target_os = "macos")]`); the rest of the tray API is cross-platform. Only macOS tested so far.
- **Exec from Rust with `std::process::Command`, NOT `tauri-plugin-shell`** (D16). The plugin is for exec from the frontend JS; our I/O is trusted backend code. The engine runs on a dedicated `std::thread` (no async framework). Zero new deps.
- **`macos-private-api` (cargo feature) is coupled to `macOSPrivateApi` (conf)**: if you touch one, touch the other. Tauri's build script fails if they don't match.
- **CSP already applied (D15)**: the restrictive `security.csp` policy in `tauri.conf.json` has been active since the first IPC command landed — verified against `sensor_status`/`install_sensor`/`set_pinned` in `tauri dev`. Do not revert to `null`.
- **Fixed dev port 1420** (`vite.config.js` + `devUrl`); `clearScreen: false` to avoid losing Rust logs.
- **Dependencies pinned to latest stable** by the user's decision (D10): do not downgrade Vite/Tauri/serde without cause.
- **Honest precision** (D11): cost under subscription is **estimated**; the `rate_limits` window is **official** data. Do not present estimates as real billing.
- **Documentation and comments in English**; the ADRs in `docs/DECISIONS.md` record the rationale behind each decision — consult them before changing architecture, the data engine, or the aesthetics.
