# Arquitectura

## Principio rector

**cc-autobahn NO es un medidor de tokens: es un cuadro de instrumentos.**
El cálculo de consumo, pricing y ventanas de facturación es un problema resuelto
por [`ccusage`](https://ccusage.com). Nosotros no lo reimplementamos ni lo forkeamos
— lo consumimos como fuente de datos. Todo el valor de este proyecto está en la
**capa visual** (skin Mercedes W203) y en el cálculo del `tok/s` **por respuesta**
(D8), que ninguna herramienta existente ofrece.

```
┌──────────────────────────────────────────────────────────────┐
│                        cc-autobahn                           │
│                                                              │
│  ┌────────────┐   IPC (Tauri commands)   ┌────────────────┐  │
│  │  Frontend  │ <──────────────────────> │  Backend Rust  │  │
│  │  (webview) │                          │  (src-tauri)   │  │
│  │            │                          │                │  │
│  │  · skin    │                          │  · exec ccusage│  │
│  │    ámbar   │                          │  · tail JSONL  │  │
│  │  · agujas/ │                          │  · detect eng. │  │
│  │    barras  │                          │  · timers      │  │
│  └────────────┘                          └───────┬────────┘  │
└──────────────────────────────────────────────────┼──────────┘
                                                    │
                        ┌───────────────────────────┼───────────────┐
                        │                           │               │
                 ┌──────▼──────┐          ┌─────────▼────────┐  ┌───▼──────────┐
                 │   ccusage    │          │ ~/.claude/**.jsonl│  │ statusline   │
                 │  --json      │          │  (tail → tok/s)   │  │ JSON (rate_  │
                 │ (motor datos)│          │                   │  │ limits)      │
                 └──────────────┘          └───────────────────┘  └──────────────┘
```

## Capas

### 1. Backend Rust (`src-tauri/`)
Responsable de **todo el I/O**. Nunca bloquea la UI.

- **Ejecución de subprocesos**: `std::process::Command` desde Rust (D16). Sin
  `tauri-plugin-shell` — ese plugin es para exec desde el frontend JS; nuestro I/O es
  backend confiable. El motor corre en un `std::thread` dedicado (sin async framework).
- **Detección de motor** (`engine::detect`): recorre el `$PATH` buscando `ccusage`
  global → `npx` → `bunx` → ninguno. Ver [DATA-ENGINE.md](./DATA-ENGINE.md).
- **Poll de ccusage** (`engine::poll_once`): ejecuta `ccusage blocks --active --json`
  cada **15 s** (D13, ventana 10–30 s), parsea con `serde_json`, emite `blocks-update`
  / `blocks-idle` / `engine-error` al frontend.
- **Tail de JSONL** (`engine::burn`): sigue el log de sesión activo, calcula `tok/s`
  **por respuesta** (`Δoutput / Δt_turno`) al completarse cada turno. Es el dato que
  ccusage no da — pero **no es instantáneo**: el JSONL solo reporta al terminar el
  turno (ver D8/DATA-ENGINE §Fuente 2).
- **Sensor statusline** (`engine::sensor`): instala cc-autobahn como comando
  `statusLine` en `~/.claude/settings.json` (consentimiento + backup + rollback, D12)
  y tailea el socket donde su binario vuelca el JSON oficial (`rate_limits`, modelo,
  effort, coste).
- **Histórico** (`engine::history`): `ccusage daily|monthly --json` bajo demanda.
- **Ventana**: frameless, always-on-top, transparente (requiere `macOSPrivateApi` en
  macOS, D14), arrastrable. Config en `tauri.conf.json`; permisos en
  `capabilities/default.json`.

### 2. Frontend (webview, `index.html` + `src/`)
Solo **presentación**. No hace I/O de sistema; recibe datos por IPC/eventos.

- `index.html`: estructura del cluster (display + selector PRND).
- `src/style.css`: skin ámbar VFD W203 (ver [DESIGN.md](./DESIGN.md)).
- `src/main.js`: render. En la base actual solo reloj + barra de segmentos.

## Flujo de datos (objetivo)

1. Al arrancar, backend detecta motor. Si falta → evento `engine-missing` →
   frontend muestra pantalla "CHECK ENGINE". Y ofrece conectar el sensor statusline
   (D12) si no está instalado.
2. Timer backend **cada 10–30 s** (D13) → `ccusage blocks --active --json` → evento
   `blocks-update` con burn medio, proyección, coste.
3. Tail JSONL en paralelo → al completarse un turno, evento `burn-tick` con `tok/s`
   **por respuesta** → aguja que salta + decae (no instantánea, D8).
4. Sensor statusline (push) → evento `sensor-update` con `rate_limits.five_hour`
   (autonomía **oficial**), `seven_day` (barra semanal), `model.id`, `effort`, coste.
5. Frontend pinta: velocímetro, barra segmentos, trip, selector modelo.

## Por qué Tauri (no Electron)

- Webview del SO → binario ~5 MB vs ~150 MB de Electron.
- Backend Rust nativo para exec/tail sin overhead.
- `always-on-top` + frameless + transparente + `data-tauri-drag-region` nativos.
- Cross-OS real (macOS / Windows / Linux).

## Estado actual

**Fases 0–2 hechas.** Backend arranca la ventana y corre dos sensores en hilos
dedicados: `engine` (ccusage `blocks --active --json` cada 15 s →
coste/proyección) y `burn` (tail del JSONL activo → `tok/s` por respuesta →
`burn-tick`, D17). El frontend pinta el velocímetro con muelle físico (D18) y
loguea los eventos de `blocks`; el cableado de coste/odómetro/autonomía-oficial
es Fase 3. Ver [ROADMAP.md](./ROADMAP.md).
