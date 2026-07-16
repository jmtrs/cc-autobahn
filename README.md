# cc-autobahn

> Un **cuadro de instrumentos Mercedes W203** para el consumo de tokens de
> Claude Code. Vive como icono en la barra de menú de macOS: click izquierdo
> muestra/oculta un panel frameless, transparente y *always-on-top* anclado
> bajo el icono, con el display VFD ámbar de matriz de puntos: `tok/s` por
> respuesta, autonomía de la ventana de 5 h, coste y modelo activo.

cc-autobahn **no es un medidor de tokens: es una skin visual**. El parseo de
logs, el pricing y las ventanas de facturación se delegan a
[`ccusage`](https://ccusage.com) — ejecutado como proceso hijo vía su salida
`--json`, sin fork ni reimplementación. El único cálculo propio es el `tok/s`
**por respuesta** (`Δoutput / Δt_turno` sobre el tail del JSONL), que ninguna
herramienta existente ofrece.

## Estado

**Fases 0–5 hechas** (solo queda Fase 6, histórico, opcional — ver
[roadmap](./docs/ROADMAP.md) para el checklist real). El backend corre tres
sensores en hilos dedicados:

- `engine` — detecta ccusage (global → npx → bunx → botón "Instalar motor" que
  instala Bun solo, D9) y pollea `blocks --active --json` cada 15 s → coste,
  proyección y autonomía estimada.
- `burn` — hace tail del JSONL de la sesión activa → `tok/s` por respuesta →
  evento `burn-tick`. El velocímetro salta al completar turno y decae con
  muelle físico (D8/D18).
- `sensor` — el mismo binario se auto-instala como comando `statusLine` de
  Claude Code (consentimiento + backup + rollback, D12) y tailea el JSON
  oficial (`rate_limits.five_hour/seven_day`) que sustituye a la proyección
  estimada en cuanto llega.

Sin motor detectado, el panel muestra el overlay "CHECK ENGINE" en vez de
datos. Icono de bandeja (menu-bar, sin Dock ni Cmd+Tab) con anillo de
progreso redibujado en runtime; panel con botón PIN y footer PACE/AUTO
alternable. `cargo test` 26/26 (incluye verificación contra JSONL y
statusline reales), `cargo clippy` limpio.

## Diseño (mapeo coche → tokens)

| Elemento W203            | Métrica Claude Code                        |
| ------------------------ | ------------------------------------------ |
| Velocímetro (Km/h)       | `tok/s` por respuesta (`Δoutput / Δt_turno`) |
| Consumo (L/100 Km)       | Coste medio `$/Mtok`                        |
| Autonomía / depósito ⛽  | Ventana de 5 h restante (barra segmentos)  |
| Trip "AFTER START"       | Tokens/tiempo desde el último reset         |
| Odómetro                 | Tokens totales acumulados                   |
| Selector PRND            | Modelo activo (O/S/H/F) iluminado + effort |
| Reloj                    | Hora real                                   |

## Filosofía

- **No reinventar el motor.** Los datos vienen de
  [`ccusage`](https://ccusage.com) (estándar de facto), como proceso hijo.
- **cc-autobahn = cuadro de instrumentos.** Aportamos la capa visual (skin W203)
  y el `tok/s` por respuesta.
- **Cero fricción.** La app se cablea sola (motor + sensor statusline) con un
  único consentimiento (D9/D12).
- **Precisión honesta.** El coste con suscripción es *estimado*; la autonomía
  (`rate_limits`) es dato *oficial*; el billing real es la Claude Console (D11).

## Fuentes y cadencias

| Sensor | Cadencia | Qué da |
| ------ | -------- | ------ |
| `ccusage blocks --active --json` | 10–30 s | burn medio, proyección, coste |
| Tail de `~/.claude/projects/**/*.jsonl` | por turno (evento) | `tok/s` por respuesta |
| Statusline JSON (sensor auto-instalado) | push | `rate_limits.five_hour`/`seven_day` oficial |

> **No es instantáneo.** El JSONL solo estampa `usage` al cerrar el turno (validado
> empíricamente, D8): la aguja salta al completar y decae, no reacciona
> mid-generación.

## Desarrollo

Requisitos: [Node.js](https://nodejs.org/), [Rust](https://rustup.rs/) y las
[dependencias de Tauri v2](https://v2.tauri.app/start/prerequisites/).

```bash
npm install          # Vite + Tauri CLI
npm run tauri dev    # compila Rust y abre el cluster (dev, puerto 1420)
npm run tauri build  # binario de release
```

Tests del backend (Rust):

```bash
cd src-tauri && cargo test
```

Regenerar iconos desde otro logo:

```bash
node scripts/make-icon.mjs
npx @tauri-apps/cli icon scripts/source-icon.png
```

## Estructura

```
cc-autobahn/
├── index.html            # carcasa del cluster (display, selector PRND, overlays)
├── src/
│   ├── style.css         # skin ámbar VFD W203
│   └── main.js           # render: reloj, segmentos, velocímetro con muelle,
│                          # overlays CHECK ENGINE / sensor, PIN, footer PACE/AUTO
├── scripts/
│   └── make-icon.mjs      # generador de icono ámbar (zero-dep PNG)
├── src-tauri/
│   ├── Cargo.toml
│   ├── tauri.conf.json    # ventana frameless, always-on-top, transparente
│   ├── capabilities/      # permisos v2 (core:default + core:event:default)
│   ├── icons/             # iconos de la app + tray-icon-template.png
│   └── src/
│       ├── main.rs        # entrypoint dual (GUI / modo statusline) + tray/menu-bar
│       ├── engine.rs      # sensor ccusage (detect + poll blocks + install_bun)
│       ├── burn.rs        # sensor tok/s: tail JSONL → burn-tick
│       ├── sensor.rs      # sensor statusline oficial (auto-instalación + tail)
│       └── tray_icon.rs   # anillo de progreso del icono de bandeja
├── docs/                  # arquitectura, diseño, decisiones (ADR), roadmap
├── vite.config.js
└── package.json
```

## Documentación

- [docs/ARCHITECTURE.md](./docs/ARCHITECTURE.md) — capas, flujo de datos, por qué Tauri.
- [docs/DESIGN.md](./docs/DESIGN.md) — lenguaje visual W203, paleta.
- [docs/DATA-ENGINE.md](./docs/DATA-ENGINE.md) — ccusage, statusline, OTEL, comparativa.
- [docs/DECISIONS.md](./docs/DECISIONS.md) — registro de decisiones (ADR) y motivos.
- [docs/ROADMAP.md](./docs/ROADMAP.md) — fases de implementación.

## Roadmap

Fases 0–5 hechas (chasis, motor de datos, `tok/s` por respuesta, sensor
statusline oficial, cero fricción, tray/menu-bar, pulido). Checklist real y
actualizado en [docs/ROADMAP.md](./docs/ROADMAP.md) — no lo dupliques aquí,
que se desalinea. Solo queda **Fase 6** (histórico, opcional): vista
semanal/mensual (`ccusage daily|monthly`) e integración OTEL.

## Licencia

[MIT](./LICENSE).
