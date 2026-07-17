# Roadmap

Implementation order, one layer at a time, verifying before moving forward.

## Phase 0 — Chassis ✅ (current)

- [x] Scaffold Tauri v2 (frameless, always-on-top, transparent, draggable).
- [x] `macOSPrivateApi: true` for real transparency on macOS (D14).
- [x] Static amber VFD W203 skin (`index.html` + `style.css`).
- [x] Live clock + segment bar (shell, no token data).
- [x] v2 permissions (`capabilities/default.json`).
- [x] Generated amber icon (`scripts/make-icon.mjs` → `tauri icon`).
- [x] Dependencies pinned to latest stable versions.
- [x] Docs (this directory).

## Phase 1 — Data engine ✅

- [x] Exec with `std::process::Command` from Rust, no shell plugin (D16).
- [x] `engine::detect` — walks `$PATH`: ccusage global → npx → bunx → none.
- [x] `engine::poll_once` — `ccusage blocks --active --json`, serde parsing,
      `blocks-update` event every **15 s** (D13). Dedicated thread, no panics.
- [x] Serde models against real ccusage v20 JSON (tokens, cost, burn, projection).
- [x] Error handling: missing engine → `engine-missing`; one-off failure →
      `engine-error`; no active block → `blocks-idle`.
- [x] Frontend listens to events (saved outside Tauri); logged in Phase 1.
- [x] ~~Apply restrictive CSP and verify HMR in `tauri dev` (D15)~~ — done
      in Phase 3 (duplicate checkbox, see there).

## Phase 2 — tok/s per response ✅

- [x] **Validate the premise** (2026-07-16): the JSONL only reports `usage` when the
      turn ends → an instantaneous needle is impossible; redefined to **per response** (D8).
- [x] `engine::burn` (`burn/`, split into `mod.rs`/`zulu.rs`/`parser.rs`/`tail.rs` in D32) — tail of the most recent JSONL in
      `~/.claude/projects/**/*.jsonl`, EOF-start; `stat`+`read` of the active file
      every 200 ms, re-scan of which file is active every 5 s (D17).
- [x] `Δoutput / Δt_turn` calculation on turn close (`end_turn`/`stop_sequence`,
      dedup by `message.id`) → `burn-tick` event (D17). `cargo test` 25/25
      against real JSONL (case D8 = 55.0 tok/s verified).
- [x] **Partial** tick per intermediate message (`tool_use`, etc.) on turns with
      tool use, without waiting for the turn's final close (D27).
- [x] Frontend speedometer: damped spring (step + overshoot) +
      decay with a "spring" back to idle; honest label, not "instantaneous" (D18).

## Phase 3 — statusline sensor + wire up display ✅

- [x] **Track A — wire up `blocks-update`** (frontend only): `#odo`, `#session-time`,
      `#avg`, `#autonomie` (EST), `#segments` (projection), `.gear` from `models[]`.
- [x] `engine::sensor` (`sensor/`, split into `mod.rs`/`statusline_bin.rs`/`install.rs` in D32) — dual binary mode (`statusline` →
      early-return, 10 ms; D19), chaining the previous statusLine (D21), sensor
      file written atomically, tail in a dedicated thread every 2 s → `sensor-update`/
      `sensor-state`.
- [x] Replace placeholders with live data (odometer/trip/cost from `blocks`;
      bar/gear/effort from the sensor).
- [x] Segment bar = **official** `rate_limits.five_hour` autonomy
      (switches over the estimated one, D23).
- [x] PRND selector = `model.id`. Kickdown (`effort.level` as small bars) was
      implemented and later **removed** due to visual feedback — it added nothing (D29).
- [x] `seven_day` as `.screen` border tint past 80% (D23, no new DOM).
- [x] **Auto-installation** of the sensor: `install_sensor`/`uninstall_sensor`/
      `sensor_status` (round-trip `Value`, backup+rollback, stable binary copy D20,
      strict JSON D22) + consent UI with diff preview.
- [x] Restrictive CSP applied and verified (D15).

## Phase 4 — Zero friction (auto-wiring, D9)

- [x] "CHECK ENGINE" screen when the engine is missing (overlay in `index.html`,
      painted via `engine_status()` on startup + live `engine-missing`/
      `engine-detected`/`blocks-update` events, without depending on winning the
      race against the first event).
