# Visual design — Mercedes W203

## Reference

The **Mercedes W203 Kombiinstrument** (2000–2007): central **dot-matrix VFD**
display, monochrome **amber/orange** on black. These are NOT analog
needles — it's text and glowing segment bars. Reference elements:

- "AFTER START" trip computer: `46 Km`, `0:40 h`, `67 Km/h`, `6.4 L/100Km`,
  large speed reading bottom-left, clock bottom-right.
- Amber odometer: total km, trip odometer, temperature, time.
- Coolant gauge: **horizontal segment bar** `40 · 80 · 120 °C`.
- Automatic gear selector **P R N D** on the side bezel, active gear
  lit up.

## Car-to-tokens mapping

| W203 Element                | cc-autobahn Metric                               |
| ---------------------------- | ------------------------------------------------ |
| Speedometer (Km/h)          | `tok/s` **per response** (`Δoutput / Δt_turn`, D8) |
| Fuel consumption (L/100 Km) | Average cost `$/Mtok`                            |
| Range / fuel tank ⛽        | 5-hour window remaining (segment bar)            |
| "AFTER START" trip           | Tokens and time since the last reset             |
| Odometer                    | Total accumulated tokens                          |
| PRND selector                | **Active model** (O/S/H/F), lit up               |
| Clock                        | Real time                                         |
| Coolant bar                  | Weekly (7d) rate-limit window, Page 2 (D33)        |
| Trip-computer stalk button  | MFD page cycle: trip / history / limits / settings (D33) |

Kickdown (effort level as small bars) was implemented and later **removed**
(D29) — it added no visual value once tried live. Don't re-add it without a
new decision recorded in `docs/DECISIONS.md`.

## Model selector (PRND reinterpretation)

The automatic transmission's PRND indicates the lit-up **active gear**. We
indicate the **model in use**, by its initial:

```
┌─┐
│O│  Opus     ← active: bright amber + glow
│S│  Sonnet   ← inactive: dim amber
│H│  Haiku
│F│  Fable
└─┘
```

- Active model = `--amber-glow` at full brightness.
- The rest = `--amber-dim`.
- Data source: `model.id` from the statusline JSON / ccusage.
- **Effort** below, like kickdown: `▪▪▪▪` (max = pedal to the floor).

## Cluster layout

```
┌─────────────────────────────────────┐
│  CC 320    hover hint text   ▸ PIN  ┌─┐│
│                                     │O││
│   1.24M tok      0:40 h             │S││
│                                     │H││
│   4.1k tok/s    $0.42/Mtok          │F││
│  106 tok/s ················· 16:57  └─┘│
├─────────────────────────────────────┤
│ ⛽ ▐███████░░░░░  3h12         62%  │
└─────────────────────────────────────┘
```

**4-page MFD (D33)**, cycled by the `▸` button next to PIN, same UX as the
real trip-computer stalk button — no page beyond Page 0 crowds this layout
further; each is its own screen:

1. Trip computer (above, unchanged since D8/D18/D23).
2. History — 30-day cost sparkline + fixed detail readout (not a floating
   tooltip, this window is 440-550×150, too short/narrow for one to not
   clip an edge).
3. Limits — weekly (7d) rate-limit bar, today's cost per model, instant vs.
   average burn rate.
4. Settings — default page + which pages are in the cycle.

A docked "header-hint" line between the nameplate and the PIN/MFD buttons
shows a one-line description of whatever's under the cursor, replacing
every native `title=` tooltip (dark-gray OS chrome, breaks the amber skin
with no CSS-reachable fix).

## Palette

| Variable       | Value      | Use                                   |
| -------------- | ---------- | -------------------------------------- |
| `--amber`      | `#ff9a1f`  | primary amber                          |
| `--amber-glow` | `#ffb347`  | highlight / large digits / active      |
| `--amber-dim`  | `#7a3d08`  | unlit segments / inactive model        |
| `--bg`         | `#0a0705`  | display glass                          |
| `--bezel`      | `#17120d`  | surrounding frame                      |

## Style details (VFD effect)

- Near-black background with a soft radial gradient (top glow).
- Amber `text-shadow` to simulate phosphor emission.
- **Scanlines**: two `repeating-linear-gradient`s (0deg + 90deg, crosshatch)
  + `mix-blend-mode: multiply`, `z-index` above every page/popup (including
  the custom dropdown) so nothing reads as floating outside the screen effect.
- `font-variant-numeric: tabular-nums` → digits that don't jitter.
- Wide `letter-spacing`, uppercase labels.
- Segment bar: `.seg` / `.seg.on` divs, 2 px gap (segmented look) — reused
  for the checkbox (Page 3): a lit/unlit block, not a native checkmark icon.
- **No native form chrome** (D33): custom checkbox (`.vfd-check`, no browser
  default), custom dropdown (`.vfd-dropdown`, a button + list — a native
  `<select>`'s popup is unstyleable OS chrome in WKWebView), no `title=`
  tooltips anywhere (see header-hint above).

## Done

- Needle/speedometer easing curve: damped spring with overshoot
  (D18), not a linear interpolation.
- 4-page MFD cycle, docked header-hint, custom checkbox/dropdown (D33).

## Parked ideas (outside the active roadmap, see `docs/ROADMAP.md`)

- **Real dot-matrix font** (currently: system monospace + glow). Candidate:
  a 5×7 dot font embedded as a local woff2 (no CDN, offline).
- Red zone at the top of the speedometer for high burn rate.
- Compact mode (speedometer + range only) for a narrow bar.

These were dropped from the Phase 5 checklist without a documented decision
(ADR) as to why — if revisited, record the reason in `docs/DECISIONS.md`
before writing code.
