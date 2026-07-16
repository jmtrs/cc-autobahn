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
| Speedometer (Km/h)          | `tok/s` **per response** (`Δoutput / Δt_turno`, D8) |
| Fuel consumption (L/100 Km) | Average cost `$/Mtok`                            |
| Range / fuel tank ⛽        | 5-hour window remaining (segment bar)            |
| "AFTER START" trip           | Tokens and time since the last reset             |
| Odometer                    | Total accumulated tokens                          |
| PRND selector                | **Active model** (O/S/H/F), lit up               |
| Kickdown (throttle kick)    | Effort level (low/med/high/max)                   |
| Clock                        | Real time                                         |
| Coolant bar                  | Weekly window (7 days) — secondary variant        |

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
│        AFTER START            ┌─┐   │
│                               │O│   │
│   1.24M tok      0:40 h        │S│   │
│                               │H│   │
│   4.1k tok/s    $0.42/Mtok    │F│   │
│  106 tok/s ················· 16:57  │
├─────────────────────────────────────┤
│ ⛽ ▐███████░░░░░  3h12         62%  │
└─────────────────────────────────────┘
```

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
- **Scanlines**: `repeating-linear-gradient` + `mix-blend-mode: multiply`.
- `font-variant-numeric: tabular-nums` → digits that don't jitter.
- Wide `letter-spacing`, uppercase labels.
- Segment bar: `.seg` / `.seg.on` divs, 2 px gap (segmented look).

## Done

- Needle/speedometer easing curve: damped spring with overshoot
  (D18), not a linear interpolation.

## Parked ideas (outside the active roadmap, see `docs/ROADMAP.md`)

- **Real dot-matrix font** (currently: system monospace + glow). Candidate:
  a 5×7 dot font embedded as a local woff2 (no CDN, offline).
- Red zone at the top of the speedometer for high burn rate.
- Compact mode (speedometer + range only) for a narrow bar.

These were dropped from the Phase 5 checklist without a documented decision
(ADR) as to why — if revisited, record the reason in `docs/DECISIONS.md`
before writing code.
