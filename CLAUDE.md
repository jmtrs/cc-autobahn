# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## What it is

cc-autobahn is an **instrument cluster** (Tauri v2) styled after the amber VFD display of the Mercedes W203, showing Claude Code token consumption. It lives as an icon in the macOS menu bar (D24): left click shows/hides a frameless, transparent, always-on-top panel; no Dock, no Cmd+Tab. It opens anchored under the icon by default, but D41 restored drag-to-move from the header/model-selector zones with a persisted manual override. Settings and the tray menu can reset it to the tray anchor.

**Guiding principle: don't reimplement usage accounting.** Log parsing, pricing, and billing windows are delegated to [`ccusage`](https://ccusage.com), run as a child process via its `--json` output. It is not forked or reimplemented (see `docs/DECISIONS.md` D1–D3). The usage-specific calculation done in-house is `tok/s` **per response** (`Δoutput / Δt_turn` over the JSONL tail), which ccusage doesn't offer. This is not token-stream telemetry: intermediate JSONL writes can produce partial ticks (D27), final closure produces the completed tick, and the needle decays between writes.

**Current state: Phases 0–7 done** (real checklist in `docs/ROADMAP.md`, rationale in `docs/DECISIONS.md` D1–D42). The three continuous sensors (`engine`, `burn`, `sensor`) run on dedicated threads and feed the speedometer, estimated/official segment bar, model selector, PACE/AUTO footer, redline state, and dynamic tray ring. The display is a 4-page MFD: trip, daily history, weekly limits/model cost, and Settings. Settings owns default/page order, theme, permission sound/hook consent, and position reset. Missing Bun/ccusage is handled by the background installer flow (D9/D36). The same executable has three dispatch modes: GUI, Claude Code `statusline`, and opt-in `permission-hook`. The permission gate queues concurrent requests FIFO and exposes Approve, Deny, and supported Always Allow actions over a Unix socket; if the GUI is unavailable it fails open to Claude Code's terminal prompt. Current verified baseline: `cargo test` **59/59**, Rustfmt, strict Clippy, and the Vite production build all pass.

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

Verification baseline:

```bash
npm run build
cargo fmt --manifest-path src-tauri/Cargo.toml --check
cargo clippy --manifest-path src-tauri/Cargo.toml --all-targets --all-features -- -D warnings
cargo test --manifest-path src-tauri/Cargo.toml # 59 tests
```

Frontend: production build only; no unit-test suite or linter is configured yet.

Releases: `npm run release -- <patch|minor|major|X.Y.Z>` (scripts/release.mjs) — bumps the version in `package.json`/`tauri.conf.json`/`Cargo.toml`/`Cargo.lock` in sync, runs `cargo test`, commits, tags `vX.Y.Z`, pushes. The tag triggers `.github/workflows/release.yml`: re-gates on tests, builds the **unsigned** universal (arm64+x86_64) dmg on `macos-latest`, **publishes** the GitHub Release (not a draft — the Homebrew cask URL must be live when the tap update lands) and bumps the cask in `jmtrs/homebrew-tap` (`Casks/cc-autobahn.rb`, skipped if the `HOMEBREW_TAP_TOKEN` secret is unset). No signing (D34); process rationale in D35.

## Architecture (two layers)

