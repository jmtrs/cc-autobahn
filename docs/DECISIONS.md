# Decision log (ADR)

Decisions made during design, with their reasoning. Lightweight format.

> Test counts inside individual decisions are point-in-time verification
> records for that change, not the current repository total. The current
> automated baseline is 77 Rust and 9 frontend tests; see `README.md` and `docs/ROADMAP.md`.

## D1 — Don't reinvent the data engine

**Decision**: consume `ccusage` as the data source, don't reimplement log
parsing or pricing.
**Reasoning**: parsing JSONL, calculating pricing, deduplicating the shared 5 h
block, and applying the Opus multiplier is complex and prone to billing errors.
It's already solved and maintained. This project's value is in the visual layer.

## D2 — ccusage over the alternatives

**Decision**: engine = `ccusage` (ryoppippi).
**Reasoning**: it's the de facto standard, the most used and stable, and exposes
clean `--json` output. Alternatives evaluated: Claude-Code-Usage-Monitor (Maciek) and
par-cc-usage (good but less standard), ccburn/codeburn (newer).
**Consequence**: no fork. It runs as a child process and its JSON is parsed.

## D3 — No forking

**Decision**: zero fork of ccusage or of any monitor.
**Reasoning (user)**: "I don't want a fork". Keep the external engine intact and
updatable; we only build the layer on top.

## D4 — Aesthetic: amber W203 VFD display (no SVG needles)

**Decision**: replicate the amber dot-matrix display of the Mercedes W203.
**Reasoning**: the user's reference photos (W203) show a text VFD and
segment bars, not analog needles. It's more authentic, lighter, and easier
than drawing SVG needles. The previous idea of a cluster with analog needles was discarded.

## D5 — Tauri over Electron

**Decision**: Tauri v2.
**Reasoning**: ~5 MB binary vs ~150 MB, native Rust backend for exec/tail,
native always-on-top + frameless + transparent, real cross-OS. User: "I like
Tauri". Requirement: "not clunky, very well integrated, very easy to use".

## D6 — Always-on-top floating widget (no TUI, no statusline)

**Decision**: frameless floating window, always visible.
**Reasoning (user)**: "on-screen visible", "well integrated", "good design like a
German car". The unicode TUI doesn't give the look; the 1-line statusline isn't a
cluster. Both were discarded as the primary form.
**Note (doesn't conflict with D12)**: here the statusline is rejected as a **display
form**. D12 uses it as a **data sensor** (source of the official JSON), not as UI. Different
things.

## D7 — PRND selector = active model

**Decision**: reinterpret the P R N D gear selector as a **model** selector
(O/S/H/F), with the active one lit up.
**Reasoning (user)**: the real PRND marks the automatic transmission's gear; we map it to
the running model using its initial. Effort shown as "kickdown" below.

## D8 — Own tok/s per response (JSONL tail) — corrected

**Decision**: display **average tok/s per response** (`Δoutput / Δt_turn`),
computed by us from the JSONL tail. The needle **jumps on completion** of each
turn and decays with a spring at idle. It is NOT a real-time instantaneous needle.
**Reasoning (empirically validated 2026-07-16)**: an active JSONL was inspected.
The `usage` field **is not streamed**: it is stamped **identically** on all
lines of a turn and only appears **when the turn finishes** (e.g. a turn of
3008 output tokens lands all at once after 36 s of silence). The log has **no
visibility into the in-progress turn**. Therefore an "instantaneous needle that
reacts as you press" is **physically impossible** from the JSONL — the most honest
option is the per-response average, rendered as a step + decay.
**Consequence (D11, honesty)**: it is forbidden to label the speedometer as
"instantaneous". It's still differentiating (no competitor shows tok/s per
response), but with the true label. Real streaming would require intercepting
the API or OTEL metrics with streaming → parked for Phase 6, optional.

## D9 — Zero friction = the app wires itself up (redefined)

**Decision**: zero friction does **not** mean "avoid wiring anything up"; it means the
**machine does the configuration**. The driver turns the key, nothing else. It applies to two
wires: (a) the data engine —ccusage global → npx → bunx → install Bun button—, and
(b) the **statusline sensor** (see D12), which the app installs itself with
consent and rollback.
**Reasoning**: the previous literal reading ("the user touches nothing") created a false
dilemma with the official `rate_limits` data (only arrives via statusline, which requires
config). A Mercedes doesn't estimate the tank level: it reads the real sensor. Estimating when
the official data exists is unacceptable (D11). But the driver isn't asked to solder the
sensor either: that's an installation defect, not the price to pay. The app self-wires.
**Consequence**: Phase 4 absorbs the statusline self-wiring in addition to the engine.

## D10 — Latest stable versions pinned

**Decision**: pin dependencies to the latest stable versions (vite 8.1.5, tauri 2.11,
@tauri-apps/cli 2.11.4, api 2.11.1, serde 1.0.228, serde_json 1.0.150).
**Reasoning (user)**: "I only want the latest and most stable". Fixed `vite ^6`
(capped at 6.x) to `^8.1.5`.

## D11 — Precision honesty

**Decision**: show that the price under a subscription is **estimated**; the autonomy
(`rate_limits`) is **official** data; real billing lives in the Claude Console.
**Reasoning**: transparency; ccusage documents that the cost is an approximation.

## D12 — Self-installed statusline sensor (official data, no friction)

**Decision**: cc-autobahn **is** the Claude Code statusline command, and it
installs itself. On first launch it reads `~/.claude/settings.json`; with **one
consent** ("connect the sensor"), it writes the `statusLine` key pointing to its
own binary, saving a **backup** of the previous value (reversible). That binary, on
every Claude Code invocation, does **two things**: (1) it emits the normal
statusline line to **stdout** —respecting whatever the user had, or a default one— so as
not to break their terminal; (2) it writes the full JSON (`rate_limits`, `model`,
`effort`, `cost`, `context_window`) to a **socket/file** (`$XDG_RUNTIME_DIR` or
`~/.claude/cc-autobahn.sock`) that the window **tails**.
**Reasoning**: the statusline JSON is **push** (Claude Code passes it via stdin only to
a configured script); an external window doesn't receive it passively. It's the only
source of the **official** 5 h / 7 d window data (`rate_limits`). Giving it up
and estimating instead would violate D11; requiring manual editing would violate the spirit of D9. The
third way —a self-configuring wrapper that respects what was there before— resolves both.
**Consequence**: statusline only fires when Claude Code renders → the cluster
lights up with the engine running and goes dim at idle (faithful to the car). It's a wrapper, not a
hijack: backup + rollback are mandatory.

## D13 — Separate cadences per source (not a single poll)

**Decision**: each sensor has its own cadence; a single poll at 1–2 s is **forbidden**.
- `ccusage blocks` (cost/projection/history): **slow poll, 10–30 s**, or a
  persistent process. The 5 h block doesn't change by the second.
- JSONL tail (`tok/s` per response): **event-driven** (when the log is written),
  not polling.
- Statusline (`rate_limits`, model, effort): **push**, arrives whenever Claude Code
  renders.
**Reasoning**: `npx -y ccusage@latest` every 1–2 s spins up Node + resolves the package on every
tick (hundreds of ms, CPU) for data that barely changes. Wasteful. Cadence = the
data's real rate of change.

## D14 — `macOSPrivateApi` for real transparency

**Decision**: `app.macOSPrivateApi: true` in `tauri.conf.json`.
**Reasoning**: on macOS, `transparent: true` + `decorations: false` requires the private
API for real transparency; without it, the background shows up black. Cost accepted: cannot
be published on the Mac App Store (irrelevant, direct distribution).

## D15 — CSP deferred until the first IPC (not a silent `null`)

**Decision**: `security.csp` stays `null` **as long as the chassis has no IPC or network**.
Once the first Tauri command lands (Phase 1), apply a restrictive CSP and
**verify it in `tauri dev`** (Vite's HMR websocket must survive):
`default-src 'self'; img-src 'self' data:; style-src 'self'; script-src 'self';
connect-src 'self' ipc: http://ipc.localhost ws://localhost:1420`
**Reasoning**: today there's no attack surface (no fetch, no IPC, no remote content). Flipping
CSP blindly could break HMR and isn't verifiable without a build. The exact policy and its
trigger are documented here so it isn't forgotten, instead of leaving `null` unexplained.
Harden it once there's something to protect.

## D16 — Exec from Rust with `std::process`, no `tauri-plugin-shell`

**Decision**: run ccusage with `std::process::Command` in the Rust backend. `tauri-plugin-shell`
is **not** used.
**Reasoning**: the shell plugin exists to invoke processes from the untrusted **frontend JS**
(with an allowlist in capabilities). Our I/O lives in Rust (trusted), so
`std::process::Command` is enough: zero dependency, zero extra capability, simpler
and more solid. True to the W203 spirit: minimal parts, all serviceable.
**Consequence**: the poll runs in a dedicated `std::thread` with `sleep` (no async
framework). Revisit only if in Phase 4 we package Bun as a *sidecar* (that might
want the plugin). Corrects the earlier finding that assumed the plugin was necessary.

## D17 — tok/s sensor: turn = sequence up to `end_turn`, tail via `stat`

**Decision**: the `burn` sensor (Phase 2) computes `tok/s` **per complete turn**, where
a turn = the sequence of `assistant` messages that closes with `stop_reason` ∈
{`end_turn`, `stop_sequence`}.

- `Δoutput` = Σ `output_tokens` of the turn's `assistant` messages,
  **deduplicated by `message.id`** (rewrites carry the same value — count it
  only once). Includes intermediate `tool_use` calls, not just the final message: it's all
  output generated in that response.
- `Δt_turn` = wall-clock `ts(current close) − ts(previous close)`; if there's no previous
  close (when hooking into the file mid-session), from the first accumulated message.
  `durationMs` in the JSONL is `null` → there's no separable API time,
  so the wall-clock includes tool execution time (honest and measurable).
- File selection: the `.jsonl` with the highest `mtime` under
  `~/.claude/projects/**/*.jsonl` (= currently active). Re-scan (which file is
  active) every 5 s; on session rotation it **starts at EOF** — zero historical
  noise, the needle starts at idle.
- Tail **via `stat` + `read` every 200 ms in a dedicated thread** (lowered from 1 s, D27),
  without `notify`/kqueue.

**Reasoning**: empirical measurement of a real JSONL (2026-07-16, `cargo test` 11/11). The
D8 case (the `end_turn` turn of 3008 tok plus a previous `tool_use` of 583) gives
`Δoutput=3591, Δt=65.278 s → 55.0 tok/s`. The per-second `stat` is **not** the waste that
D13 forbids (that was Node spawning per tick): it's a trivial syscall. kqueue would
require the `notify` crate — rejected under the W203 principle of minimal parts. The
Zulu timestamp is parsed by hand (no `chrono`): Claude Code's format is always
`YYYY-MM-DDTHH:MM:SS.mmmZ`. `pos` only advances up to the last `\n` (residual buffer)
→ a line is never lost to a partial write.

**Correction to D8**: D8 literally stated that `usage` is stamped "identically on all
lines of a turn". In reality each `assistant` message carries its own `usage`
with its own `output_tokens`; what's identical are the **rewrites of the same
`message.id`**. D8's conclusion holds unchanged (there's no streaming; the data
arrives when the message closes), only the mechanism is refined.

