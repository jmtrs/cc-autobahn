# Data engine

cc-autobahn does not calculate usage: it **reads** it from three official/standard sources.
All the raw data already exists; we just present it.

## Source 1 — ccusage (main engine)

[`ccusage`](https://ccusage.com) (ryoppippi) — the most widely used and stable
Claude Code usage analysis tool. It reads `~/.claude/projects/**/*.jsonl` and resolves
pricing (LiteLLM), deduplication of the 5h block shared across sessions, and the
Opus multiplier. **We do not fork it: we consume its `--json` output.**

Reference version: **20.0.17**.

### Commands

```bash
ccusage blocks --active --json         # active 5h block: burnRate, projection, start/end
ccusage claude daily --json --since …  # daily history (History/Limits pages, D33)
ccusage session --json                 # per session (not used yet)
```

**Scoping matters**: cc-autobahn's History page calls `ccusage claude daily`,
not the bare `ccusage daily`. The top-level command aggregates every agent
ccusage detects on the machine (Codex, Gemini, etc.) if the user has those
CLIs installed too — confirmed by running both against real data. The
`claude` subcommand scopes to Claude Code only, which is the only thing
this project's numbers should ever reflect.

### Fields per object

`inputTokens`, `outputTokens`, `cacheCreationTokens`, `cacheReadTokens`,
`totalTokens`, `totalCost`. `blocks` also include burn rate (tokens/min)
and a projection for the end of the 5h window.

### Important note

`ccusage blocks --live` (live dashboard) was **removed in v18.0.0**. That's why
cc-autobahn does its own polling of `--json` + JSONL tail, instead of relying
on a live mode that no longer exists.

## Source 2 — JSONL tail (tok/s per response)

ccusage gives burn rate per **minute** (smoothed average). We give `tok/s`
**per response**, by following the active session log at
`~/.claude/projects/**/*.jsonl`: once a turn completes, `Δoutput / Δt_turn` →
`tok/s`. The needle **jumps on completion** and decays with a spring back to idle.

**Physical limit (validated 2026-07-16).** The JSONL `usage` field is **not
streamed**: it is stamped **identically** on every line of a turn and
only appears **when the turn ends** (measured: a turn with 3008 output tokens
lands all at once after 36 s of silence). The log **never sees the in-progress turn**. That's
why an "instantaneous, reacts-as-you-type" needle is **impossible** from this source — the
honest approach is the per-response average as a step function. It's still the differentiator
(no competitor shows it), but under its true label (see D8/D11). Real
streaming would need Source 4 (OTEL) — not planned, see below.

## Source 3 — Claude Code statusline JSON (self-installing sensor)

Claude Code passes, via stdin to a **configured statusline script**, a JSON with
**official** account data. It is **push**: an external window does not receive it on its own.

**Wiring (see D12).** cc-autobahn **is** that script and installs itself: on first
launch it writes the `statusLine` key to `~/.claude/settings.json` (with
consent, backup, and rollback). On each invocation its binary (1) emits the normal
statusline line to stdout (preserving the previous one or falling back to a default) and (2) dumps
the full JSON to a socket/file that the window tails. This way the official data arrives
**without the user editing anything by hand**. Relevant fields:

```json
{
  "model": { "id": "claude-opus-4-8", "display_name": "Opus" },
  "cost": { "total_cost_usd": 0.01234, "total_duration_ms": 45000 },
  "context_window": {
    "current_usage": {
      "input_tokens": 8500,
      "output_tokens": 1200,
      "cache_creation_input_tokens": 5000,
      "cache_read_input_tokens": 2000
    },
    "used_percentage": 8
  },
  "rate_limits": {
    "five_hour": { "used_percentage": 23.5, "resets_at": 1738425600 },
    "seven_day": { "used_percentage": 41.2, "resets_at": 1738857600 }
  },
  "effort": { "level": "high" },
  "exceeds_200k_tokens": false
}
```

- `rate_limits.five_hour` → **range/autonomy** (official data, better than the estimate).
- `rate_limits.seven_day` → **weekly needle** (ccusage doesn't frame it this way).
- `model.id` → PRND selector. `effort.level` → kickdown.
- `cost.total_cost_usd` → session price.

Docs: <https://code.claude.com/docs/en/statusline>

## Source 4 (optional) — OpenTelemetry

For advanced historical dashboards, Claude Code emits OTEL metrics:

```bash
export CLAUDE_CODE_ENABLE_TELEMETRY=1
export OTEL_METRICS_EXPORTER=prometheus   # or otlp / console
```

- `claude_code.token.usage` — attributes: `type` (`input`/`output`/`cacheRead`/
  `cacheCreation`), `model`, `query_source`, `speed`, `effort`, `agent.name`…
- `claude_code.cost.usage` — cost in USD.

With Prometheus+Grafana, `rate()` over `claude_code.token.usage` = real tok/s.
**Deliberately not implemented** (D-review, decided when scoping Phase 6):
adds a whole new dependency (an OTEL collector) for a real-time need that
D8's per-response `tok/s` already covers honestly. Not on the roadmap; would
need a fresh decision recorded in `docs/DECISIONS.md` to revisit.

Docs: <https://code.claude.com/docs/en/monitoring-usage>

## Cadences (D13 — not a single poll)

| Sensor | Cadence | Why |
| ------ | -------- | ------- |
| `ccusage blocks` | **10–30 s** (or a persistent process) | the 5h block doesn't change every second; running `npx` every 1–2 s is wasteful |
| JSONL tail (`tok/s`) | **event-driven** (when the log is written) | no polling |
| Statusline (`rate_limits`, model) | **push** | arrives whenever Claude Code renders |
| `ccusage claude daily` (History/Limits) | **on-demand** (D33) | daily totals barely move within a day; fetched only when that MFD page opens, cached client-side ~5 min |

## Zero-friction strategy (the app wires itself up, D9)

Two self-installing wires. The user only gives **one consent**.

**Wire A — data engine** (`engine::detect`):

1. **Global `ccusage`** on PATH → use it directly.
2. **Node** present → `npx -y ccusage@latest blocks --json` (nothing to install).
3. **Bun** present → `bunx ccusage blocks --json`.
4. **No runtime** → amber "CHECK ENGINE" overlay with a single button
   **"Install engine"** (`engine::install::install_bun`, Phase 4/D9): runs the
   official Bun installer, updates the `PATH` of the already-running process (the installer
   only adds it to the shell rc) and relaunches the engine without restarting the app.
   macOS/Linux for now.
5. **(Optional, not started)**: bundle Bun as a Tauri *sidecar* → 0 network
   required, at the cost of +30-90 MB per platform in the final binary.

**Wire B — statusline sensor** (D12): the app writes the `statusLine` key to
`~/.claude/settings.json` (backup + rollback) pointing at its own binary, which
dumps the official JSON to a socket that the window tails. No manual editing.

## Accuracy / honesty

- With a **subscription** (Pro/Max), the USD price is **estimated** (ccusage calculates it
  from public pricing). Range/autonomy (`rate_limits`) is genuinely official data.
- With the **API**, the cost is exact. Official billing is always the Claude Console.

## Comparison of existing tools (why ccusage)

| Tool                            | inst. tok/s | in/out/cache | cost+proj. | 5h+ETA | weekly | history | car aesthetic |
| ------------------------------ | :---------: | :----------: | :---------: | :----: | :-----: | :-------: | :------------: |
| **ccusage**                    |     ❌      |      ✅      |     ✅      |   ✅   |   ❌    |    ✅     |       ❌       |
| Claude-Code-Usage-Monitor      |  ❌ (/min)  |      ✅      |     ✅      |   ✅   |   ❌    |    ✅     |       ❌       |
| par-cc-usage                   |  ❌ (/min)  |      ✅      |     ✅      |   ✅   |   ❌    |    ✅     |       ❌       |
| ccburn / codeburn              |  ❌ (/min)  |      ✅      |     ~       |   ✅   |   ❌    |    ~      |       ❌       |
| **cc-autobahn** (this project) |   ✅ new    |      ✅      |     ✅      |   ✅   | ✅ new  |    ✅     |    ✅ unique   |

ccusage wins as the **engine** (de facto standard, more stable, clean JSON).
cc-autobahn contributes what's missing: `tok/s` **per response**, a weekly needle, and the
W203 skin. (The table's "inst. tok/s" column should be read as "per response": see D8 — the
JSONL doesn't allow true instant readings.)
