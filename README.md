# CC Autobahn

> A **Mercedes W203 instrument cluster** for Claude Code and Codex usage.
> It lives as a menu-bar icon on macOS: left click shows/hides a frameless,
> transparent, *always-on-top* panel anchored under the icon by default, with the amber
> dot-matrix VFD display: `tok/s` per response, remaining 5h window autonomy,
> cost, and active model.

<p align="center">
  <img src="docs/screenshots/demo.gif" alt="cc-autobahn demo — dragging the panel, MFD pages, live gauges" width="100%" />
</p>
<p align="center">
  <img src="docs/screenshots/hero.png" alt="cc-autobahn — trip computer, history, and limits pages" width="100%" />
</p>
<p align="center">
  <img src="docs/screenshots/dynamic-range.png" alt="PACE spike vs. AUTO estimate, same 5h window" width="49%" />
  <img src="docs/screenshots/history-limits.png" alt="30-day cost history and weekly rate-limit window" width="49%" />
</p>

## Install

```sh
brew install --cask jmtrs/tap/cc-autobahn
```

That's the whole install. `jmtrs/tap/cc-autobahn` is a **cask** (it ships a
GUI `.app`, not a CLI formula) published on a personal tap
(`jmtrs/homebrew-tap`), not homebrew-core — the `user/tap/formula` shorthand
above resolves it automatically, no separate `brew tap` step needed. Every
release keeps the cask's download URL and checksum in sync with the GitHub
Release, so this one command always tracks the latest version.

