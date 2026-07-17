# cc-autobahn

> A **Mercedes W203 instrument cluster** for Claude Code's token consumption.
> It lives as a menu-bar icon on macOS: left click shows/hides a frameless,
> transparent, *always-on-top* panel anchored under the icon, with the amber
> dot-matrix VFD display: `tok/s` per response, remaining 5h window autonomy,
> cost, and active model.

cc-autobahn **is not a token meter: it's a visual skin**. Log parsing,
pricing, and billing windows are delegated to
[`ccusage`](https://ccusage.com) — run as a child process via its
`--json` output, with no forking or reimplementation. The only calculation
done in-house is `tok/s` **per response** (`Δoutput / Δt_turn` over the
JSONL tail), which no existing tool offers.

## Install

Requirements: **macOS** (universal: Apple Silicon + Intel) and
[Claude Code](https://claude.ai/code). Nothing else upfront — the cluster
offers to install its own engine (Bun + ccusage) on first run.

1. Download the `.dmg` from the
   [latest release](https://github.com/jmtrs/cc-autobahn/releases/latest) and
   drag `cc-autobahn.app` to `/Applications`.
2. The build is **unsigned** (no Apple Developer ID, D34), so Gatekeeper
   blocks the first launch. Either right-click the app → **Open** → **Open**,
   or run:
   ```sh
   xattr -dr com.apple.quarantine /Applications/cc-autobahn.app
   ```
3. Click the new menu-bar icon (no Dock, no Cmd+Tab) to show the cluster.
   With no engine detected, the **CHECK ENGINE** overlay has an *Install
   engine* button that wires everything up on its own (D9/D12).

## Status

**Phases 0–6 done** (see the [roadmap](./docs/ROADMAP.md) for the actual
checklist; only two optional/future items remain — Bun sidecar, Windows/
Linux). The backend runs three continuous sensors on dedicated threads,
plus one on-demand fetch:

- `engine` — detects ccusage (global → npx → bunx → an "Install engine"
  button that installs Bun on its own, D9) and polls `blocks --active --json`
  every 15 s → cost, projection, and estimated autonomy. `engine::history`
  fetches `ccusage claude daily --json` on demand (only when the History/
  Limits page opens, not on a timer, D33).
- `burn` — tails the JSONL of the active session → `tok/s` per response →
  `burn-tick` event. The speedometer jumps on turn completion and decays
  with a physical spring (D8/D18).
- `sensor` — the same binary auto-installs itself as Claude Code's
  `statusLine` command (consent + backup + rollback, D12) and tails the
  official JSON (`rate_limits.five_hour/seven_day`), which replaces the
  estimated projection as soon as it arrives.

With no engine detected, the panel shows the "CHECK ENGINE" overlay instead
of data. Tray icon (menu-bar, no Dock or Cmd+Tab) with a progress ring
redrawn at runtime. The panel itself is a 4-page MFD cycled by one button
next to PIN (D33), same UX as the W203's real trip-computer stalk button:
the live trip computer (speedometer, odometer, autonomy), a 30-day cost
History sparkline, the official weekly rate-Limits window + per-model cost
breakdown, and front-end Settings (which pages are in the cycle, and which
opens by default). `cargo test` 32/32 (includes verification against real
JSONL, statusline, and multi-model ccusage data), `cargo clippy` clean.

## Design (car → tokens mapping)

| W203 Element               | Claude Code Metric                           |
| --------------------------- | --------------------------------------------- |
| Speedometer (Km/h)          | `tok/s` per response (`Δoutput / Δt_turn`)   |
| Fuel consumption (L/100 Km) | Average cost `$/Mtok`                         |
| Range / fuel tank ⛽        | Remaining 5h window (segment bar)             |
| Trip "AFTER START"          | Tokens/time since last reset                  |
| Odometer                    | Total accumulated tokens                      |
| PRND selector               | Active model (O/S/H/F) lit up + effort        |
| Clock                       | Real time                                     |
| Trip-computer stalk button  | MFD page cycle: trip / history / limits / settings |

## Philosophy

- **Don't reinvent the engine.** Data comes from
  [`ccusage`](https://ccusage.com) (the de facto standard), as a child process.
- **cc-autobahn = instrument cluster.** We provide the visual layer (W203
  skin) and the `tok/s` per response calculation.
- **Zero friction.** The app wires itself up (engine + statusline sensor)
  with a single consent prompt (D9/D12).
- **Honest precision.** Cost under a subscription is *estimated*; the
  autonomy (`rate_limits`) is *official* data; actual billing is the Claude
  Console (D11).

## Sources and cadences

| Sensor | Cadence | What it provides |
| ------ | -------- | ------ |
| `ccusage blocks --active --json` | 10–30 s | average burn, projection, cost |
| Tail of `~/.claude/projects/**/*.jsonl` | per turn (event) | `tok/s` per response |
| Statusline JSON (auto-installed sensor) | push | official `rate_limits.five_hour`/`seven_day` |

> **It's not instantaneous.** The JSONL only stamps `usage` when the turn
> closes (empirically validated, D8): the needle jumps on completion and
> decays, it doesn't react mid-generation.

## Development

Requirements: [Node.js](https://nodejs.org/), [Rust](https://rustup.rs/), and
the [Tauri v2 dependencies](https://v2.tauri.app/start/prerequisites/).

```bash
npm install          # Vite + Tauri CLI
npm run tauri dev    # builds Rust and opens the cluster (dev, port 1420)
npm run tauri build  # release binary
```

Backend tests (Rust):

```bash
cd src-tauri && cargo test
```

Regenerate icons from another logo:

```bash
node scripts/make-icon.mjs
npx @tauri-apps/cli icon scripts/source-icon.png
```

## Structure

```
cc-autobahn/
├── index.html            # cluster shell (display, PRND selector, overlays)
├── src/
│   ├── style.css         # amber VFD W203 skin
│   ├── main.js           # thin entrypoint: wires all modules on DOMContentLoaded
│   └── modules/
│       ├── format.js         # VFD number formatters (tok/s, tokens, h:min, $, model codes)
│       ├── telemetry-state.js # shared state (lastBlock/sensor/pace buffers)
│       ├── clock.js          # trip-computer clock tick
│       ├── speedometer.js    # tok/s spring animation + burn-tick handler
│       ├── trip-computer.js  # Page 0: segments, gear, odo/avg, blocks/sensor handlers
│       ├── footer-metric.js  # PACE/AUTO footer toggle + computation
│       ├── engine-overlay.js # CHECK ENGINE overlay + install_bun button
│       ├── sensor-consent.js # sensor connect/disconnect consent UI
│       ├── pin-button.js     # PIN button (pins panel open)
│       ├── ipc-events.js     # wires backend events to the modules above
│       ├── header-hint.js    # docked "what's under the cursor" line (replaces title= tooltips)
│       ├── mfd-nav.js        # MFD page-cycle button + state (D33)
│       ├── mfd-settings.js   # localStorage: default page, which pages are in the cycle
│       ├── history-data.js   # shared on-demand fetch (history_daily), used by Page 1 + 2
│       ├── history-page.js   # Page 1: 30-day cost sparkline + per-day model breakdown
│       ├── limits-page.js    # Page 2: weekly rate-limit window, cost/model, burn rate
│       └── settings-page.js  # Page 3: default page + page-cycle toggles, custom dropdown
├── scripts/
│   └── make-icon.mjs      # amber icon generator (zero-dep PNG)
├── src-tauri/
│   ├── Cargo.toml
│   ├── tauri.conf.json    # frameless, always-on-top, transparent window
│   ├── capabilities/      # v2 permissions (core:default + core:event:default)
│   ├── icons/             # app icons + tray-icon-template.png
│   └── src/
│       ├── main.rs        # dual entrypoint (GUI / statusline mode) + Tauri bootstrap
│       ├── window.rs      # PinnedState, hide-on-blur, panel positioning
│       ├── tray.rs        # menu-bar menu + icon + click-to-toggle
│       ├── engine/        # ccusage sensor: detect (mod.rs) + install_bun (install.rs) + poll (blocks.rs) + on-demand daily history (history.rs)
│       ├── burn/          # tok/s sensor: zulu parsing + turn calc (parser.rs) + JSONL tail (tail.rs)
│       ├── sensor/         # official statusline sensor: mod.rs (tail) + statusline_bin.rs (CLI mode) + install.rs (settings.json)
│       └── tray_icon.rs   # tray icon progress ring
├── docs/                  # architecture, design, decisions (ADR), roadmap
├── vite.config.js
└── package.json
```

## Documentation

- [docs/ARCHITECTURE.md](./docs/ARCHITECTURE.md) — layers, data flow, why Tauri.
- [docs/DESIGN.md](./docs/DESIGN.md) — W203 visual language, palette.
- [docs/DATA-ENGINE.md](./docs/DATA-ENGINE.md) — ccusage, statusline, OTEL, comparison.
- [docs/DECISIONS.md](./docs/DECISIONS.md) — decision log (ADR) and rationale.
- [docs/ROADMAP.md](./docs/ROADMAP.md) — implementation phases.

## Roadmap

Phases 0–6 done (chassis, data engine, `tok/s` per response, official
statusline sensor, zero friction, tray/menu-bar, polish, MFD history/limits/
settings pages). The real, up-to-date checklist lives in
[docs/ROADMAP.md](./docs/ROADMAP.md) — don't duplicate it here, it gets out
of sync. Only two optional/future items remain: packaging Bun as a Tauri
sidecar, and Windows/Linux support (tray API is cross-platform except
`set_activation_policy`, untested outside macOS so far).

## License

[MIT](./LICENSE).