## D18 — Needle with physical spring (step + decay)

**Decision**: the speedometer (`#burn`) isn't a flat value: after each `burn-tick` the
`target` jumps to the turn's `tok/s` and an **underdamped spring** drives the
needle there with mechanical overshoot (`v += (target−pos)·k; v *= damp; pos += v`).
Without a fresh tick for 2 s, the `target` decays to 0 (idle). The secondary reading
`#burn-inst` shows the raw `tok/s` of the last turn, without the spring.

**Reasoning**: fidelity to the W203 leather (analog needle with inertia, not a digit that
jumps). And honesty (D11): the label is "tok/s per response", **never
"instantaneous"** — the needle decays because the data only arrives when the turn closes (D8).

## D19 — Dual binary: same bin + early-return (no separate bin for statusline)

**Decision**: Claude Code's `statusLine` command is the **same binary**,
`cc-autobahn`. `main` parses `argv[1]=="statusline"` and returns **before**
building `tauri::Builder` → no GUI/webview starts. Measured: **10 ms** per
invocation (debug, 7 runs, p95 < 30 ms).
**Reasoning**: splitting off a minimal separate `[[bin]]` adds workspace complexity and
a shared lib to save <30 ms that the early-return already achieves. If the
invocation cadence went up and the overhead became noticeable, this would be reconsidered.

## D20 — Stable path: copy the bin, never `current_exe()` into settings

**Decision**: on install, the binary is **copied** to
`${CLAUDE_CONFIG_DIR:-~/.claude}/cc-autobahn/cc-autobahn-statusline` (0755), and
**that** path is written into `settings.json`. Never `std::env::current_exe()`.
**Reasoning**: on macOS, an unnotarized downloaded `.app` runs from
`/private/var/folders/.../AppTranslocation/<hash>/...` (Gatekeeper translocation).
`current_exe()` returns that **ephemeral** path; the next launch
changes the hash and the statusline would point to nothing. Copying to a stable path
under the config dir resolves this identically in dev and release.

## D21 — Chain passthrough of the previous statusLine (respects what was already there)

**Decision**: statusline mode reads stdin, **re-executes** the user's previous
`statusLine` (saved at `cc-autobahn/prev-statusline`) via `sh -c` with that
same stdin, re-emits its stdout, and **additionally** dumps the JSON to the sensor file.
If there's no prev or it fails, a default line is used.
**Reasoning**: D12 promises to "respect whatever they had". Claude Code only invokes a
single `command`; the wrapper doesn't receive the previous output, but it can re-execute
it. Without chaining, any existing statusline would be silently destroyed (e.g. the
caveman plugin). Idempotent: if the current statusLine already points to us, we don't
capture ourselves as prev (avoids an infinite recursive chain).

## D22 — settings.json only if it parses as strict JSON

