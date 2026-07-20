# Data engine

cc-autobahn does not calculate usage: it **reads** it from four official/standard sources.
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
ccusage claude session --json          # normalized Claude session report
ccusage codex daily --json --speed auto   # estimated Codex daily cost/tokens
ccusage codex session --json --speed auto # normalized Codex thread/session report
```

**Scoping matters**: cc-autobahn's History page calls `ccusage claude daily`,
not the bare `ccusage daily`. The top-level command aggregates every agent
ccusage detects on the machine (Codex, Gemini, etc.) if the user has those
CLIs installed too — confirmed by running both against real data. The
Provider subcommands keep Claude and Codex reports isolated. Codex model maps
contain per-model tokens but only aggregate `costUSD`; cc-autobahn leaves
multi-model cost allocation unknown rather than distributing it heuristically.

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
**per response**, by following fresh session logs concurrently under
`~/.claude/projects/**/*.jsonl`. Eligible intermediate assistant/tool-use
writes can produce a partial tick; final turn closure produces the completed
`Δoutput / Δt_turn` tick. The needle jumps on writes and decays between them.

**Physical limit (validated 2026-07-16).** JSONL is not a token stream. Some
responses arrive only as one final write (a measured 3008-token response landed
after 36 seconds); tool-using turns can expose intermediate message records,
which D27 uses for partial ticks. No JSONL write means no new measurement, so
an "instantaneous, reacts to every generated token" needle remains impossible
from this source. The honest output is a stepwise per-response rate. True
streaming would require another telemetry source such as OTEL.

Codex uses a separate defensive decoder over recent rollout JSONL beneath
`CODEX_HOME` (`~/.codex` by default), including recursive `sessions/` and
`archived_sessions/`. `session_meta.id` supplies thread identity,
`turn_context.model` supplies model activity, and each non-duplicate
`token_count.info.last_token_usage.output_tokens` supplies one response-rate
step. Reads, lines, recursion and active file count are bounded; unknown
formats fail closed without affecting Claude telemetry.

## Source 3 — Claude Code statusline JSON (self-installing sensor)

Claude Code passes, via stdin to a **configured statusline script**, a JSON with
**official** account data. It is **push**: an external window does not receive it on its own.

**Wiring (see D12).** cc-autobahn **is** that script and installs itself: on first
launch it writes the `statusLine` key to `~/.claude/settings.json` (with
consent, backup, and rollback). On each invocation its binary (1) emits the normal
statusline line to stdout (preserving the previous one or falling back to a default) and (2) atomically writes
the full JSON to `~/.claude/cc-autobahn-status.json`, which the GUI tails. This way the official data arrives
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
- `model.id` → PRND selector. `effort.level` is parsed but not rendered (D29).
- `cost.total_cost_usd` → session price.
- `context_window.used_percentage` → **context-fill gauge**, taken as-is (Claude's own figure, not recomputed).
- `context_window.current_usage` → the one in-house derived figure from this source: **prompt-cache hit rate**, `cache_read_input_tokens / (cache_read_input_tokens + cache_creation_input_tokens + input_tokens)` (D51). Codex derives the same two metrics from its own `last_token_usage` rollout field instead.

Docs: <https://code.claude.com/docs/en/statusline>

## Source 4 — Codex App Server account sensor

One owned `codex app-server --stdio` child supplies Codex's official account
limits and account token-usage summary. cc-autobahn initializes only the stable
surface, probes `account/rateLimits/read` and `account/usage/read`, and consumes
`account/rateLimits/updated` notifications. Sparse updates merge into the last
full snapshot without treating missing or null metadata as a deletion.

All `rateLimitsByLimitId` buckets are preserved. The `codex` bucket drives the
primary/secondary gauges when present; legacy `rateLimits` remains the fallback.
The selected executable and its reported version own the connection. Unsupported
methods, authentication modes, stale data and child exits degrade this component
without stopping rollout telemetry or `ccusage codex` history. Account usage is
official token telemetry, **not** official USD billing.

Protocol input is newline-delimited JSON-RPC with a 1 MiB message bound. The
adapter keeps one child, correlates request IDs, stores startup snapshots, polls
once per minute, marks old snapshots stale then unavailable, and reconnects with
bounded backoff.

Docs: <https://developers.openai.com/codex/app-server/>

## Source 5 (optional) — OpenTelemetry

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
| JSONL tails (`tok/s`) | **200 ms file follow + 5 s discovery** | independent Claude/Codex ticks across concurrent sessions/threads |
| Statusline (`rate_limits`, model) | **push** | arrives whenever Claude Code renders |
| Codex App Server account sensor | **push + 60 s capability probe** | sparse limit notifications stay live; polling recovers missed updates and refreshes usage |
| `ccusage claude daily` (History/Limits) | **on-demand** (D33) | daily totals barely move within a day; fetched only when that MFD page opens, cached client-side ~5 min |

## Zero-friction strategy (the app wires itself up, D9)

Telemetry has two setup wires. The statusline modification requires explicit
consent; engine installation is offered only when no runtime is found.

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
dumps the official JSON to `~/.claude/cc-autobahn-status.json`, which the
window tails. No manual editing.

**Independent permission wire** (D42): Settings can separately install an
opt-in `hooks.PermissionRequest` entry. This is request/response, not usage
telemetry: its short-lived hook process communicates with the GUI over
`~/.claude/cc-autobahn/permission.sock` and fails open to Claude Code's own
terminal prompt when the GUI is unavailable.

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