- **Rust backend (`src-tauri/`)** — responsible for **all I/O**, never blocks the UI. Each sensor is a directory module, split by concern: `engine/` (`mod.rs`: `detect` global ccusage → npx → bunx + poll loop; `blocks.rs`: `poll_once`/`ccusage blocks --active --json` at **slow cadence 15 s**, D13; `install.rs`: `install_bun`, fire-and-forget — spawns a `std::thread` for the actual Bun install and reports via `install-progress`/`install-succeeded`/`install-failed` events instead of blocking the command handler, D36; `history.rs`: `history_daily`/`ccusage claude daily --json`, **on-demand** cadence, D33), `burn/` (`zulu.rs`: Zulu timestamp parsing; `parser.rs`: `TurnState`/`process_line`, pure turn-calc logic; `tail.rs`: JSONL file tail → `tok/s` per response, D17/D27), `sensor/` (`mod.rs`: shared paths + tail watcher; `statusline_bin.rs`: the `statusline` CLI entrypoint; `install.rs`: settings.json auto-install/uninstall + `refresh_if_stale`, a background-thread self-refresh of the installed binary copy on every startup, D36), `permission/` (`mod.rs`: FIFO `PendingQueue` + Unix-socket listener thread + `permission_approve`/`permission_deny` commands, D42; `hook_bin.rs`: the `permission-hook` CLI entrypoint, blocks on the socket for a decision and fails open on any failure; `install.rs`: settings.json `hooks.PermissionRequest` array merge/unmerge + `refresh_if_stale`, same shape as `sensor::install` but array-based), `pathfix.rs` (PATH hardening at GUI startup — prepends Homebrew/Bun paths when present on disk, since a Finder/Dock launch inherits launchd's bare PATH, D36), `tray_icon.rs` (tray icon progress ring, D30, plus the `alert`/`pending_permission` blink states), `window.rs` (PIN state, hide-on-blur, panel positioning), `tray.rs` (menu-bar menu/icon/click).
- **Frontend (webview, `index.html` + `src/`)** — presentation plus narrowly scoped native window commands; receives data via Tauri IPC/events. `src/style.css` = VFD skin; `src/main.js` wires modules for telemetry, redline, consent/permission gates, synthetic cursor, permission sounds, themes, and D41 window drag/reset, plus the four MFD pages. `redline.js` drives screen/tray alerts from PACE/AUTO; `window-drag.js` invokes native start-dragging/reset behavior only in the documented drag zones.

**Three continuous sensors + one on-demand fetch, four cadences (D13/D33), plus one event-driven gate:** ccusage `blocks` = slow poll (cost/projection); JSONL tail = event per turn (`tok/s`); statusline = push (official `rate_limits` data); ccusage `claude daily` = on-demand, fetched only when the History/Limits MFD page opens; the `PermissionRequest` hook (D42) is neither poll nor push — it's a blocking request/response over a Unix socket, one connection per Claude Code tool call that needs a decision.

**Statusline sensor (D12) — how the official data arrives:** the statusline JSON is *push* (Claude Code passes it via stdin only to a configured script); an external window doesn't receive it passively. cc-autobahn **is** that script and self-installs: it writes `statusLine` into `~/.claude/settings.json` (consent + backup + rollback) pointing at its own binary, which emits the normal line to stdout and atomically writes `~/.claude/cc-autobahn-status.json`. The GUI tails that private file. It is the only source of official `rate_limits.five_hour/seven_day` data.

**Permission hook (D42) — approve/deny from the cluster:** unlike the statusLine sensor, a Claude Code hook is *synchronous* — Claude Code blocks the tool call until the hook process exits. The file+poll pattern above can't answer that, so this uses a real `std::os::unix::net::UnixListener` socket (`~/.claude/cc-autobahn/permission.sock`): the hook blocks on it waiting for a human's Approve/Deny click, and prints nothing (letting Claude Code fall back to its own terminal prompt) if cc-autobahn isn't running or nobody answers in time. Concurrent sessions queue FIFO. Opt-in from the Settings page, not auto-installed like the statusline sensor.

Target flow: the backend emits events (`blocks-update`, `burn-tick`, `sensor-update`, `engine-missing`, `permission-pending`, `permission-resolved`) that the frontend listens to and renders. Details in `docs/ARCHITECTURE.md`.

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

- **Window config in `tauri.conf.json`; permissions in `capabilities/default.json`** (v2). The window has `label: "cluster"`, starts hidden (`visible:false`), and uses `core:default`, `core:event:default`, and `core:window:allow-start-dragging`. Tray anchoring/persistence live in Rust; the frontend uses the narrow D41 drag/reset bridge. `app.macOSPrivateApi: true` is required for transparency on macOS (D14) — do not remove it.
- **Tray/menu-bar (D24)**: show/hide/position live in `src-tauri/src/window.rs`; the menu/icon/"Quit"/click-to-toggle live in `src-tauri/src/tray.rs` (`TrayIconBuilder`, `tray-icon` feature of the `tauri` crate itself, no new plugin); `main.rs` just wires the two together in `.setup()`. The **icon itself** is a progress ring redrawn at runtime by `tray_icon.rs` (D30), called from `engine/`/`sensor/` on every new data point — not a static PNG (that PNG, `icons/tray-icon-template.png`, only remains as the initial icon before the first redraw). **Always use `TrayIcon::set_icon_with_as_template()`, never plain `set_icon()`** — `set_icon()` doesn't preserve macOS's "template" flag across calls and the icon gets repainted as fixed black instead of adapting to light/dark mode (real bug, D30). `ActivationPolicy::Accessory` only on macOS (`#[cfg(target_os = "macos")]`); the rest of the tray API is cross-platform. Only macOS tested so far.
- **Exec from Rust with `std::process::Command`, NOT `tauri-plugin-shell`** (D16). The plugin is for exec from the frontend JS; our I/O is trusted backend code. The engine runs on a dedicated `std::thread` (no async framework). Zero new deps.
- **`macos-private-api` (cargo feature) is coupled to `macOSPrivateApi` (conf)**: if you touch one, touch the other. Tauri's build script fails if they don't match.
- **CSP already applied (D15)**: the restrictive `security.csp` policy in `tauri.conf.json` has been active since the first IPC command landed — verified against `sensor_status`/`install_sensor`/`set_pinned` in `tauri dev`. Do not revert to `null`.
- **Fixed dev port 1420** (`vite.config.js` + `devUrl`); `clearScreen: false` to avoid losing Rust logs.
- **Resolved dependencies are locked** by `package-lock.json`/`Cargo.lock`; upgrades are intentional (D10). Do not downgrade Vite/Tauri/serde without cause.
- **Honest precision** (D11): cost under subscription is **estimated**; the `rate_limits` window is **official** data. Do not present estimates as real billing.
- **Documentation and comments in English**; the ADRs in `docs/DECISIONS.md` record the rationale behind each decision — consult them before changing architecture, the data engine, or the aesthetics.

## Known correctness debt

- The Claude permission queue currently uses required `prompt_id` as the request key. Current Claude hook documentation treats it as prompt correlation data, not a guaranteed unique tool-approval ID, and older payloads may omit it. Before adding another provider, generate a per-hook-invocation ID, keep `prompt_id` optional metadata, and route decisions by provider + generated request ID.
- Current Always Allow emulates Claude behavior locally: exact Bash rules are written to project settings, while Read/Edit/Write approvals are remembered in GUI memory. Prefer Claude's native `permission_suggestions`/`updatedPermissions` contract when modernizing this path; do not extend the emulation blindly.
