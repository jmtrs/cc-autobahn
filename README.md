# cc-autobahn

> Un **cuadro de instrumentos Mercedes W203** para el consumo de tokens de
> Claude Code. Ventana flotante, *always-on-top*, con el display VFD ámbar de
> matriz de puntos: `tok/s` por respuesta, autonomía de la ventana de 5 h,
> coste y modelo activo.

cc-autobahn **no es un medidor de tokens: es una skin visual**. El parseo de
logs, el pricing y las ventanas de facturación se delegan a
[`ccusage`](https://ccusage.com) — ejecutado como proceso hijo vía su salida
`--json`, sin fork ni reimplementación. El único cálculo propio es el `tok/s`
**por respuesta** (`Δoutput / Δt_turno` sobre el tail del JSONL), que ninguna
herramienta existente ofrece.

## Estado

**Fases 0–2 hechas.** El backend corre dos sensores en hilos dedicados:

- `engine` — detecta ccusage y pollea `blocks --active --json` cada 15 s →
  coste, proyección y autonomía.
- `burn` — hace tail del JSONL de la sesión activa → `tok/s` por respuesta →
  evento `burn-tick`. El velocímetro salta al completar turno y decae con
  muelle físico (D8/D18).

`cargo test` 16/16 (incluye verificación contra JSONL real). El cableado de
coste/odómetro/autonomía-oficial al display es la Fase 3. Ver
[roadmap](./docs/ROADMAP.md).

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
├── index.html            # carcasa del cluster (display + selector PRND)
├── src/
│   ├── style.css         # skin ámbar VFD W203
│   └── main.js           # render: reloj, segmentos, velocímetro con muelle
├── scripts/
│   └── make-icon.mjs      # generador de icono ámbar (zero-dep PNG)
├── src-tauri/
│   ├── Cargo.toml
│   ├── tauri.conf.json    # ventana frameless, always-on-top, transparente
│   ├── capabilities/      # permisos v2 (drag, always-on-top)
│   ├── icons/             # iconos de la app
│   └── src/
│       ├── main.rs        # entrypoint: arranca ventana + sensores
│       ├── engine.rs      # sensor ccusage (detect + poll blocks)
│       └── burn.rs        # sensor tok/s: tail JSONL → burn-tick
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

- [x] **Fase 0** — Chasis: Tauri v2 frameless always-on-top + skin ámbar estático.
- [x] **Fase 1** — Motor de datos: `engine::detect` + `poll_once` (`std::process`,
      sin plugin) → eventos; modelo serde contra JSON real de ccusage v20.
- [x] **Fase 2** — `tok/s` por respuesta: tail del JSONL → `burn-tick`; velocímetro
      con muelle físico (D8/D17/D18). `cargo test` 16/16.
- [ ] **Fase 3** — Sensor statusline + cablear display (odómetro, trip, coste,
      autonomía oficial, selector PRND).
- [ ] **Fase 4** — Cero fricción: "CHECK ENGINE" + instalación del motor +
      auto-instalación del sensor statusline.
- [ ] **Fase 5** — Pulido: bandeja, recordar posición, fuente dot-matrix, zona roja.
- [ ] **Fase 6** — Histórico (opcional): vista semanal/mensual, OTEL.

## Licencia

[MIT](./LICENSE).
