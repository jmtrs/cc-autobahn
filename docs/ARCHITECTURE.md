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
- **Ventana / tray**: icono en la barra de menú de macOS (`TrayIconBuilder`, sin
  plugin nuevo, D24), sin Dock ni Cmd+Tab (`ActivationPolicy::Accessory`). El
  icono en sí **no es un PNG estático**: es un anillo de progreso (% de la
  ventana de 5h restante) redibujado en runtime pixel a pixel por
  `tray_icon.rs`, actualizado desde `engine::poll` y `sensor::tail` en cada
  dato nuevo (D30). Click izquierdo muestra/oculta el panel, anclado justo
  bajo el icono (posición calculada desde `TrayIconEvent::rect`); click fuera
  lo oculta (hide-on-blur vía `WindowEvent::Focused(false)`, con guard
  anti-carrera de 300 ms, salvo con el botón PIN activo, D26); click derecho
  abre menú con "Salir". La ventana en sí sigue frameless, transparente
  (requiere `macOSPrivateApi`, D14), `alwaysOnTop` y con esquinas nativas
  redondeadas vía `CALayer` (D25). Ya no es arrastrable (sustituye D6). Config
  en `tauri.conf.json`; permisos en `capabilities/default.json` (recortados a
  solo `core:default` + `core:event:default` — el control de ventana ocurre
  100% en Rust, no vía IPC).

### 2. Frontend (webview, `index.html` + `src/`)
Solo **presentación**. No hace I/O de sistema; recibe datos por IPC/eventos.

- `index.html`: estructura del cluster (display + selector PRND + botón PIN,
  D26 + overlay de consentimiento del sensor).
- `src/style.css`: skin ámbar VFD W203 (ver [DESIGN.md](./DESIGN.md)).
- `src/main.js`: render — velocímetro con muelle físico (D18), barra de
  segmentos/autonomía (estimada `EST` o oficial con prioridad y congelado
  ante desconexión momentánea, D23/D28), selector PRND (D7, sin kickdown,
  D29), footer PACE/AUTO alternable (D28, persistido en `localStorage`).

## Flujo de datos (objetivo)

1. Al arrancar, backend detecta motor. Si falta → evento `engine-missing` →
   frontend muestra pantalla "CHECK ENGINE". Y ofrece conectar el sensor statusline
   (D12) si no está instalado.
2. Timer backend **cada 10–30 s** (D13) → `ccusage blocks --active --json` → evento
   `blocks-update` con burn medio, proyección, coste.
3. Tail JSONL en paralelo → al completarse un turno, evento `burn-tick` con `tok/s`
   **por respuesta** → aguja que salta + decae (no instantánea, D8).
4. Sensor statusline (push) → evento `sensor-update` con `rate_limits.five_hour`
   (autonomía **oficial**), `seven_day` (tinte de borde al 80%), `model.id`
   (selector PRND), coste. `effort.level` llega en el payload pero ya no se
   pinta (kickdown retirado, D29).
5. Frontend pinta: velocímetro, barra segmentos, trip, selector modelo, footer
   PACE/AUTO. En paralelo, el icono de bandeja recibe el mismo % de autonomía
   restante y redibuja su anillo de progreso (D30) — no pasa por el frontend,
   se calcula directo en Rust en el punto de emisión de cada evento.

## Por qué Tauri (no Electron)

- Webview del SO → binario ~5 MB vs ~150 MB de Electron.
- Backend Rust nativo para exec/tail sin overhead.
- `always-on-top` + frameless + transparente + tray/menu-bar nativos (D24).
- Cross-OS real (macOS / Windows / Linux).

## Estado actual

**Fases 0–4.5 hechas; Fase 5 en curso** (ver checklist real en
[ROADMAP.md](./ROADMAP.md)). Backend arranca oculto tras el icono de bandeja
(D24) y corre tres sensores en hilos dedicados: `engine` (ccusage `blocks
--active --json` cada 15 s → coste/proyección), `burn` (tail del JSONL activo
→ `tok/s` por respuesta → `burn-tick`, D17, con tick parcial por mensaje
intermedio y cadencia de 200 ms, D27) y `sensor` (tail del fichero que vuelca
el statusline → dato **oficial** `rate_limits` → `sensor-update`, D12). El
frontend pinta velocímetro con muelle físico (D18), barra de segmentos
(`blocks` estimado con marca "EST", o `sensor` oficial con prioridad y
congelado ante desconexión momentánea, D23/D28), selector PRND (D7) y footer
PACE/AUTO alternable (D28). El mismo binario es el comando `statusLine` (modo
dual, early-return, D19) con chain del statusLine previo (D21) y
auto-instalación con consent/backup/rollback (D20/D22). La ventana flotante
siempre visible se sustituyó por un icono de menu-bar con panel bajo demanda
(D24, solo macOS por ahora), con esquinas nativas redondeadas (D25) y botón
PIN para fijarlo (D26). Ese icono de bandeja es ahora un anillo de progreso
redibujado en runtime, no un PNG estático (D30). El kickdown (indicador de
effort) se implementó y se retiró después por no aportar valor visual (D29).
