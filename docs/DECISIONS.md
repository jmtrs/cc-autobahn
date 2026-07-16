# Decision log (ADR)

Decisions made during design, with their reasoning. Lightweight format.

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
  process doesn't re-read — it's prepended by hand with
  `std::env::set_var("PATH", ...)` after installing, so that `detect()` and the
  subsequent `Command`s find `bunx` without requiring an app restart (D9:
  true zero friction). If the engine appears, `engine::start` is relaunched.

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