**Decision**: `settings.json` is mutated via a round-trip
`serde_json::Value` (never a typed struct — so unknown fields aren't dropped), and only
if the file parses as **strict JSON**. If it has comments/JSONC or is malformed → it isn't
touched, CTA "configure manually". 0600 backup without overwriting,
atomic tmp+rename write, post-write re-validation + rollback.
**Reasoning**: Claude Code validates `settings.json` with strict Zod; one badly
written field leaves the user without config. The round-trip with `Value` preserves everything
we don't touch; validation + rollback prevents leaving it unusable.

## D23 — Honest metrics without new DOM (Track A vs B)

**Decision**: the `#segments` bar reflects ccusage's **projection** (marked
`EST`) as long as there's no sensor, and the **official %** `rate_limits.five_hour` when
there is one (without `EST`). `seven_day` adds no DOM: it tints the `.screen` border red
past 80% (a W203-style reserve warning light). `#odo` shows tokens from the **5h block**, not
lifetime accumulated totals (those require `ccusage daily`, Phase 6).
**Reasoning**: D11 (don't estimate when official data exists) and fidelity to the W203 layout. The
automatic switching between estimated and official source always prioritizes the official one
without adding elements or hidden modes.
**Bug fixed (post-D24)**: the official source filled the segments based on
`fiveHourPct` (% **spent**), the opposite of the estimated source (`applyEstimated`, which already
used **remaining** minutes) — a tank that fills as you spend instead
of emptying, inconsistent between the two sources and against the gas-pump icon.
Fixed to `100 - fiveHourPct` in `onSensorUpdate` (`src/main.js`).

## D24 — Tray/menu-bar replaces the always-visible floating window (supersedes D6)

**Decision**: the cluster stops being a permanently visible always-on-top floating
window and becomes a fixed icon in the macOS menu bar (`TrayIconBuilder`, the
`tray-icon` feature of the `tauri` crate itself — no new plugin, D10/D16). Left click
shows/hides a panel anchored under the icon (position computed by hand from
`TrayIconEvent::Click { rect, .. }`, without `tauri-plugin-positioner`); clicking outside
hides it (`WindowEvent::Focused(false)`); right click opens a menu with "Quit
cc-autobahn". `ActivationPolicy::Accessory` on macOS removes the icon from the Dock/Cmd+Tab.
The window starts hidden (`"visible": false`, without `center`) and keeps
`alwaysOnTop` (to float over any app while visible — not the
same as "positioned under the tray").
**Reasoning (user)**: they no longer want to drag/move the window by hand; they prefer the
menu-bar utility model (Maccy/Ice/Bartender) — icon always accessible,
panel on demand, zero manual positioning friction.
**Supersedes D6**: D6 documented the always-visible floating window as a deliberate
decision ("on-screen visible", quoted from the user). It's replaced because the
user themselves changed their preference; D6 remains as a historical record of how
the previous design was arrived at, it isn't deleted.
**Consequence**: `data-tauri-drag-region` is removed from `index.html` (dragging is
no longer needed). `capabilities/default.json` loses the `core:window:*`
permissions (vestigial — all tray/window control happens in pure Rust, never via IPC
from JS). A "Quit" menu item is added because `ActivationPolicy::Accessory` leaves
no Dock icon to close the app another way. Anti-race guard (300 ms)
between hide-on-blur and tray-click so that closing the panel by clicking the icon doesn't
reopen it. Only `set_activation_policy` is behind `#[cfg(target_os = "macos")]` — the
rest is cross-platform Tauri v2 API, Windows/Linux are left for later without
requiring an architecture change.
**Scope**: macOS only for now. Verified live (`tauri dev`): icon visible
in the menu bar, panel anchors correctly under the icon, hide-on-blur
works, closing via tray toggle doesn't reopen it, "Quit" menu works, absent from
Dock/Cmd+Tab.

## D25 — Rounded corners via native CALayer (D24 addendum)

**Decision**: the panel uses `objc2-app-kit`/`objc2-quartz-core` to clip the
`NSWindow` at the `CALayer.cornerRadius` level (macOS-only, `#[cfg(target_os =
"macos")]`), instead of relying only on `.cluster`'s CSS `border-radius`.
**Reasoning**: with `transparent:true` + `decorations:false`, Tauri/WebKit on macOS
doesn't properly clip CSS `border-radius` to the window's alpha channel — it leaves
a square "peak" on all 4 corners (a known bug, documented in several issues
in the official `tauri-apps/tauri` repo, with no clean fix in the framework as of
today). Two approaches were discarded before this one:
1. `overflow: hidden` on `.cluster` — it wasn't a CSS overflow problem.
2. `window.set_shadow(false)` — the native shadow wasn't the cause.
3. A window with a straight outer edge (no `border-radius`) — worked without
   artifacts but lost the rounded aesthetic; discarded by preference.
**Why no third-party plugin**: `tauri-plugin-mac-rounded-corners` (cloudworxx) was
evaluated, but it isn't a normal crate — the installer copies source code
(`mod.rs`) directly into the repo, adds the legacy `cocoa`/`objc` 0.2.x stack (unsafe
FFI, duplicating the stack Tauri already uses), and brings "Traffic
Lights" functions that are irrelevant here (the panel has no native window buttons).
**Consequence**: `objc2` (0.6), `objc2-app-kit`, and `objc2-quartz-core` (0.3,
both already resolved in `Cargo.lock` as transitive deps of `macos-private-api`
— D10 spirit: they're exposed to our code without adding new versions to the tree)
are declared under `[target.'cfg(target_os = "macos")'.dependencies]`. `main.rs`
calls `content_view.setWantsLayer(true)` + `layer.setCornerRadius(12.0)` +
`layer.setMasksToBounds(true)` once in `setup()`. `transparent:true` and
`shadow:true` return to `tauri.conf.json`; the `.cluster` CSS `border-radius:12px`
is restored (now correctly clipped by the native layer, and must
match the 12px radius). Verified live: clean corners on all 4,
no peak, with the transparent window + native shadow active.

## D26 — PIN button (D24 addendum)

**Decision**: a "PIN" button in the panel header (`index.html`/`style.css`) that,
when activated, disables hide-on-blur (`WindowEvent::Focused(false)` no longer
hides the window while it's pinned).
**Reasoning (user)**: they wanted to be able to leave the panel open without it closing when
clicking outside, to check it while working in another window.
**Consequence**: new shared state `PinnedState` (`Arc<Mutex<bool>>`)
managed by Tauri (`.manage(...)`), a new `set_pinned` command invoked from
`main.js` (`wirePinButton`). The guard is applied inside the `on_window_event`
handler itself, before touching `last_blur_hide` — if pinned, it neither hides
nor registers the hide, leaving the anti-race guard (D24) intact for when
the PIN is deactivated.

## D27 — Partial tick per intermediate message + tail cadence at 200 ms

**Decision**: two changes in `burn.rs` to lower the perceived latency of the
tok/s speedometer down to the real floor imposed by D8:
1. `TAIL_INTERVAL_MS` drops from 1000 to 200 ms — the `stat`+`read` of a single
   already-known file is a trivial syscall; lowering it has no real cost
   (the cadence of `ACTIVE_RESCAN_SECS = 5 s`, which does scan ALL
   projects, stays unchanged).
2. `TurnState::ingest` now emits a **partial** `burn-tick` for each intermediate
   `assistant` message (e.g. `tool_use`) that isn't the first in the turn,
   with the tok/s of ONLY that message over the Δt since the previous message — without
   waiting for the final `end_turn`/`stop_sequence`. The turn-closing aggregate tick
   (with the turn's total) stays exactly the same as before.
**Reasoning (user)**: in a single-piece response (no tools) there is
nothing to do — the JSONL only has the data when that single write finishes
(D8, validated 2026-07-16, it isn't an adjustable cadence). But in turns with
several tool calls (most real coding work: Read, Edit, Bash) there
ARE several messages written progressively before closing —
waiting for the whole end wasted that information already available on disk.
**Why the turn's first message doesn't emit a partial tick**: its Δt against itself
is 0 (nothing to measure yet); from the second message onward there is a real
Δt from the previous one. Verified with a dedicated test
(`intermediate_tool_use_emits_partial_tick`) and against the previous 24 tests, which
still pass unmodified (additive change, doesn't replace the final tick).
**Consequence**: the `burn-tick` payload can now arrive more often in
long turns with tools; the frontend doesn't change how the speedometer behaves (it already treats
every tick the same: a needle jump). The "LAST tok/s" footer that read this same
payload was replaced by PACE/AUTO (D28) precisely because D27 made it
ambiguous (full turn vs. intermediate message, with no marker to distinguish them).
`ACTIVE_RESCAN_SECS` stays at 5 s, untouched.

## D28 — Footer: PACE (recent rate) / AUTO (rate-adjusted autonomy)

**Decision**: the "LAST tok/s" footer (D26 labeled it, D27 made it ambiguous) is
replaced by two new metrics, togglable by click and persisted in
`localStorage` (key `cc-autobahn.footerMetric`, the first time the project
uses Web Storage):
- **PACE**: `▲/▼ N%` — the difference between the rate of the last 5 min
  (`Σ turnOutputTokens` of received `burn-tick`s, over the real span
  covered) and the block's OUTPUT average, computed by hand with
  `block.tokenCounts.outputTokens / minutes elapsed` (see the correction
  below — it does NOT use ccusage's `burnRate.tokensPerMinute`). `—` if there are no
  recent ticks or no active block.
- **AUTO**: minutes remaining, reprojecting the recent TREND of
  `rate_limits.five_hour.used_percentage` (Δ%/Δt of the last 10 min, minimum
  2 samples separated by ≥2 min) — NOT ccusage's linear projection. `—` with no
  sensor connected, insufficient samples, or a rate ≤0.
**Reasoning (user)**: the old footer added nothing next to the speedometer and
became ambiguous after D27. Actually useful metrics were requested: how much is being
spent RIGHT NOW compared to the average (PACE), and a "range to
empty"-style autonomy that adjusts to the real rate instead of a fixed projection (AUTO).
**Why AUTO is sensor-only**: verified by reading ccusage's real source code
v20 (Rust, `gh api repos/ccusage/ccusage/.../blocks.rs`,
`project_block_usage`): `projection.remainingMinutes` = `block.end_time −
now()`, **pure clock**, doesn't depend on the consumption rate at all.
Reprojecting that quantity by rate wouldn't make mathematical sense (D11: don't
estimate/invent where the data doesn't support it). Only `rate_limits.five_hour`
(official) measures real quota consumption, so only there is reprojection
honest.
**Correction (tested live the same day)**: the initial design reused
ccusage's `burnRate.tokensPerMinute` as the block average. Testing with real
data, PACE stayed pinned at `▼ -100%` despite real activity (turns of
3438, 784, 3625 output tokens). Cause, confirmed against
`TokenCounts::total()` in ccusage's source code: `tokensPerMinute` sums
`input + output + cache_creation + cache_read` — and `cache_read_tokens` can
be huge in long sessions (reuse of cached context on every call), inflating the
denominator far above the pure `output_tokens` that `burn-tick` measures. Comparing
"recent (output only)" against "average (input+output+
cache)" is comparing different magnitudes — the result always lands near
-100% regardless of the real rate. **Fixed**: the block's average is
now computed by hand as `block.tokenCounts.outputTokens / minutes elapsed`
(same `startTime` already used by `session-time`) — same magnitude as
`burn-tick.turnOutputTokens`, a coherent comparison. Lesson: verify a third-party
formula against real data before trusting that it measures the same thing you
think it does, not just against the source code in the abstract. Further confirmed with
`npx ccusage blocks --active --json` live: `tokensPerMinute` reached
**1,872,536** (dominated by `cacheReadInputTokens: 37,386,004`) versus
real `outputTokens: 46,631` — the magnitude of the error would have been ~40x, not
a minor nuance. (ccusage also exposes `tokensPerMinuteForIndicator`
—input+output, without cache— but it still mixes input with output; equally discarded
for not being the same magnitude as `burn-tick`, which is 100% output.)
**Correction 2 (same review, with real sensor data)**: `computeAdjustedAutonomy`
had no ceiling — with real data (`five_hour.used_percentage: 85`, reset in
16 real minutes) it was confirmed that a slow rate could reproject MORE
autonomy than really exists (the window resets at its fixed hour regardless
of the %). **Fixed**: `minutesLeft = min(reprojection,
real_minutes_until_fiveHourResetsAtMs)` — a hard ceiling against the official
reset data, which is 100% certain.
**Correction 3**: `recentTicks` (the PACE buffer) wasn't cleared when the 5h block
rotated — if the rotation happens within the last 5 min of buffer,
"recent" could mix tokens from the old block with the new one's average.
**Fixed**: the buffer is cleared when `block.id` changes (`onBlocksUpdate`).
**Correction 4**: `formatHMin` rounded hours and minutes separately
(`floor(min/60)` + `round(min%60)`), which could produce `m=60` (e.g. 119.5 min →
"1h60" instead of "2h00"). **Fixed**: round once to a whole minute before
splitting into h/m.
**Correction 5**: `computePace` had no "insufficient data" guards
analogous to AUTO's — a block just started (elapsed≈0) or a single very
recent tick (span≈0) could artificially inflate the ratio via near-zero
division. **Fixed**: minimum 1 min of block elapsed and minimum
30 s of tick span before computing, otherwise `—`.
**Correction 6 (the "fuel gauge" autonomy bar, not PACE/AUTO)**: found with
real user screenshots: the official bar showed "0h17" (85% used) and,
after a normal pause (Claude Code not rendering for a while — the sensor
marks it "disconnected" after 60 s, `STALE_SECS`, `sensor.rs`), it jumped to
"EST 4h31" — ccusage's projection, a 5h-window system
**independent** of the official one (`rate_limits`). The jump between the two was a
meaningless number, not just a cosmetic issue. **Fixed**: a new sticky flag
`everSensorConnected` — once there has ever been official data, a
momentary disconnection no longer falls back to ccusage's projection; it **freezes**
as-is (`onBlocksUpdate`/`onSensorState` stop touching
segments/autonomy/gear/kick/warn) and the countdown stays alive with the last known
`fiveHourResetsAtMs` (`refreshAutonomie` no longer depends on
`sensorConnected`, only on having a valid reset — that data doesn't stop
being true just because the sensor is quiet for a while). The fallback to ccusage's "EST"
is now reserved exclusively for when the sensor NEVER connected.
**UI language**: the visible labels (`PACE`, `AUTO`) are in English,
consistent with the rest of the cluster (`AFTER START`, `tok/s`, `Mtok`) — CLAUDE.md's
rule of comments in Spanish applies to code/documentation, not to the
display copy.
**No collision with Phase 6** (`docs/ROADMAP.md`): both metrics use data already
emitted today via `blocks-update`/`sensor-update` (`Block.tokenCounts`,
`rate_limits`); Phase 6 is about `ccusage daily/monthly`, different historical
data, not touched here.

## D29 — Kickdown (effort indicator) removed from the selector

**Decision**: the `.kick` element (`#kick`, four small bars `▂▂▂▂`
representing `effort.level`) is removed from the PRND selector. Removed without leaving
traces: `index.html` (`<span class="kick">`), `style.css` (`.gear .kick`),
`main.js` (`KICK_FULL`, `EFFORT_BARS`, `setKick()` and its two call sites in
`onSensorUpdate`/`onSensorState`) — verified with `grep -i kick` across the
three files after the change, zero matches.
**Reasoning (user)**: direct feedback on a screenshot of the real panel: "those three
horizontal little bars, I'd remove them, they don't add anything".
**Consequence**: `effort.level` still arrives in the `sensor-update` payload
(`SensorUpdate.effortLevel`, `sensor.rs`, untouched) — it just stopped being
rendered, not emitted; recoverable without backend changes if needed. D7 and D28 document
the kickdown as part of the original design and of the frozen state at
their respective times — they remain as a historical record, not edited retroactively
(same criterion as D24 regarding D6).

## D30 — Tray icon as a progress ring (replaces static disc)

**Decision**: the menu-bar icon (D24) drops the fixed static PNG (a filled
disc, generated by `scripts/make-tray-icon.mjs` → `tray-icon-template.png`)
and becomes a **progress ring redrawn at runtime**
(`src-tauri/src/tray_icon.rs`, new module), pixel by pixel and with no
drawing dependencies — the same manual pattern as `make-tray-icon.mjs` (D16:
zero new deps). It represents the **remaining %** of the 5h window with the
same criterion as `#segments` in the panel (a tank that empties, not that
fills, D23): a faint always-visible track (alpha 55/255) + an opaque arc (alpha
255) drawn from 12 o'clock, clockwise. It's redrawn on every new
data point, at the same spot where the corresponding event was already being emitted:
- `engine.rs` (poll ~15s): `remaining_minutes / WINDOW_MIN * 100` from
  ccusage's projection; no active block → full ring (100%).
- `sensor.rs` (official push): `100 - five_hour_pct`.

No estimated-vs-official precedence logic replicated from the frontend (D23):
the tray is a low-commitment glance, the last data point that arrives from
either source wins — a deliberate simplification, not an oversight.
**Reasoning (user)**: "the icon is kind of bad, we should put something that
actually does something" → real data instead of a fixed decoration; "like a
circular charger... that keeps updating every so often" — the same visual
language as a ring tried earlier for the panel gauge (discarded there
because the fuel icon "was fine"; the concept did fit the
tray).
**Bug found and fixed (verified live, `tauri dev`)**:
`TrayIcon::set_icon()` **does not preserve macOS's "template" flag** between
calls — every redraw was repainted as a normal-color image (fixed black),
without adapting to the menu bar's light/dark mode (visually confirmed by
the user: "it looks dark when I have a dark background, it should adapt to
the theme"). Tauri's documentation hints at it in passing
("calling set_icon followed by set_icon_as_template causes a visible
flicker") but doesn't make clear that it's needed on **every** frame, not just
once when building the tray. **Fixed**: use
`TrayIcon::set_icon_with_as_template(icon, true)` (sets image + flag
atomically, designed by Tauri specifically to avoid the flicker of the two
separate calls) on every `set_progress()` invocation, not just
`.icon_as_template(true)` once on the initial `TrayIconBuilder`.
**Consequence**: `app.manage(tray)` stores the `TrayIcon<Wry>` handle after
`.build()` so it can be retrieved from `engine.rs`/`sensor.rs` via
`app.try_state::<TrayIcon<Wry>>()`, without coupling those modules to the tray
construction code in `main.rs`. `tray-icon-template.png` and
`scripts/make-tray-icon.mjs` remain as the initial icon (required by
`TrayIconBuilder::icon()` before there's any data), overwritten almost
immediately by the first `set_progress(100.0)` in `setup()`. No new deps
(D16): `Image::new_owned(rgba, w, h)` builds the image from raw bytes,
without going through the PNG decoder (the `image-png` feature already present was only
needed for the original static `.icon(bytes)`).
**Verified**: `cargo test` 26/26, `cargo clippy` with no warnings, confirmed
visually by the user in `tauri dev` after the template-flag fix.

## D31 — Phase 4: CHECK ENGINE + "Install engine" runs Bun's official installer

**Decision**: implements the rest of D9 (the `ccusage global → npx → bunx →
install Bun button` cascade). Two new commands in `engine.rs`:
- `engine_status()` — `#[tauri::command]` with no arguments, **pull**: returns
  `detect().is_some()`. Paints the "CHECK ENGINE" overlay on the frontend's first
  render without depending on winning the race against the
  `engine-missing` event (the `engine::start` thread may emit it before the
  frontend finishes registering the listener) — same pattern as
  `sensor_status` (D12).
- `install_bun(app)` — runs the official installer
  (`curl -fsSL https://bun.sh/install | bash` via `std::process::Command`,
  D16; macOS/Linux, on Windows a manual-install message, the project still
  untested there, D24). The installer adds `~/.bun/bin` to `PATH` through
  the shell rc file (`.zshrc`/`.bashrc`), which the **already-running** cc-autobahn
  process doesn't re-read — it's prepended into app-managed `PathState` after
  installing, and each engine subprocess receives that value via
  `Command::env("PATH", ...)`, so that `detect()` and the subsequent `Command`s
  find `bunx` without requiring an app restart (D9: true zero friction). If the
  engine appears, `engine::start` is relaunched.

**Reasoning**: without this, "Phase 4 — zero friction" was half done: the
`engine-missing` screen was already emitted since Phase 1 but the frontend only did
`console.warn`, and there was no way to install the engine without leaving the app.

**Bug found and fixed (adversarial review + live test by the
user)**: the overlay's default text was written as static content indented in
`index.html`. `.sensor-body` (a class reused from the sensor overlay, D12) has
`white-space: pre-wrap` — it literally preserves the indentation/line breaks
from the HTML source, breaking the paragraph up in odd places. The sensor overlay
never suffered from this because its text is always set via JS
(`setSensorBody`), never in static HTML. Fixed: `#engine-body`
starts empty in `index.html`, the default text
(`ENGINE_DEFAULT_BODY`) is set in `main.js` the way the sensor overlay already
did. A second finding (concurrent double-click on "Install engine"
triggering two installers in parallel) was fixed with an explicit guard
(`if (btn.disabled) return`) before disabling the button.

**Verified**: `cargo test` 26/26, `cargo clippy` with no warnings, adversarial
review (fresh subagent against the diff) + live test by the user
in `tauri dev` (PATH without `npx`/`bunx`, overlay confirmed, text fixed,
"Install engine" button functional).

## D32 — Split fat files into concern-sized modules (pure refactor, no behavior change)

**Decision**: `engine.rs`, `sensor.rs`, and `burn.rs` had each grown to bundle 2-3
unrelated concerns by history (D9/D12/D17 each adding to the same file rather than
creating a new one); `main.rs` had the Tauri bootstrap, window positioning math, and
tray menu/click wiring all inline in one `setup()` closure; `src/main.js` had grown to
613 lines of 8 unrelated UI widgets as flat top-level functions and globals. Split each
into a directory module by concern, with `#[cfg(test)] mod tests` moving with the code
they test:
- `engine/` — `mod.rs` (`Engine` enum, `detect`, poll loop `start`), `install.rs`
  (`install_bun` + Bun installer), `blocks.rs` (`Block`/`Projection` structs + `poll_once`).
- `sensor/` — `mod.rs` (shared paths, `SensorUpdate`, tail `start`), `statusline_bin.rs`
  (the `statusline` CLI entrypoint), `install.rs` (settings.json install/uninstall commands).
- `burn/` — `mod.rs` (tail thread `start`), `zulu.rs` (Zulu timestamp parsing),
  `parser.rs` (`TurnState`/`process_line`, pure turn-calc logic), `tail.rs` (`Tail`,
  JSONL file tailing).
- `window.rs` (new) — `PinnedState`, `set_pinned`, `position_under_tray`, hide-on-blur
  wiring, the macOS corner-radius block. `tray.rs` (new) — tray menu + icon +
  click-to-toggle (the `REOPEN_GUARD` debounce). `main.rs` thinned to arg parsing +
  `tauri::Builder` wiring, calling `window::wire` / `tray::build`.
- `src/modules/` (new) — `format.js` (VFD formatters), `telemetry-state.js` (shared
  state object between `trip-computer.js` and `footer-metric.js`, avoiding a circular
  import), `clock.js`, `speedometer.js`, `trip-computer.js`, `footer-metric.js`,
  `engine-overlay.js`, `sensor-consent.js`, `pin-button.js`, `ipc-events.js`. `main.js`
  is now a thin entrypoint that imports and wires these on `DOMContentLoaded`.

Cross-submodule Rust items that aren't `#[tauri::command]`s are `pub(crate)`, not full
`pub` — encapsulation preserved despite the split. `tauri::generate_handler!` needs the
full module path to a command (a `pub use` re-export doesn't carry the macro's hidden
generated items), so `main.rs` invokes e.g. `engine::install::install_bun`, not
`engine::install_bun`.

**Reasoning**: mechanical cleanup requested directly ("refactor everything to be
cleaner and more separated, both JS and Rust") — no new functionality, no behavior
change, no new Tauri commands/events, no `capabilities/default.json` change (the
command set is identical, just relocated).

**Verified**: `cargo test` 26/26 (same test names, just relocated), `cargo clippy`
clean, `vite build` clean (19 modules transformed, no import errors), and the
already-running `tauri dev` session (with its file watcher) auto-recompiled and
restarted live across every Rust edit without crashing — confirming the split runs
correctly, not just compiles.

## D33 — MFD pages: cycle screens instead of adding more fields to Page 0 (Phase 6 + D23/D28 loose ends)

**Decision**: instead of growing the single trip-computer readout with more fields
(daily history, weekly rate-limit numbers, per-model cost, instant vs. average burn),
the display is split into 4 pages cycled by one new button (`#mfd-btn`, header,
forward-only, wraps around) — same UX as the W203's real stalk-mounted trip-computer
button:

- **Page 0** — the original trip computer, untouched.
- **Page 1 (History, Phase 6)** — `ccusage claude daily --json` (last 30 days),
  bar sparkline + 30-day total. **Not** `ccusage daily` (no source scope): the
  top-level command mixes in every agent ccusage detects on the machine (Codex,
  Gemini, etc.) if installed — confirmed by running both against real data.
- **Page 2 (Limits)** — three fields that were already flowing into the frontend but
  either reduced to a side-effect or never painted: `sevenDayPct`/`sevenDayResetsAt`
  (D23 already computed these for the border-tint warning and threw the numbers away),
  `burnRate.costPerHour` (D28 already parses it in `engine/blocks.rs`, never read in
  JS), and an average $/h derived client-side from `costUsd / elapsed-since-startTime`
  (no new backend field). Today's per-model cost split reuses Page 1's fetch (see
  below) instead of a second call.
- **Page 3 (Settings)** — front-end only, `localStorage` (same pattern as the D-review
  nameplate override): default landing page, and whether History/Limits are in the
  cycle. Explicitly **not** built: a project filter or cost-mode (auto/calculate/
  display) toggle — both would need a mutable Rust poll-settings state shared with the
  continuous `engine::start` loop, not justified for a first pass (YAGNI).

**Backend**: one new module, `engine/history.rs` (`#[tauri::command] history_daily`),
nested under `engine` (not a sibling top-level module) specifically so it can see
`Engine::base_command`/`label`, which are module-private — Rust privacy rules make
private items visible to descendant modules, not siblings. Date math (`since_date`,
`civil_from_days`) is hand-rolled (Howard Hinnant's algorithm, same family as
`burn::zulu::days_from_civil`) to stay `chrono`-free (D10 zero-new-deps spirit).
Tried `blocks --breakdown` first for the per-model split, hoping to avoid a second
report type — **verified against the real CLI that the flag is a no-op on `blocks`'
JSON output in this ccusage version**; `claude daily`'s `modelBreakdowns` was the only
place that data actually exists, so Page 2's breakdown rides on Page 1's fetch instead.

**Cadence**: a 4th class alongside D13's three (slow poll / per-turn event / push) —
**on-demand**. `history_daily` is not part of the continuous poll loop; it's called
once when Page 1 or Page 2 opens and cached client-side for 5 minutes
(`history-data.js`). Daily totals don't move within a few minutes, so polling them in
the background would be a wasted process spawn for data nobody's looking at.

**Reasoning**: explicit user direction against cramming more readouts onto the single
screen ("no meter más info en la pantalla, hacer que sea customizable"); the W203's
own real trip computer already solves this with a page-cycle button, so the fix was to
follow the car metaphor rather than invent a new pattern.

**Verified**: `cargo test` 31/31 (new: `engine::history` date-math + real-sample
parsing tests), `cargo clippy` clean, `vite build` clean. No Tauri runtime available
in this environment for a full native run, so the page-cycle logic, CSS, and
graceful-no-IPC fallback (`history_daily` no-ops outside Tauri) were driven end-to-end
with Playwright against the plain Vite dev server: full page-cycle (0→1→2→3→0),
Page 3's "hide History" toggle correctly removing it from the cycle, zero console/page
errors across the run. `history_daily`'s real IPC round-trip against a live Claude
Code install is unverified — first `npm run tauri dev` should confirm Page 1/2 render
real numbers, not just the empty-state fallback.

## D34 — Distribution: unsigned GitHub Releases, no Apple Developer ID (yet)

**Decision**: distribute as an **unsigned** universal (arm64 + x86_64) macOS build
attached to GitHub Releases, built by `.github/workflows/release.yml`
(`tauri-action`, triggered by `v*` tags, draft release). No code signing, no
notarization, no auto-updater. The Gatekeeper friction is documented instead of
paid for: right-click → Open, or `xattr -dr com.apple.quarantine`.

**Reasoning**: Developer ID + notarization ($99/yr) is the *only* way to remove
the first-launch warning — ad-hoc/self-signed certs change nothing, and Homebrew
no longer helps (Homebrew 5.0 forces `com.apple.quarantine` on all casks and is
removing `--no-quarantine`). For a 0.x project whose audience is
terminal-comfortable Claude Code users, the documented workaround is acceptable;
the $99 is deferred until there's real traction (unknown users hitting the
warning, a homebrew-cask official submission, or a Tauri auto-updater, which
would want signing anyway).

**Verified**: workflow green on `v0.1.0` (6m19s); the downloaded dmg mounted
and its binary checked out as a real universal fat file (`x86_64 arm64`).

## D35 — Release automation: one local command, CI does the rest (incl. Homebrew cask)

**Decision**: releasing is `npm run release -- <patch|minor|major|X.Y.Z>`
(`scripts/release.mjs`). It refuses a dirty tree / non-main branch / existing
tag, runs `cargo test` as a local gate, bumps the version in the four files
that carry it (`package.json`, `tauri.conf.json`, `Cargo.toml`, `Cargo.lock`)
so they can't drift, then commits, tags and pushes. From the tag,
`release.yml` re-gates on tests, builds the unsigned universal dmg,
**publishes** the GitHub Release (no longer a draft — the cask's download URL
must be live when the tap update lands, unlike D34's original draft flow) and
updates the cask in the **existing** `jmtrs/homebrew-tap` repo
(`Casks/cc-autobahn.rb`, same tap as `no-coauthor`) via the
`HOMEBREW_TAP_TOKEN` secret — the same PAT pattern no-coAuthor's workflow
already uses. The step skips cleanly when the secret is absent, so a missing
token never fails the release itself.

**Reasoning**: three hand-edited version files + manual tag + manual cask
sha256 is exactly the kind of process that drifts and fails silently; one
command + CI removes the failure modes. Own tap rather than homebrew-cask
official (notability threshold not met yet); **cask**, not formula, because
it's a GUI `.app`. The Gatekeeper caveat ships via the cask's `caveats`
stanza (Homebrew ≥ 5 quarantines all casks; `--no-quarantine` is gone).

**Verified**: `brew fetch --cask jmtrs/tap/cc-autobahn` green on v0.1.0
(downloads the dmg and validates the sha256); `scripts/release.mjs` syntax
checked and its four version-bump regexes matched against the real files. The
cask-update step itself first runs on the next tag.

## D36 — Finder-launch PATH hardening, non-blocking Bun install, self-healing statusline copy

**Decision**: three fixes surfaced by actually simulating a clean machine
(isolated `HOME`/`CLAUDE_CONFIG_DIR`/`PATH`, and briefly relocating the real
`/opt/homebrew/bin/npx`/`bunx` to reproduce a truly engine-less Mac) instead
of reasoning about it from the source:

1. **`pathfix::apply()`** (new module, `src-tauri/src/pathfix.rs`): runs once
   at GUI startup (not statusline mode), prepends `/opt/homebrew/bin`,
   `/usr/local/bin`, `~/.bun/bin`, `~/.local/bin` to the process `PATH` when
   they exist on disk. A Finder/Dock launch inherits launchd's bare `PATH`
   (`/usr/bin:/bin:/usr/sbin:/sbin`), which hides an already-installed
   Homebrew/Bun engine — `npm run tauri dev` never surfaced this because it
   inherits the terminal's `PATH`.
2. **`install_bun` made fire-and-forget** (`engine/install.rs`): it used to
   run the Bun installer (`curl -fsSL https://bun.sh/install | bash`, D9)
   synchronously inside the `#[tauri::command]` handler. A plain synchronous
   command runs on the same thread that pumps the webview's event loop — the
   10–60 s blocking child process froze the whole UI (button label, spinner,
   everything), even though the JS had already mutated the DOM moments
   earlier. A classic Tauri footgun, caught live: cosmetic install-progress
   work (staged button text, an amber segment scanner) kept "not appearing"
   in testing because the browser never got a chance to repaint, not because
   the CSS/JS was wrong. Moved the installer to `std::thread::spawn` (same
   "never block the UI" rule already applied to `engine`/`burn`/`sensor`);
   the command now returns almost immediately and the outcome arrives via
   `install-succeeded`/`install-failed` events instead of the invoke's
   return value.
3. **`sensor::install::refresh_if_stale()`**: the statusline binary copy
   (`~/.claude/cc-autobahn/cc-autobahn-statusline`) is only ever written
   once, at consent time (D12/D20) — a copy installed by an old release
   never learns about newer builds on its own, so every subsequent release
   would silently leave `statusLine` pointing at dead code forever. Runs on
   a background thread at every GUI startup: if the sensor is already
   installed and the on-disk copy differs byte-for-byte from the current
   binary, overwrites it in place. Same stable path, no `settings.json`
   write, no re-consent — nothing a user would need to approve twice.

**Reasoning**: all three are Finder-launch / long-lived-install bugs that
dev mode structurally can't reproduce (inherited terminal `PATH`, short
session lifetime, a synchronous command *feels* instant against a warm
cache). Found by simulating a clean machine end-to-end rather than by
inspection — `pathfix::hardened()`'s dedup logic was itself inverted on the
first pass (candidates were filtered out instead of winning the front of
`PATH`) and caught immediately by its own unit test.

**Verified**: `cargo test` 34/34 (3 new `pathfix` tests), `cargo clippy`
clean. End-to-end: isolated instance with `HOME`/`CLAUDE_CONFIG_DIR` pointed
at empty temp dirs, `npx`/`bunx` briefly relocated out of `/opt/homebrew/bin`
(restored after) — confirmed the CHECK ENGINE overlay, the button's staged
progress (now actually visible mid-install), a real isolated Bun install,
engine auto-detection via the hardened `PATH`, and the sensor consent flow
all complete correctly.

## D37 — Redline: the whole instrument reacts to PACE/AUTO, not just the footer

**Decision**: the PACE/AUTO footer (`footer-metric.js`) used to be a flat text
value with no visual weight — it lost against the speedometer's spring (D18) and
the segment bar's glow (D23). New module `src/modules/redline.js` evaluates
PACE and AUTO **both, every render** (not just whichever is currently displayed)
against two thresholds — PACE ≥ +50% over the block average, or AUTO ≤ 15 min
remaining — and, when either is crossed, drives a **sustained** tint
(`.screen.redline`, `.segments.redline`, red breathing box-shadow/segment
color, `@keyframes redlinePulse`) plus a **one-shot** entry effect fired only on
the edge (`wasRedline` tracked module-locally, not per-render): a brightness
flash on `.screen` (`redline-enter`/`redlineFlash`, animates `filter` — kept off
`box-shadow` on purpose, see below), a ripple sweep across the 12 segments
(`.seg.ripple`/`segRipple`, staggered via a per-segment `--i` custom property),
and a mechanical "kick" (`.spike`/`valueSpike`, scaleY + white flash) on both
the footer value and the speedometer readout (`#burn`).

No new DOM, no backend/Rust changes, no new IPC — pure CSS/JS reusing patterns
already in the codebase instead of inventing a new one: the reflow-then-class
one-shot trigger is the same technique `setGear()` already uses for `.gear .g.pulse`
(`trip-computer.js`), and the sustained tint follows the same
`classList.toggle()` shape as the existing 7-day `.screen.warn`.

**`.warn`/`.redline` overlap**: both tint the same `.screen` element's
`box-shadow`. A CSS animation always overrides a static declaration on the same
property regardless of selector specificity or source order — so a plain
`.screen.redline` rule would silently swallow `.warn`'s 7-day reserve tint
whenever both are active simultaneously (a real, not hypothetical, case: heavy
weekly usage and a recent burst pace can coincide). Fixed with a dedicated
higher-specificity `.screen.warn.redline` rule (`redlinePulseWarn` keyframe)
that layers both tints' `box-shadow` values into one animation instead of one
hiding the other.

**Reasoning**: explicit user direction ("que sea interactivo... que pasen
cosas... que quede guau") for the footer to have real presence, escalated
through several rounds to "the whole interface changes" rather than a local
effect — landed on threshold-crossing reactions across screen/segments/footer/
speedometer instead of a literal rotating needle (there is no needle DOM,
D18's "speedometer" is a spring-animated number) or a tray-icon color change
(the tray is a monochrome "template" image, D30 — alpha-only, can't carry a
red tint without breaking macOS's light/dark adaptation; left out of scope for
this pass).

**Verified**: `vite build` clean, no import cycles, no class-name collisions
with existing CSS. No Tauri runtime available in this environment for a full
native run — the threshold logic, edge-detection (no repeated flash while
sustained-critical), and the `.warn`+`.redline` combined keyframe were reviewed
by hand rather than driven end-to-end in a live `tauri dev` session; first run
should confirm the visual effect against real `burn-tick`/`sensor-update` data.

## D38 — Multi-session JSONL tail (concurrent windows)

**Decision**: `burn::tail` tracked exactly one `.jsonl` file — the single
highest-`mtime` one under `~/.claude/projects/**/*.jsonl` (`most_recent_jsonl`).
Replaced with `active_jsonls`, which returns every `.jsonl` written within the
last `ACTIVE_WINDOW_SECS` (60 min), and `TailSet`, a `HashMap<PathBuf, Tail>`
that adds newly-discovered files at EOF (same D8 zero-historical-noise rule,
now per file) and keeps already-discovered files tailed while they still exist.
`Tail` itself lost its `active: Option<PathBuf>` field — the map key already
identifies the path, so `drain`/`pump` take `path: &Path` explicitly. Still
plain `stat`-based polling at the existing 200 ms/5 s cadence (D13/D17/D27) —
no file watcher, no new crate.

**Reasoning**: D17/D27 implicitly assumed a single active Claude Code session.
Running 2+ concurrent sessions (two terminals, common in practice) meant
`most_recent_jsonl` silently dropped every session but whichever wrote most
recently — the needle would jump between sessions and lose turns from the
non-winning one, with no error, no log, nothing in the docs warning it could
happen.

**Consequence**: the 60 min freshness window is a discovery heuristic, not
synced to ccusage's exact 5h billing block boundary — deliberately, since
`burn` has no dependency on `engine` and shouldn't gain one just to read a
block boundary (would break D13's independent-dedicated-threads design). A
session first seen within the window stays tracked even if it goes quiet for
longer than 60 min, so a long-running turn can still emit its later `end_turn`;
old files that were already stale before app startup are ignored until they
write again. `burn-tick` emission is unchanged: each `Tail` still
emits independently, no new payload field to distinguish source file — the
frontend already treats every tick identically (D8's per-response semantics),
and with 2+ sessions closing turns in the same 200 ms window the needle
reflects whichever tick was processed last, acceptable given the needle was
never meant to be an aggregate.

**Verified**: `cargo test` (3 new tests: `active_jsonls_excludes_stale_includes_fresh`
covers the window boundary and the EOF-start rule via `TailSet::rescan`;
`drain_isolates_concurrent_files` proves two files' `pos`/state don't cross-talk
when drained independently; `rescan_keeps_known_stale_file_state` covers the
long quiet turn case), existing `drain_partial_line_not_duplicated` updated for
the explicit-`path` signature, `cargo clippy`/`cargo fmt --check` clean.

## D39 — Tray ring: estimated-vs-official arbitration moves into `tray_icon.rs` (supersedes part of D30)

**Decision**: `tray_icon::set_progress` now takes a `ProgressSource` (`Estimated` |
`Official`) and owns the priority decision itself — once an `Official` write
has landed, later `Estimated` writes are ignored for the rest of the process's
lifetime (sticky, mirrors `everSensorConnected` in `trip-computer.js`, D23).
`engine.rs` and `sensor.rs` call it unconditionally with their own source tag;
neither module needs to know about the other's connection state.

**Supersedes D30**: D30 stated "No estimated-vs-official precedence logic
replicated from the frontend (D23): the tray is a low-commitment glance, the
last data point that arrives from either source wins — a deliberate
simplification, not an oversight." That simplification turned out to be a
real, user-visible bug: with both ccusage and the official statusLine sensor
connected (any Pro/Max user with the sensor installed), `engine`'s 15s poll and
`sensor`'s 2s tail both painted the same ring with different percentage
*meanings* (time-remaining vs quota-remaining), so the ring visibly flickered
between the two every cycle. D30's "last writer wins" was fine when only one
source could ever be active; it stopped being fine once both routinely are.

**Why the arbitration lives in `tray_icon.rs`, not in a caller**: an earlier
version of this fix added a `sensor::is_official_active()` flag that `engine`
checked before writing — functionally correct, but wrong altitude: the actual
contested resource is `tray_icon`'s `Mutex<TrayState>`, written from two
independent threads, so the priority rule belongs there. Splitting it across
two unrelated modules also meant a future third writer of the ring would have
no signal that a priority protocol exists and could reintroduce the flicker
bug by writing unconditionally, the same way `sensor.rs` originally did. It
also created exactly the D38-flagged risk of coupling `engine`'s independent
thread to `sensor`'s connection state (D13's independent-dedicated-threads
design). Moving the rule into `set_progress` removes that coupling entirely:
`engine`/`sensor` are back to knowing nothing about each other.

**Consequence**: the ring's priority is now sticky (matches `#segments`'
`everSensorConnected` semantics) instead of momentary — a sensor blip no
longer hands the ring back to the estimated source and then back again a few
seconds later. The same review also fixed `onSensorUpdate` (`trip-computer.js`)
computing `#segments`' fill from the quota % while the accompanying
"autonomie" text stayed time-based — same class of "two sources disagreeing
under one gauge" bug D23 already fixed once for direction/polarity, this time
for units. Fill now always derives from time-remaining, matching the text; if
`resetsAt` is momentarily absent from a partial statusLine payload, segments
are left as-is rather than forced to an empty tank (mirrors `refreshAutonomie`'s
existing "don't touch it on incomplete data" rule).

**Verified**: `cargo test` (38/38), `cargo clippy` clean; manual check pending
(run the app with both ccusage and the statusLine sensor connected — ring
should hold the official reading with no flicker; disconnect the sensor mid-
session and confirm the ring stays on the last official value instead of
reverting to the time-based one).

## D40 — Both 5h gauges show quota, not time, once the official sensor is connected (supersedes D39's frontend half)

**Decision**: `#segments` and the `#autonomie` text now read the 5h **quota
remaining** (`100 - used_percentage`) in official mode, not time-until-reset.
Time is redundant with the on-screen clock and only ccusage (the estimated
source) actually needs a time-based gauge, since it has no concept of quota
at all. `onSensorUpdate()` (`trip-computer.js`) sets both from `fiveHourPct`;
`refreshAutonomie()` (called by `clock.js` on every tick) re-paints the same
quota text in official mode instead of recomputing a countdown, and only
falls back to the time countdown once `state.everSensorConnected` is false.
`applyEstimated()` (no sensor ever connected) is untouched — it still shows
`EST 3h12`-style time, the correct fallback when quota simply isn't known.

**Supersedes**: D39 fixed segments/text disagreeing by making *both* read
time, on the reasoning that time was the only axis available to both sources
uniformly. That traded a within-gauge contradiction for a cross-gauge one:
the tray ring (fixed in the same D39 entry) reads quota in official mode,
so the panel's segment bar and the tray icon meant different things while
both claimed to represent "the 5h window." Re-litigated here: since quota
only exists with the sensor connected, and time is always available
independent of the sensor, the correct fix is sensor-connected → quota
everywhere (ring + bar + text), no sensor → time everywhere (ring + bar +
text) — never a forced quota value when the sensor never reported one.

**Consequence**: `state.fiveHourPct` is new (`telemetry-state.js`), tracked
separately from `state.fiveHourResetsAtMs` so the clock tick can re-render
quota without needing a fresh sensor payload. `state.fiveHourResetsAtMs` is
now read only by the estimated-fallback path. The partial-payload safeguard
from D39 (don't touch segments/text if the field is missing from this push)
carries over unchanged, just keyed on `fiveHourPct` instead of `resetsAt`.
The `#segments` aria-label and the `.row.gauge` hover hint were reworded from
"5h window autonomy"/"5h billing window remaining" to "5h quota remaining" to
match.

**Verified**: `npm run build` (frontend compiles). Manual check pending (with
the official sensor connected, confirm the segment bar and `#autonomie` text
move with consumed quota, not with wall-clock time, and that they now agree
with the tray ring's meaning; with no sensor, confirm both still show the
`EST …` time fallback unchanged).

## D40-fix — Non-Pro/Max users: gate quota gauges on `everQuotaConnected`, not `everSensorConnected`

**Bug found (post-release, v0.5.1)**: D40 gated the quota gauges on
`state.everSensorConnected`, which only means "the statusLine file has
connected" — it says nothing about whether that file ever actually carried
`rate_limits`. A non-Pro/Max subscriber's Claude Code never emits
`rate_limits` on stdin at all (real, tolerated shape — see
`sensor::mod.rs`'s `tolerates_missing_rate_limits` test), yet the sensor file
still updates on every prompt, so `sensor-update` still fires and
`onSensorUpdate()` still flips `everSensorConnected` to `true` — permanently,
since it's sticky. From that point on, `refreshAutonomie()`'s official
branch ran forever with `state.fiveHourPct` stuck at its `0` default,
painting a fabricated **"100%"** on every clock tick, and `onBlocksUpdate()`'s
`!state.everSensorConnected` gate permanently blocked ccusage's time estimate
from ever serving as the fallback it was supposed to be for these users.

**Fix**: added `state.everQuotaConnected` (`telemetry-state.js`) — set only
inside `onSensorUpdate()`'s existing `if (pctFinite)` branch, i.e. only when a
payload actually carried a usable `fiveHourPct`. All three gates that D40 had
tied to `everSensorConnected` now use `everQuotaConnected` instead:
`refreshAutonomie()`'s quota-vs-time branch, `onBlocksUpdate()`'s ccusage
fallback gate, and `onSensorState()`'s disconnect-freeze check.
`everSensorConnected` itself is untouched and still means what it always
meant (statusLine file connected at least once) — nothing else in the
codebase reads it for gauge purposes anymore.

**Consequence**: a non-Pro/Max user now gets ccusage's `EST 3h12`-style time
estimate as a permanent, live-updating fallback — exactly the "no sensor →
time" rule from D40, correctly extended to "sensor connected but no quota →
still time." A Pro/Max user's behavior is unchanged, since their first
payload does carry `fiveHourPct` and both flags flip true together.

**Verified**: `npm run build` (frontend compiles). Manual check still
pending for both the quota path (Pro/Max, sensor connected) and this fallback
path (simulate a `rate_limits`-less statusLine payload and confirm the
segment bar/text keep tracking ccusage's projection instead of freezing on
"100%").

## D40-toggle — Click the quota gauge to peek at the reset time

**Decision**: D40 demoted reset-time to "redundant with the clock, estimated-
fallback only" — but with the sensor connected the reset time is genuinely
gone from the panel with no way to check it (found in review: the clock
tells you the current time, not how far the 5h window's own boundary is).
Added a click toggle on `.row.gauge` (`toggleAutonomieView()`,
`trip-computer.js`): clicking flips `state.autonomieShowTime` and repaints
both `#segments` and `#autonomie` together from whichever data source is
toggled on. Quota mode and time mode are still never mixed within the row
(D40's core rule) — the toggle swaps the row's whole meaning, not one half
of it.

**Consequence**: `onSensorUpdate()` and `refreshAutonomie()` both now paint
through a shared `paintQuotaGauge()` instead of each hardcoding the quota
render — the toggle state lives in one place and every repaint path (fresh
payload, clock tick, click) reads it consistently. No-op in estimated mode
(`!state.everQuotaConnected`): there's no quota to toggle to, so clicking
does nothing and the row keeps showing ccusage's time projection either way.
`.gauge`'s CSS gained `cursor: pointer`; the hover hint was reworded to "5h
quota remaining — click for reset time" so the affordance is discoverable.

**Verified**: `npm run build` (frontend compiles). Manual check pending
(with the sensor connected, click the gauge row and confirm both the bar and
the text swap to the reset countdown, then back to quota % on a second
click; confirm clicking does nothing in estimated-only mode).

## D41 — Drag-to-move with a persisted override (partially supersedes D24)

**Decision**: D24 dropped manual dragging in favor of the menu-bar model
("no longer a draggable floating window"). The user asked to have both: the
default stays anchored under the tray icon (D24's `position_under_tray`,
untouched), but the panel can also be dragged elsewhere, and that spot is
remembered across opens/restarts instead of the panel snapping back under
the icon every time.

**Implementation**: `data-tauri-drag-region` is back on `<main class="cluster">`
(`index.html`) — the same element it lived on pre-D24 — with the single new
permission `core:window:allow-start-dragging` in `capabilities/default.json`.
This alone turned out to be unusable in practice: `.cluster`'s bezel is
`padding: 2px` (near-zero, edge-to-edge — a later, unrelated D-review pass
after D24's comment was written), so the actual bare/grabbable area is a
2px sliver, effectively unhittable with a mouse.

First fix attempt required holding Option while dragging (to avoid fighting
the instrument's own click handlers) — round-tripped with the user, who
found it too fiddly for something they'll do occasionally without
remembering a modifier. Second attempt dropped the modifier and allowed
click-and-drag anywhere in the window (excluding interactive elements) —
round-tripped again: the user wanted the grab area confined to two
recognizable zones instead of the whole panel, so a click on the dense trip
computer readouts is never ambiguous with a drag attempt.

The shipped version, `src/modules/window-drag.js` (`wireWindowDrag`): a
`mousedown` listener on `document` calls `getCurrentWindow().startDragging()`
only when the target is inside `.row.header` (nameplate/page-label/MFD/PIN
strip) or `.gear` (the PRND selector) — `DRAG_ZONE_SELECTOR`. Within those
two zones it still excludes, by `e.target.closest(...)`, `button` (the
MFD/PIN buttons) and `#nameplate` specifically (click-to-rename, marked
`data-no-drag` — the only non-button click target that lives inside a drag
zone; `#footer-metric` and `.row.gauge`'s click-toggles are both outside the
two zones now, so they don't need the marker). `data-tauri-drag-region` is
left on
`<main class="cluster">` as a harmless no-op along the 2px bezel; the JS
listener is what actually works.

**Resetting the override, and making it instant**: the tray menu's "Reset
position" originally only called `clear_position` — the panel still showed
in the dragged spot until the next open/close cycle, since nothing
re-painted it. `window::reset_position_now` (`window.rs`) fixes that: it
clears the override and, if the panel is currently visible, immediately
re-anchors it under the tray icon using `TrayIcon::rect()` (fetched from
app-managed state — the tray icon itself is `app.manage()`d in `main.rs`
right after `tray::build`). The tray menu item and a new "Reset position"
button on the Settings page (Page 3, next to THEME) both call this same
function — the button through a new `#[tauri::command] reset_position`
(`window.rs`), the same one-command exception to "no window IPC" that
`set_pinned` already established for the PIN button.

`window.rs` gained a `PositionState` (`Arc<Mutex<Option<(f64, f64)>>>`,
`None` = D24 default) persisted to a small JSON file under
`app.path().app_data_dir()` (first use of that Tauri API in this codebase —
justified because, unlike every other user preference (D33, `localStorage`),
window placement is decided in Rust before the frontend exists, per D24).
`position_under_tray`'s own reposition on every tray click also fires
`WindowEvent::Moved`; to keep that from being mistaken for a user drag and
clobbering the override, an `AutoRepositionGuard` timestamp (same idiom as
`tray.rs`'s `REOPEN_GUARD`) is stamped right before every programmatic
`set_top_left` call, and the `Moved` handler in `wire()` ignores events that
land inside that short window. Real drags update `PositionState` in memory
on every `Moved` event and flush to disk on a throttle (150 ms) plus
unconditionally on blur-hide, so the final position always survives even if
a mid-drag write was skipped.

Showing the panel (`tray.rs`'s click handler) now branches: `PositionState`
set → `position_at` (new, clamps the saved point to whichever monitor
currently contains it, in case the screen layout changed since the drag) —
`PositionState` empty → `position_under_tray`, unchanged. A new tray-menu
item, "Reset position", clears the override (memory + deletes the file) so
the next open goes back to anchoring under the icon.

**Consequence**: D24's "no longer draggable" is superseded only for the
drag gesture itself; its actual architecture principle — window
show/hide/position is decided in Rust, not via custom IPC — holds:
`data-tauri-drag-region` is a native webview affordance, not a command the
frontend calls.

**Verified**: `cargo test` 38/38, `cargo clippy` clean.

## D42 — Self-installed `PermissionRequest` hook: approve/deny Claude Code sessions from the cluster

**Decision**: cc-autobahn self-installs, opt-in from Settings, as a Claude
Code `PermissionRequest` hook (matcher `"*"`, all tools). Any Claude Code
session on the machine that hits a permission prompt now surfaces it in the
cluster — tool name, a short input summary, cwd — with Approve/Deny buttons,
instead of the user alt-tabbing to a terminal. Concurrent sessions queue FIFO
(one visible at a time, a "+N more" badge for the rest).

**Why a real socket, not the statusline sensor's file+poll pattern**: a
Claude Code hook is synchronous — Claude Code blocks the whole tool call
until the hook process exits (default 600s timeout, plenty of room for a
human to click a button). The statusLine sensor (D12) is fire-and-forget: a
short-lived CLI process writes a JSON file and exits immediately; the GUI
polls that file later, at its own leisure, with no way to talk back. That's
structurally insufficient here — the hook process must block waiting for an
actual human decision. So this gets a real IPC primitive instead:
`std::os::unix::net::UnixListener`/`UnixStream` (stdlib, zero new deps — D16
spirit), bound at `~/.claude/cc-autobahn/permission.sock`. The GUI runs the
listener on its own dedicated thread (same "one thread per concern" shape as
`engine`/`burn`/`sensor`, D13); each hook invocation is one short-lived
client connection: one JSON request line in, one JSON decision line out.

**Fail-open is the load-bearing safety property**: a dead or never-started
cc-autobahn must never hang a real coding session. The mechanism is
deliberately simple — `hook_bin::run_permission_hook` prints Claude Code's
`hookSpecificOutput` JSON to stdout only if it actually received a decision
over the socket. On ANY failure (no socket file, connection refused, no
response before its own 580s read-timeout, malformed input) it prints
**nothing** and exits 0. That silence isn't a special case we detect and
handle — it's Claude Code's own contract: no valid JSON on a hook's stdout
already means "fall back to the normal terminal prompt." The hook never
invents a decision on its own initiative; it only ever forwards one that
came from a human via the socket. `UnixStream::connect` against a missing or
listener-less path fails near-instantly (`ENOENT`/`ECONNREFUSED`) — there's
no slow-handshake failure mode to guard against here like there would be for
a network socket, so no manual connect-timeout wrapper was needed.

**Settings.json merge — an array, not an object, and no "previous value" to
chain**: `hooks.PermissionRequest` is a list of matcher-groups (other tools,
or other cc-autobahn features under other event types, may already have
entries there), unlike `statusLine`'s single object. `apply_install`
appends a new `{"matcher":"*","hooks":[...]}` entry, or — idempotently, on
reinstall — replaces its own entry in place by matching on the stable binary
path in the `command` string, never touching any other entry. Unlike
`statusLine` (D12), uninstall doesn't need a "previous value" chain: there's
nothing to restore, `apply_uninstall` just removes the one matcher-group we
added and cleans up an empty `PermissionRequest`/`hooks` key rather than
leaving `[]`/`{}` litter. Same backup/atomic-write/rollback mechanics as
`sensor::install` (0600 non-overwriting `settings.json.cc-autobahn.bak`,
`serde_json::Value` round-trip — never a typed struct, Claude Code validates
settings.json with a strict Zod schema and one bad field bricks it — atomic
tmp+rename, post-write re-parse with rollback on failure).

**Binary copy**: a second stable path, `~/.claude/cc-autobahn/cc-autobahn-permission-hook`
(separate from the statusline sensor's own stable copy), for the same reason
as D12: an unnotarized macOS `.app` runs from an ephemeral Gatekeeper-
translocation path, so `current_exe()` can't be written into settings.json
directly. Kept as its own copy rather than unifying with the statusline
binary — smaller diff against existing `sensor::install` code, at the cost
of two `refresh_if_stale` self-heals instead of one; revisit if a third
hook-style feature makes the duplication worth collapsing.

**Tray badge**: `TrayState` gained its own `pending_permission` bool
alongside the existing `alert` (D37/D-review), rather than reusing `alert`
for both — the two urgencies mean different things at a glance (PACE/AUTO
budget pressure vs. "a session is blocked waiting on you"), even though both
drive the same blink thread and painting-precedence-over-progress behavior.
`permission::mod.rs` calls `tray_icon::set_permission_pending` directly
(backend already knows the queue state authoritatively), unlike
`set_tray_alert`, which is IPC because `redline.js` owns that threshold
logic on the frontend.

**UI shape**: modeled on `redline.js` (sustained state + one-shot pulse on
the rising edge) rather than `engine-overlay.js`'s full-panel takeover — a
pending permission is transient and should coexist with normal use, not
block the whole display. Reuses the `.sensor-overlay`/`.sensor-card`/
`.sensor-btn` visual language for both the live gate (`#permission-gate`,
highest z-index of any overlay) and the consent flow
(`#permission-consent-overlay`), with the theme's `--amber-glow` accent (not
a hardcoded color) so an actionable request doesn't read as a warning while
still following the user's chosen theme like every other element. The
consent overlay is opt-in only,
opened from a new "PERMISSION HOOK" row on the Settings page (Page 3) —
unlike the statusline sensor's overlay, it is never auto-shown at startup,
since the hook is an optional extra capability the app's core value doesn't
depend on.

**Scope deliberately kept small**: the queue is a plain FIFO
(`VecDeque<PendingSlot>`) with only one visible request at a time — no full
multi-request UI, no "always allow for this session" memory, no `ask`/`defer`
decisions (only binary allow/deny, since a human always resolves to one or
the other). Add if a real need shows up; nothing here forecloses it.

**Hardening from an 8-angle code review, applied before shipping**: (1) the
panel didn't auto-show when a request arrived while hidden (the common
case — hide-on-blur means it usually is) — a request would only ever surface
as a blinking tray icon; `window::show_for_permission` now shows/focuses the
panel on a new arrival, same positioning logic as the tray's click handler.
(2) `hook_bin::print_decision` used `println!`, which panics on a stdout
write failure (EPIPE if Claude Code already gave up reading) — right at the
one moment a real decision was about to be delivered, contradicting the
module's own "never panics" contract; switched to `writeln!`, which returns
a `Result` instead. (3) the socket read in `handle_connection` had neither a
timeout nor a size cap — a stalled or wrong-protocol connection could pin an
OS thread forever or grow memory unbounded; added a 10s read/write timeout
and a 1MB request cap (4KB on the hook's own response read, which is always
a few bytes). (4) the accept loop discarded `accept()` errors via
`.flatten()` with no backoff, which would hot-spin at 100% CPU under
persistent OS-level failure; now sleeps 200ms on error. (5) a narrow
race existed where `permission_approve`/`_deny` could remove and resolve a
request at the exact instant `handle_connection`'s own queue timeout fired,
silently losing a real human decision (the frontend would report success
while the hook still timed out and failed open) — closed with a `try_recv()`
fallback that picks up an in-flight decision instead of discarding it. (6)
`chmod_755`/`copy_private`/`same_contents`/`write_settings_atomic`/
`read_settings_value`/`settings_path` were byte-for-byte duplicated between
`sensor::install` and this module (3 independent review angles converged on
the same finding) — hoisted into `sensor/mod.rs` alongside the already-shared
`claude_config_dir`/`write_private`, both installers now import them.

One finding remains a documented tradeoff: no app-level single-instance lock
exists anywhere in cc-autobahn. The permission listener now probes an
existing socket and leaves a live owner untouched (replacing only a stale Unix
socket), so a second launch can no longer steal all new permission requests;
however, requests still belong to the first running UI. A full app-wide
single-instance policy remains separate scope. `prompt_id` is genuinely absent from Claude
Code's hook payload before the first user prompt in a session (confirmed
against the official hooks doc, not the review's own — since it's a real
field/event, unlike what one reviewer initially claimed) — the one hook
invocation that could hit this fails open exactly like a closed GUI would,
which is already the correct, safe default.

**2026-07-19 notification review**: serialized queue snapshot/emission side
effects so an older `resolved` event cannot overtake a newer `pending` event;
made socket mode 0600 mandatory; protected live sockets and unexpected regular
files; restored frontend retry state after failed IPC; and delayed Bash's
in-memory Always Allow entry until its settings write succeeds. Verified with
59/59 Rust tests (including live/stale/non-socket ownership), clean Clippy,
and a production Vite build.

## D43 — Native NSPanel over a fullscreen app's Space

**Bug**: the panel doesn't show up while another app is in fullscreen. On
macOS a fullscreen app runs in its own dedicated Mission Control Space; a
normal `NSWindow` only ever renders in the Space it was created on.
`alwaysOnTop:true` in `tauri.conf.json` only raises the window's *level*
(`NSFloatingWindowLevel`) — it says nothing about which Space the window is
allowed to appear in.

**Historical investigation** — several flag/level-only attempts were made
and disproven on the user's actual machine (a
2-monitor Sonoma+ setup), each confirmed with on-device debug logging, not
just a clean compile:

1. `collectionBehavior = CanJoinAllSpaces | FullScreenAuxiliary` +
   `NSStatusWindowLevel` (25) — didn't work.
2. `CanJoinAllSpaces` alone + `NSScreenSaverWindowLevel` (1000) — didn't
   work (dropping `FullScreenAuxiliary` was itself a wrong reading of
   Apple's docs — real working menu-bar panels, e.g. Ardent Swift's hotkey
   panel and the open-source Helium app, combine both flags).
3. Both flags + `NSScreenSaverWindowLevel` (1000) — didn't work.
4. Both flags + `CGWindowLevelForKey(.maximumWindow)` (2147483631, verified
   via `swift -e 'print(CGWindowLevelForKey(.maximumWindow))'`) — didn't
   work either, despite a **standalone, dependency-free Swift/AppKit repro
   with the exact same collectionBehavior/level/styleMask reliably working**
   on the same machine, same macOS version, same monitor. Forcing the
   window's `styleMask` to a bare `Borderless` (tao's borderless+
   non-resizable branch otherwise leaves stray `Miniaturizable` +
   `FullSizeContentView` bits set) didn't close the gap either.

**Root cause, as far as it was narrowed down**: `NSWindow.isOnActiveSpace`
reads back `false` for the real app's window in every attempt above, even
though every inspectable property (`collectionBehavior`, `level`,
`styleMask`, `isVisible`, on-screen position) matched the standalone repro
exactly. Explicitly calling `NSApplication.activate()` from inside the
tray's click handler — itself a genuine, synchronous AppKit `mouseDown:`
callback on the real main thread (`tray-icon` crate uses a custom
`NSStatusItem` view with real `mouseDown:`/`mouseUp:`, not an indirect/
queued dispatch — confirmed by reading its source) — still left
`NSApp.isActive` reading `false` even after polling with the run loop
pumped for 200ms. Whatever is preventing this specific accessory app from
becoming the active app during another app's native fullscreen wasn't
identified; it may be a real, mostly-undocumented macOS restriction, or
something specific to how Tauri/tao/WKWebView's window differs from a bare
`NSWindow`. Every found *working* third-party example used `NSPanel` with
`isFloatingPanel = true`; that observation led to the resolution below.

**Resolution (2026-07-19)**: the missing distinction really was the runtime
window class. `window::wire` now swizzles tao's `TaoWindow` allocation into an
ivar-free `NSPanel` subclass with `canBecomeKeyWindow = YES`, then configures:

- `NonactivatingPanel` style, so the fullscreen app stays frontmost;
- `isFloatingPanel = true` and `hidesOnDeactivate = false`;
- `CanJoinAllSpaces | FullScreenAuxiliary` collection behavior;
- `NSScreenSaverWindowLevel` (1000), high enough for native fullscreen but
  not the dangerous `maximumWindow` level from the reverted experiment.

Both tray clicks and permission-request auto-open use native
`orderFrontRegardless` + `makeKeyWindow`. The latter is dispatched onto the
AppKit main thread: the first end-to-end attempt exposed that the permission
socket calls `show_for_permission` from a worker thread, where direct AppKit
window ordering aborts the process.

**Verified on-device**: a temporary independent AppKit process entered native
fullscreen, then the real permission-hook path opened cc-autobahn. The hook
remained blocked awaiting a decision and `CGWindowListCopyWindowInfo` with
`optionOnScreenOnly` reported the cc-autobahn window in that active fullscreen
Space at layer 1000 with its expected 550x150 bounds. `cargo test` 56/56 and
`cargo clippy --all-targets -- -D warnings` clean.

## D44 — Permission identity before provider expansion

**Problem**: Claude's `prompt_id` correlates permission requests to a user
prompt; it is optional and multiple tool approvals may share it. Using it as
the queue key made independent requests collide and could remove or resolve the
wrong entry. Codex has different native identifiers, so this could not remain
the routing foundation for a dual-provider queue.

**Decision**: every Claude hook invocation now generates a UUID v4
`request_id`. The optional Claude `prompt_id` remains metadata only. Queue
lookup, timeout cleanup and frontend commands use `request_id`; duplicate IDs
are rejected without replacing the original entry. Provider becomes the next
routing discriminator when Codex requests enter the same queue.

**Native Always Allow**: current Claude `permission_suggestions` travel through
the socket as opaque JSON. When exactly one suggestion exists, the hook echoes
it unchanged as `decision.updatedPermissions`, matching Claude's official
contract. The existing exact-rule/session-memory implementation remains only
for older payloads with no native suggestion. Multiple suggestions are not
guessed: the current single-action UI hides Always Allow until it can expose an
explicit choice.

**Failure semantics**: malformed input, duplicate identity, persistence failure
or disconnected response channel produces no hook decision. Claude keeps its
native permission UI. Fallback persistence, queue removal and response enqueue
are serialized under the queue mutex so the timeout worker cannot observe an
absent slot before the decision exists; a failed channel send is an error, not
success. The response cap increased from 4 KiB to the 1 MiB request limit plus
envelope headroom because a native suggestion may be large.

**Verification**: 68/68 Rust tests, including same-prompt independence,
duplicate rejection, optional `prompt_id`, exact native suggestion round-trip,
payloads above 4 KiB, persistence failure, disconnected channel and a
deterministic timeout/decision ordering test. Rustfmt, strict Clippy and Vite
production build pass. A separate adversarial reviewer found four races/wire
defects in the first implementation; all were fixed and the final directed
review reported no remaining finding.

## D45 — Provider-discriminated foundation before Codex transport

**Boundary**: provider adapters own external wire formats and identity. Shared
code consumes normalized contracts carrying `provider`; current Claude
renderers explicitly reject Codex, unknown and missing discriminants. External
`ccusage` JSON cannot choose its provider label: Claude DTO fields skip provider
deserialization and the adapter assigns Claude.

**State model**: frontend state is now `global` chassis state plus independent
`providers.claude` and `providers.codex` objects. Existing renderers bind to the
Claude object until provider-scoped DOM lands. Mutable health and rate buffers
are never shared. MFD page and permission head live under global state.

**Health and startup**: provider health is keyed by provider + component,
stored authoritatively in Rust and exposed through both `provider-health`
events and `provider_health_snapshot`. Backend timestamps are monotonic per
component even if wall clock moves backwards; frontend rejects older replay.
Engine recovery emits `connected` after a degraded poll succeeds. Sensor
payloads expose camelCase `observedAtMs`; `sensor_snapshot` hydrates state lost
before webview listeners exist. Live listeners register before snapshots, and
an equal/older snapshot cannot overwrite a live event. Application-wide binary
events use explicit `app-engine-*` names and are not provider events.

**Testing and CI**: Node's built-in runner covers isolated buffers, strict
routing, health hydration, replay/equal-timestamp rejection and global chassis
updates. CI runs it on direct pushes to `main`/`develop` and on pull requests.

**Verification**: 77/77 Rust tests and 9/9 frontend tests pass, plus Vite
production build, Rustfmt, strict Clippy and `git diff --check`. A fresh
adversarial reviewer found startup hydration, recovery, spoofing, replay,
event-naming and camelCase defects across three passes; all were fixed and the
final review reported no remaining concrete bug. Codex transport and
provider-scoped UI remain separate later cuts.

## D46 — Bounded Codex rollout telemetry before App Server

**Source boundary**: Codex live telemetry comes from a dedicated local rollout
adapter, not branches in Claude's parser. `CODEX_HOME` may contain one path,
comma-separated homes, or direct JSONL directories; absent configuration falls
back to `~/.codex`. Home roots expand to `sessions/` and
`archived_sessions/`, then recurse without following symlinks.

**Decoder contract**: only `session_meta.id`, `task_started`,
`turn_context.model`, and `token_count.info.last_token_usage.output_tokens`
have meaning. Each rollout owns its decoder, response clock, cumulative-token
dedupe and read offset. A response rate is `last output tokens / elapsed since
the previous response (or task start)`. Unknown, null, malformed,
non-monotonic, zero-token and duplicate records emit nothing. Discovery depth,
active file count, line size, bootstrap window and bytes per pump are bounded.

**Startup and honesty**: new tails bootstrap thread/model/token baselines from
bounded head/tail reads at EOF, so startup never replays historical speed.
Model activity is stored in Rust and exposed through both a live event and a
snapshot, closing the pre-WebView subscription race. Codex is marked available
only while a recent local rollout exists. History/model-cost breakdown remains
explicitly unavailable until `ccusage codex daily/session` lands; local rollout
availability is not presented as an official quota or billing source.

**Verification**: anonymized decoder/tail/discovery fixtures, malformed and
duplicate records, normalized frontend routing, provider buffer isolation, and
a privacy-preserving bootstrap against a real local rollout pass. Full baseline:
87 Rust tests, 28 frontend tests, Rustfmt, strict Clippy, Vite production build,
and whitespace validation.

## D47 — Codex history never allocates aggregate cost heuristically

**Observed wire contract**: `ccusage 20.0.17 codex daily/session` returns
aggregate `costUSD` and a model-keyed token map. It does not return Claude's
`modelBreakdowns[].cost`. Both commands support `--speed auto`, which reads
the relevant Codex `config.toml` service tier.

**Normalization**: `history_daily` now requires a provider and
`history_sessions` exposes the parallel normalized contract. Claude and Codex
wire DTOs remain separate; normalized entries carry provider and estimated
source quality. Codex model maps are sorted deterministically. Aggregate cost
is assigned to a model only when exactly one model exists; with multiple
models, per-model cost is `null` and the UI renders `—`. Aggregate Codex costs
render as `EST`. Session file and project directory fields are never serialized
across IPC.

**Caching and availability**: daily/session caches and pending promises are
keyed by report kind + provider, so History and Limits share one spawn without
cross-provider reuse. History can load without a currently fresh rollout;
successful history health is itself a valid Codex data-source capability.
Today's model list selects the actual local date instead of relabeling the most
recent stale usage as today.

**Verification**: real local CLI schemas were inspected without printing
conversation contents or paths. Fixtures cover both providers, sole/multiple
model cost semantics, privacy, spoof rejection, malformed input, cache
isolation, in-flight coalescing, retry and stale-day handling. Full baseline:
89 Rust tests, 36 frontend tests, Rustfmt, strict Clippy, Vite production build
and whitespace validation.

## D48 — Official Codex account data stays behind one stable App Server adapter

**Lifecycle and version boundary**: cc-autobahn resolves the Codex executable
from its hardened application PATH, reports that exact runtime's version and
owns one `codex app-server --stdio` child. Initialization opts out of
experimental APIs. Stable account methods are capability-probed at runtime;
older binaries and unsupported authentication modes degrade only App Server
health. Reconnect is sequential with bounded exponential backoff, so duplicate
children cannot accumulate. Tauri exit events explicitly stop and reap the
active child before the process exits.

**Wire and normalization contract**: newline JSON-RPC messages are bounded to
1 MiB and correlated by request ID. `account/rateLimits/read` seeds a full
snapshot; `account/rateLimits/updated` is merged recursively while ignoring
missing/null fields. Every `rateLimitsByLimitId` bucket crosses IPC in sorted
form, while the `codex` bucket deterministically owns the primary/secondary
panel gauges. Epoch seconds become milliseconds once in Rust. Account usage
keeps summary and daily token buckets but never invents an official USD cost.

**Freshness and fallback**: Rust stores the latest rate-limit/account-usage
snapshots to close the pre-WebView subscription race. Events carry
`official`/`stale`/`unavailable` quality without rewriting the original
observation time; App Server health is independent from rollout and
history health, so failure cannot erase cross-surface local telemetry or
estimated history. A real redacted contract probe against `codex-cli 0.144.6`
confirmed initialize, rate limits and account usage.

**Verification**: sparse merge, null preservation, deterministic multi-bucket
selection, account usage and input-size bounds have Rust fixtures. Full gate:
98 Rust tests, 40 frontend tests, Rustfmt, strict Clippy, Vite production build
and whitespace validation.