- [x] "INSTALL ENGINE" button (`install_bun` in `engine/install.rs`: official Bun
      installer via `std::process::Command`, process `PATH` manually updated
      after installing, retries `detect()` and relaunches `engine::start`).
      macOS/Linux; on Windows a manual-install message (project still
      untested on that OS, D24). Verified live (overlay + button + text
      fixed from `white-space: pre-wrap` inherited from `.sensor-body`).
      Later moved off the UI thread and given staged progress feedback
      (spinner, staged button text) after live clean-machine testing showed
      the synchronous installer call froze the whole webview (D36).
- [x] ~~Auto-install statusline sensor~~ — done in Phase 3 (D19-D22), not
      Phase 4: `install_sensor`/`uninstall_sensor`/`sensor_status` in `sensor/install.rs`.
- [x] PATH hardening at GUI startup (`pathfix::apply`) so a Finder/Dock
      launch — which inherits launchd's bare PATH, not the terminal's — still
      finds an already-installed Homebrew/Bun engine (D36).
- [x] Installed statusline binary self-refreshes on every startup
      (`sensor::install::refresh_if_stale`) so an old release's copy doesn't
      keep pointing `statusLine` at dead code forever (D36).
- [ ] (Optional) Package Bun as a Tauri sidecar.

## Phase 4.5 — Tray/menu-bar (D24, brought forward, done)

- [x] Menu-bar icon (`TrayIconBuilder`, `tray-icon` feature, no new plugin).
- [x] Dynamic icon: progress ring (% of remaining 5h window) redrawn at
      runtime from `engine`/`sensor`, no drawing deps — replaces the initial
      static PNG (D30).
- [x] `ActivationPolicy::Accessory` on macOS — no Dock or Cmd+Tab.
- [x] Left click shows/hides the panel anchored under the icon (position from
      `TrayIconEvent::rect`, clamped against screen edges).
- [x] Hide-on-blur (`WindowEvent::Focused(false)`) + 300 ms anti-race guard
      (closing by clicking the icon doesn't reopen it).
- [x] Context menu (right click) with "Quit cc-autobahn".
- [x] `data-tauri-drag-region` removed; capabilities trimmed to
      `core:default`/`core:event:default`.
- [ ] (Future) Windows/Linux — the API is cross-platform except for
      `set_activation_policy` (macOS only), still to be tested on those OSes.

## Phase 5 — Integration and polish

- [x] System tray (show/hide, quit) — see Phase 4.5 / D24.
- [x] PACE/AUTO footer (recent pace vs. block average; autonomy
      adjusted to pace, official sensor only) — replaces "LAST tok/s" (D28).
- [x] Split fat files (`engine.rs`/`sensor.rs`/`burn.rs`/`main.rs`/`main.js`) into
      concern-sized modules, no behavior change (D32).

## Phase 6 — MFD pages: history + limits (D33)

- [x] 4-page MFD cycle (`#mfd-btn`, header): Page 0 trip computer (unchanged),
      Page 1 History, Page 2 Limits, Page 3 Settings — cycles like the W203's
      real stalk-mounted trip-computer button instead of cramming more
      fields into Page 0.
- [x] Page 1 — daily history (`ccusage claude daily --json`, **scoped to
      `claude`**, not the top-level multi-agent command), 30-day bar
      sparkline + total. New `engine::history::history_daily` command,
      on-demand cadence (D13's 4th class): fetched on page-open, not polled.
- [x] Page 2 — official weekly rate-limit window (`sevenDayPct`/
      `sevenDayResetsAt`, already parsed since D23 but only ever used as a
      border tint), today's per-model cost split (reuses Page 1's fetch),
      instant vs. average $/h burn rate (`burnRate.costPerHour`, already
      parsed since D28, never painted).
- [x] Page 3 — default landing page + which optional pages are in the
      cycle, `localStorage` only. Deliberately deferred: project filter,
      cost-mode toggle (would need mutable poll-settings state shared with
      the continuous `engine::start` loop — not justified yet).

## Verification per phase

After each phase: `npm run tauri dev`, check that the cluster starts and the
new data appears without breaking what came before. The first `cargo build` is slow (it compiles
the webview); that's normal.
