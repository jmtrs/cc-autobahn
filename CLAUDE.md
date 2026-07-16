# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Qué es

cc-autobahn es un **cuadro de instrumentos** (Tauri v2) con estética del display VFD ámbar del Mercedes W203, que muestra el consumo de tokens de Claude Code. Vive como icono en la barra de menú de macOS (D24): click izquierdo muestra/oculta un panel frameless, transparente y always-on-top anclado bajo el icono; sin Dock ni Cmd+Tab. Ya no es una ventana flotante arrastrable (D6, superado).

**Principio rector: no es un medidor de tokens, es una skin visual.** El parseo de logs, pricing y ventanas de facturación se delegan a [`ccusage`](https://ccusage.com), ejecutado como proceso hijo vía su salida `--json`. No se forkea ni se reimplementa (ver `docs/DECISIONS.md` D1–D3). El único cálculo propio es el `tok/s` **por respuesta** (`Δoutput / Δt_turno` sobre el tail de JSONL), que ccusage no ofrece. **No es instantáneo**: el JSONL solo estampa `usage` al terminar el turno (validado empíricamente, D8) — la aguja salta al completar y decae, no reacciona mid-generación.

**Estado actual: Fases 0–5 hechas** (checklist real en `docs/ROADMAP.md`, el porqué de cada pieza en `docs/DECISIONS.md` D1–D30). Solo queda Fase 6 (histórico, opcional) y dos ítems opcionales/futuros (sidecar de Bun, Windows/Linux). Los tres sensores (`engine`, `burn`, `sensor`) corren en hilos dedicados y están cableados al display: velocímetro (muelle físico, D18), barra de segmentos (estimada `EST` o dato oficial con prioridad, D23), selector PRND (D7), footer PACE/AUTO (D28), botón PIN (D26). Sin motor detectado, el frontend pinta el overlay "CHECK ENGINE" con un botón que instala Bun solo (`engine::install_bun`, D9) y relanza el motor sin reiniciar la app. El binario dual también es el comando `statusLine` de Claude Code (auto-instalable, D19–D22). El icono de bandeja (D24) es un anillo de progreso redibujado en runtime, no un PNG estático (D30). `cargo test` 26/26, `cargo clippy` limpio.

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

Backend: `cargo test` (26 tests, en `src-tauri/`) + `cargo clippy` limpio. Frontend: sin tests ni linter configurados.

## Arquitectura (dos capas)

- **Backend Rust (`src-tauri/`)** — responsable de **todo el I/O**, nunca bloquea la UI. `engine.rs` (`detect`: ccusage global → npx → bunx; `poll_once`: `ccusage blocks --active --json` a **cadencia lenta 15 s**, D13), `burn.rs` (tail JSONL → `tok/s` por respuesta, D17/D27), `sensor.rs` (binario dual + sensor statusline auto-instalado, ver abajo), `tray_icon.rs` (anillo de progreso del icono de bandeja, D30). `engine::history` (`ccusage daily|monthly`) sigue sin implementar (Fase 6).
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

- **Config de ventana en `tauri.conf.json`; permisos en `capabilities/default.json`** (v2). La window tiene `label: "cluster"`, arranca oculta (`visible:false`) — las capabilities se atan a ese label y están recortadas a `core:default`/`core:event:default` (todo el control de tray/ventana ocurre en Rust puro, D24, nunca vía IPC desde JS). `app.macOSPrivateApi: true` es obligatorio para la transparencia en macOS (D14) — no quitarlo.
- **Tray/menu-bar (D24)**: show/hide/posición/hide-on-blur/menú "Salir" viven en `src-tauri/src/main.rs` (`TrayIconBuilder`, feature `tray-icon` del propio crate `tauri`, sin plugin nuevo). El **icono en sí** es un anillo de progreso redibujado en runtime por `tray_icon.rs` (D30), llamado desde `engine.rs`/`sensor.rs` en cada dato nuevo — no un PNG estático (ese PNG, `icons/tray-icon-template.png`, solo queda como icono inicial antes del primer redibujado). **Usar siempre `TrayIcon::set_icon_with_as_template()`, nunca `set_icon()` a secas** — `set_icon()` no conserva el flag "template" de macOS entre llamadas y el icono se repinta negro fijo sin adaptarse a modo claro/oscuro (bug real, D30). `ActivationPolicy::Accessory` solo en macOS (`#[cfg(target_os = "macos")]`); el resto de la API de tray es cross-platform. Solo macOS probado por ahora.
- **Exec desde Rust con `std::process::Command`, NO `tauri-plugin-shell`** (D16). El plugin es para exec desde el frontend JS; nuestro I/O es backend confiable. El motor corre en un `std::thread` dedicado (sin async framework). Cero deps nuevas.
- **`macos-private-api` (feature cargo) va acoplada a `macOSPrivateApi` (conf)**: si tocas una, la otra. El build script de tauri falla si no casan.
- **CSP ya aplicada (D15)**: la política restrictiva de `security.csp` en `tauri.conf.json` está activa desde que aterrizó el primer comando IPC — verificada contra `sensor_status`/`install_sensor`/`set_pinned` en `tauri dev`. No volver a `null`.
- **Puerto dev fijo 1420** (`vite.config.js` + `devUrl`); `clearScreen: false` para no perder logs de Rust.
- **Dependencias fijadas a las últimas estables** por decisión del usuario (D10): no downgradar Vite/Tauri/serde sin motivo.
- **Precisión honesta** (D11): el coste con suscripción es **estimado**; la ventana `rate_limits` es dato **oficial**. No presentar estimaciones como facturación real.
- **Documentación y comentarios en español**; los ADR en `docs/DECISIONS.md` registran el porqué de cada decisión — consultarlos antes de cambiar arquitectura, motor de datos o estética.
