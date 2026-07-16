# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Qué es

cc-autobahn es un **cuadro de instrumentos** flotante (Tauri v2) con estética del display VFD ámbar del Mercedes W203, que muestra el consumo de tokens de Claude Code. Ventana frameless, always-on-top, transparente y arrastrable.

**Principio rector: no es un medidor de tokens, es una skin visual.** El parseo de logs, pricing y ventanas de facturación se delegan a [`ccusage`](https://ccusage.com), ejecutado como proceso hijo vía su salida `--json`. No se forkea ni se reimplementa (ver `docs/DECISIONS.md` D1–D3). El único cálculo propio es el `tok/s` **por respuesta** (`Δoutput / Δt_turno` sobre el tail de JSONL), que ccusage no ofrece. **No es instantáneo**: el JSONL solo estampa `usage` al terminar el turno (validado empíricamente, D8) — la aguja salta al completar y decae, no reacciona mid-generación.

**Estado actual: Fase 1 (motor de datos) hecha.** El backend detecta ccusage (`$PATH`: global → npx → bunx) y pollea `blocks --active --json` cada 15 s en un hilo dedicado, emitiendo eventos (`blocks-update`, `blocks-idle`, `engine-error`, `engine-missing`, `engine-detected`). Modelo serde validado con tests contra JSON real (`cargo test`, 3/3). El frontend aún pinta placeholders + loguea los eventos; el cableado al display es Fase 3. Roadmap en `docs/ROADMAP.md`.

## Comandos

```bash
npm install          # Vite + Tauri CLI
npm run tauri dev    # compila Rust y abre el cluster (dev)
npm run tauri build  # binario de release
npm run dev          # solo frontend Vite (puerto 1420, strictPort)
```

Regenerar iconos:

```bash
node scripts/make-icon.mjs                        # icono ámbar zero-dep
npx @tauri-apps/cli icon scripts/source-icon.png  # deriva todos los tamaños
```

No hay tests ni linter configurados.

## Arquitectura (dos capas)

- **Backend Rust (`src-tauri/`)** — responsable de **todo el I/O**, nunca bloquea la UI. Hoy solo `src/main.rs` arranca la ventana. Comandos previstos (no implementados): `engine::detect` (ccusage global → npx → bunx), `engine::poll_blocks` (`ccusage blocks --active --json` a **cadencia lenta 10–30 s**, no 1–2 s — D13), `engine::burn` (tail JSONL → `tok/s` por respuesta), `engine::sensor` (sensor statusline auto-instalado, ver abajo), `engine::history`.
- **Frontend (webview, `index.html` + `src/`)** — solo presentación, sin I/O de sistema; recibe datos por IPC/eventos de Tauri. `src/style.css` = skin ámbar; `src/main.js` = render.

**Tres sensores, tres cadencias (D13):** ccusage = poll lento (coste/proyección); tail JSONL = evento por turno (`tok/s`); statusline = push (dato oficial `rate_limits`).

**Sensor statusline (D12) — así llega el dato oficial:** el JSON de statusline es *push* (Claude Code lo pasa por stdin solo a un script configurado); una ventana externa no lo recibe pasiva. cc-autobahn **es** ese script y se auto-instala: escribe `statusLine` en `~/.claude/settings.json` (consentimiento + backup + rollback) apuntando a su binario, que emite la línea normal a stdout **y** vuelca el JSON a un socket que la ventana tailea. Es la única fuente de `rate_limits.five_hour/seven_day` (autonomía **oficial**).

Flujo objetivo: backend emite eventos (`blocks-update`, `burn-tick`, `sensor-update`, `engine-missing`) que el frontend escucha y pinta. Detalle en `docs/ARCHITECTURE.md`.

## Mapeo coche → tokens (lenguaje del dominio)

| Elemento W203        | Métrica Claude Code                      |
| -------------------- | ---------------------------------------- |
| Velocímetro          | `tok/s` por respuesta (cálculo propio)   |
| Consumo (L/100 Km)   | Coste medio `$/Mtok`                     |
| Autonomía / depósito | Ventana de 5 h restante (barra segmentos)|
| Trip "AFTER START"   | Tokens/tiempo desde el último reset      |
| Odómetro             | Tokens totales acumulados                |
| Selector PRND        | Modelo activo (O/S/H/F) iluminado        |

## Convenciones

- **Config de ventana en `tauri.conf.json`; permisos en `capabilities/default.json`** (v2). La window tiene `label: "cluster"` — las capabilities se atan a ese label. Zonas arrastrables usan `data-tauri-drag-region`. `app.macOSPrivateApi: true` es obligatorio para la transparencia en macOS (D14) — no quitarlo.
- **Exec desde Rust con `std::process::Command`, NO `tauri-plugin-shell`** (D16). El plugin es para exec desde el frontend JS; nuestro I/O es backend confiable. El motor corre en un `std::thread` dedicado (sin async framework). Cero deps nuevas.
- **`macos-private-api` (feature cargo) va acoplada a `macOSPrivateApi` (conf)**: si tocas una, la otra. El build script de tauri falla si no casan.
- **CSP sigue `null`** a propósito mientras no haya IPC/red; al aterrizar el primer comando aplicar la política de D15 y verificar HMR en `tauri dev`.
- **Puerto dev fijo 1420** (`vite.config.js` + `devUrl`); `clearScreen: false` para no perder logs de Rust.
- **Dependencias fijadas a las últimas estables** por decisión del usuario (D10): no downgradar Vite/Tauri/serde sin motivo.
- **Precisión honesta** (D11): el coste con suscripción es **estimado**; la ventana `rate_limits` es dato **oficial**. No presentar estimaciones como facturación real.
- **Documentación y comentarios en español**; los ADR en `docs/DECISIONS.md` registran el porqué de cada decisión — consultarlos antes de cambiar arquitectura, motor de datos o estética.
