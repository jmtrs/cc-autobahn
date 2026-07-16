# Motor de datos

cc-autobahn no calcula el uso: lo **lee** de tres fuentes oficiales/estándar.
Todos los datos brutos existen ya; nosotros los presentamos.

## Fuente 1 — ccusage (motor principal)

[`ccusage`](https://ccusage.com) (ryoppippi) — la herramienta de análisis de uso
de Claude Code más usada y estable. Lee `~/.claude/projects/**/*.jsonl` y resuelve
pricing (LiteLLM), deduplicación del bloque de 5 h compartido entre sesiones, y el
multiplicador de Opus. **No la forkeamos: consumimos su salida `--json`.**

Versión de referencia: **20.0.17**.

### Comandos

```bash
ccusage blocks --active --json   # bloque 5h activo: burnRate, projection, start/end
ccusage daily   --json           # histórico diario
ccusage monthly --json           # histórico mensual
ccusage session --json           # por sesión
```

### Campos por objeto

`inputTokens`, `outputTokens`, `cacheCreationTokens`, `cacheReadTokens`,
`totalTokens`, `totalCost`. Los `blocks` incluyen además burn rate (tokens/min)
y proyección al final de la ventana de 5 h.

### Nota importante

`ccusage blocks --live` (dashboard en vivo) fue **eliminado en v18.0.0**. Por eso
cc-autobahn hace su propio polling de `--json` + tail de JSONL, en vez de depender
de un modo live que ya no existe.

## Fuente 2 — Tail de JSONL (tok/s por respuesta)

ccusage da burn rate por **minuto** (media suavizada). Nosotros damos `tok/s`
**por respuesta**, siguiendo el log de la sesión activa en
`~/.claude/projects/**/*.jsonl`: al completarse un turno, `Δoutput / Δt_turno` →
`tok/s`. Aguja que **salta al completar** + decae con muelle a ralentí.

**Límite físico (validado 2026-07-16).** El campo `usage` del JSONL **no se
transmite en streaming**: se estampa **idéntico** en todas las líneas de un turno y
solo aparece **al terminar** el turno (medido: un turno de 3008 tokens de salida
aterriza de golpe tras 36 s de silencio). El log **no ve el turno en curso**. Por
eso una aguja "instantánea que reacciona al pisar" es **imposible** desde aquí — lo
honesto es el promedio por respuesta como escalón. Sigue siendo el diferencial
(ningún competidor lo muestra), pero con la etiqueta verdadera (ver D8/D11). Streaming
real → OTEL con streaming, parqueado en Fase 6.

## Fuente 3 — Statusline JSON de Claude Code (sensor auto-instalado)

Claude Code pasa por stdin a un **script de statusline configurado** un JSON con
datos **oficiales** de la cuenta. Es **push**: una ventana externa no lo recibe sola.

**Cableado (ver D12).** cc-autobahn **es** ese script y se instala solo: al primer
arranque escribe la clave `statusLine` en `~/.claude/settings.json` (con un
consentimiento, backup y rollback). En cada invocación su binario (1) emite a stdout
la línea de statusline normal (respeta la previa o pone una por defecto) y (2) vuelca
el JSON completo a un socket/fichero que la ventana tailea. Así el dato oficial llega
**sin que el usuario edite nada a mano**. Campos relevantes:

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

- `rate_limits.five_hour` → **autonomía** (dato oficial, mejor que la estimación).
- `rate_limits.seven_day` → **aguja semanal** (ccusage no la enmarca así).
- `model.id` → selector PRND. `effort.level` → kickdown.
- `cost.total_cost_usd` → precio de la sesión.

Docs: <https://code.claude.com/docs/en/statusline>

## Fuente 4 (opcional) — OpenTelemetry

Para dashboards históricos avanzados, Claude Code emite métricas OTEL:

```bash
export CLAUDE_CODE_ENABLE_TELEMETRY=1
export OTEL_METRICS_EXPORTER=prometheus   # o otlp / console
```

- `claude_code.token.usage` — atributos: `type` (`input`/`output`/`cacheRead`/
  `cacheCreation`), `model`, `query_source`, `speed`, `effort`, `agent.name`…
- `claude_code.cost.usage` — coste en USD.

Con Prometheus+Grafana, `rate()` sobre `claude_code.token.usage` = tok/s real.
No es necesario para el MVP; queda como integración futura opcional.

Docs: <https://code.claude.com/docs/en/monitoring-usage>

## Cadencias (D13 — no un poll único)

| Sensor | Cadencia | Por qué |
| ------ | -------- | ------- |
| `ccusage blocks` | **10–30 s** (o proceso persistente) | el bloque de 5 h no cambia por segundo; `npx` cada 1–2 s es derroche |
| Tail JSONL (`tok/s`) | **evento** (al escribirse el log) | no polling |
| Statusline (`rate_limits`, modelo) | **push** | llega cuando Claude Code renderiza |

## Estrategia cero fricción (la app se cablea sola, D9)

Dos cables auto-instalados. El usuario solo da **un consentimiento**.

**Cable A — motor de datos** (`engine::detect`):

1. **`ccusage` global** en PATH → usar directamente.
2. **Node** presente → `npx -y ccusage@latest blocks --json` (sin instalar nada).
3. **Bun** presente → `bunx ccusage blocks --json`.
4. **Sin runtime** → pantalla ámbar "CHECK ENGINE" con botón único
   **"INSTALAR MOTOR"** que descarga Bun (binario portable) y ejecuta `bunx ccusage`.
5. **Futuro**: empaquetar Bun como *sidecar* de Tauri → 0 dependencias del usuario.

**Cable B — sensor statusline** (D12): la app escribe la clave `statusLine` en
`~/.claude/settings.json` (backup + rollback) apuntando a su propio binario, que
vuelca el JSON oficial a un socket que la ventana tailea. Sin edición manual.

## Precisión / honestidad

- Con **suscripción** (Pro/Max), el precio USD es **estimado** (ccusage lo calcula
  por pricing público). La autonomía (`rate_limits`) sí es dato oficial.
- Con **API**, el coste es exacto. El billing oficial siempre es la Claude Console.

## Comparativa de herramientas existentes (por qué ccusage)

| Herramienta                    | tok/s inst. | in/out/cache | coste+proy. | 5h+ETA | semanal | histórico | estética coche |
| ------------------------------ | :---------: | :----------: | :---------: | :----: | :-----: | :-------: | :------------: |
| **ccusage**                    |     ❌      |      ✅      |     ✅      |   ✅   |   ❌    |    ✅     |       ❌       |
| Claude-Code-Usage-Monitor      |  ❌ (/min)  |      ✅      |     ✅      |   ✅   |   ❌    |    ✅     |       ❌       |
| par-cc-usage                   |  ❌ (/min)  |      ✅      |     ✅      |   ✅   |   ❌    |    ✅     |       ❌       |
| ccburn / codeburn              |  ❌ (/min)  |      ✅      |     ~       |   ✅   |   ❌    |    ~      |       ❌       |
| **cc-autobahn** (este)         |   ✅ nuevo  |      ✅      |     ✅      |   ✅   | ✅ nuevo |    ✅     |    ✅ único    |

ccusage gana como **motor** (estándar de facto, más estable, JSON limpio).
cc-autobahn aporta lo que falta: `tok/s` **por respuesta**, aguja semanal y la piel
W203. (La columna "tok/s inst." de la tabla se lee como "por respuesta": ver D8 — el
JSONL no permite instantáneo real.)