**Prefer not to use Homebrew?** Download the `.dmg` from the
[latest release](https://github.com/jmtrs/cc-autobahn/releases/latest) and
drag `cc-autobahn.app` to `/Applications` instead.

Either way, requirements stop at **macOS** (universal build: Apple Silicon +
Intel) and Claude Code and/or Codex — nothing else upfront. The cluster offers
to install its own engine (Bun + ccusage) on first run.

The build is **unsigned** (no Apple Developer ID), so Gatekeeper blocks the
first launch either way. Either right-click the app → **Open** → **Open**,
or run:

```sh
xattr -dr com.apple.quarantine /Applications/cc-autobahn.app
```

First run: click the new menu-bar icon (no Dock, no Cmd+Tab) to show the
cluster. With no engine detected, the **CHECK ENGINE** overlay has an
*Install engine* button that wires everything up on its own.

## What it is

cc-autobahn **does not reimplement a token meter: it's an instrument-cluster
shell around trusted data sources**. All the usage math
— log parsing, pricing, billing windows — is delegated to
[**ccusage**](https://ccusage.com) by [**@ryoppippi**](https://github.com/ryoppippi),
run as a child process via its `--json` output. It is not forked or
reimplemented: ccusage does the hard, error-prone part (parsing JSONL,
pricing, deduplicating the shared 5h block, the Opus multiplier) and does it
well. The usage-specific calculation kept in-house is `tok/s` **per
response**, which ccusage doesn't offer. The app also owns native window/tray
behavior and optional provider-native permission bridges.

## Features

- **Live speedometer** — `tok/s` per response, with a physical spring on the
  needle: it jumps on completion and decays, rather than faking real-time
  motion the data can't actually support.
- **4-page MFD**, cycled by one button next to PIN — same UX as the W203's
  real trip-computer stalk: trip computer, 30-day cost history, the official
  weekly rate-limit window + per-model cost breakdown, and settings.
- **Provider-honest numbers** — Claude's opt-in `statusLine` and Codex's
  capability-probed App Server provide official limits where available;
  estimates, stale values and unavailable sources remain visibly distinct.
- **Tray icon as a live gauge** — one native-size ring shows the most urgent
  valid Claude/Codex quota (lowest remaining percentage, never an average);
  critical usage and pending permission requests switch it into an alert state.
- **Permission decisions in the cluster** — opt-in Claude Code and Codex
  `PermissionRequest` hooks share a provider-namespaced queue with Approve,
  Deny, and supported Always Allow actions. Both fail open to their native
  approval UI when the GUI is unavailable; Codex hook trust remains native.
- **Anchored or movable** — opens under the menu-bar icon by default; drag
  the header or model selector to save a manual position, then reset it from
  Settings or the tray menu.
- **Configurable VFD** — built-in/custom themes, page order/default page,
  permission sound, hook consent, and position reset live on the shared
  Settings page.
- **Zero setup** — no ccusage or Bun on the machine? one button installs
  both and starts polling, no terminal required.

Current verified baseline: `cargo test` **143/143**, `npm run test:frontend`
**59/59**, 45 pixel-compared Playwright baselines across Claude, Codex and
Both in amber, emerald and magenta at **550×150 / 550×290**, `cargo fmt --check`,
`cargo clippy --all-targets --all-features -- -D warnings`, and
`npm run build` all pass.

## Design (car → tokens mapping)

| W203 Element               | Claude/Codex metric                          |
| --------------------------- | --------------------------------------------- |
| Speedometer (Km/h)          | `tok/s` per response (`Δoutput / Δt_turn`)   |
| Fuel consumption (L/100 Km) | Average cost `$/Mtok`                         |
| Range / fuel tank ⛽        | Remaining 5h window (segment bar)             |
| Trip "AFTER START"          | Tokens/time since last reset                  |
| Odometer                    | Total accumulated tokens                      |
| PRND selector               | Provider-native active-model family          |
| Clock                       | Real time                                     |
| Trip-computer stalk button  | MFD page cycle: trip / history / limits / settings |

## Philosophy

- **Zero friction.** Engine setup is one click when needed; statusline and
  permission-hook changes are separate, explicit consent flows with rollback.
- **Honest precision.** Cost under a subscription is *estimated*; the
  autonomy (`rate_limits`) is *official* data; actual billing is always the
  provider's own account/console.

## Sources and cadences

| Sensor | Cadence | What it provides |
| ------ | -------- | ------ |
| `ccusage blocks --active --json` | 10–30 s | average burn, projection, cost |
| Tail of `~/.claude/projects/**/*.jsonl` | 200 ms file follow + 5 s discovery | partial/final `tok/s` ticks per response |
| Statusline JSON (auto-installed sensor) | push | official `rate_limits.five_hour`/`seven_day` |
| `ccusage claude daily --json` | on demand | History/Limits totals and per-model cost |
| Codex rollout files under `CODEX_HOME` | 200 ms file follow + 5 s discovery | per-response rate, model and thread identity |
| `codex app-server --stdio` | capability-probed poll + notifications | official Codex limits and account usage |
| `ccusage codex daily/session --json` | on demand | estimated Codex history and session totals |
| Claude/Codex `PermissionRequest` hooks | event-driven request/response | provider-namespaced tool approvals over a Unix socket |

> **It's not token-stream telemetry.** A provider tail can emit a partial tick
> from an intermediate usage record and a final tick when the turn closes.
> Between JSONL writes the needle decays; it cannot
> react to individual generated tokens.

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
cargo test --manifest-path src-tauri/Cargo.toml
cargo fmt --manifest-path src-tauri/Cargo.toml --check
cargo clippy --manifest-path src-tauri/Cargo.toml --all-targets --all-features -- -D warnings
npm run build
npm run test:frontend
npm run test:visual
```

Regenerate icons from another logo:

```bash
node scripts/make-icon.mjs
npx @tauri-apps/cli icon scripts/source-icon.png
```

## Structure

```
cc-autobahn/
├── index.html          # cluster shell (display, PRND selector, overlays)
├── src/
│   ├── style.css       # amber VFD W203 skin
│   ├── main.js         # thin entrypoint, wires the widgets below
│   └── modules/        # one widget/concern per file (speedometer, MFD pages, overlays, IPC wiring...)
├── src-tauri/
│   └── src/
│       ├── main.rs     # three modes: GUI / statusline / permission-hook
│       ├── engine/     # ccusage detection, polling, Bun auto-install
│       ├── burn/       # JSONL tail → tok/s per response
│       ├── sensor/     # statusline auto-install + official rate_limits tail
│       ├── permission/ # PermissionRequest install + Unix-socket approval queue
│       ├── pathfix.rs, tray.rs, tray_icon.rs, window.rs
│       └── ...
├── docs/                # architecture, design, decisions (ADR), roadmap — see below
└── scripts/make-icon.mjs
```

The full per-file breakdown lives in [docs/ARCHITECTURE.md](./docs/ARCHITECTURE.md).

## Documentation

- [docs/ARCHITECTURE.md](./docs/ARCHITECTURE.md) — layers, data flow, why Tauri.
- [docs/DESIGN.md](./docs/DESIGN.md) — W203 visual language, palette.
- [docs/DATA-ENGINE.md](./docs/DATA-ENGINE.md) — ccusage, statusline, OTEL, comparison.
- [docs/DECISIONS.md](./docs/DECISIONS.md) — decision log (ADR) and rationale.
- [docs/ROADMAP.md](./docs/ROADMAP.md) — implementation phases.
- [docs/CODEX-INTEGRATION-ASSESSMENT.md](./docs/CODEX-INTEGRATION-ASSESSMENT.md) — verified Claude/Codex plan; automated phases 0–6 are implemented, with the trusted cross-surface soak tracked separately.

## Roadmap

Phases 0–7 done (chassis, data engine, `tok/s` per response, official
statusline sensor, zero friction, tray/menu-bar, polish, MFD history/limits/
settings pages, redline feedback, movable positioning, and the permission
gate). The real, up-to-date checklist lives in
[docs/ROADMAP.md](./docs/ROADMAP.md) — don't duplicate it here, it gets out
of sync. Future work includes packaging Bun as a Tauri sidecar,
Windows/Linux validation, and the trusted cross-surface release soak.

## Credits

cc-autobahn exists because [**ccusage**](https://github.com/ryoppippi/ccusage)
by [**@ryoppippi**](https://github.com/ryoppippi) already solved the hard
problem — parsing Claude Code's JSONL logs, pricing, deduplicating billing
blocks — correctly and reliably. This project leaves that accounting intact,
then adds per-response speed, native tray/window behavior, permission routing,
and the Mercedes instrument-cluster presentation. If you find this useful, go
star [ccusage](https://github.com/ryoppippi/ccusage) too; it remains the usage
engine underneath.

## License

[MIT](./LICENSE).
