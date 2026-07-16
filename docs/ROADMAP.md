# Roadmap

Orden de implementación, una capa cada vez, verificando antes de avanzar.

## Fase 0 — Chasis ✅ (actual)

- [x] Scaffold Tauri v2 (frameless, always-on-top, transparente, arrastrable).
- [x] `macOSPrivateApi: true` para transparencia real en macOS (D14).
- [x] Skin ámbar VFD W203 estático (`index.html` + `style.css`).
- [x] Reloj vivo + barra de segmentos (shell, sin datos de tokens).
- [x] Permisos v2 (`capabilities/default.json`).
- [x] Icono ámbar generado (`scripts/make-icon.mjs` → `tauri icon`).
- [x] Dependencias fijadas a últimas estables.
- [x] Docs (este directorio).

## Fase 1 — Motor de datos ✅

- [x] Exec con `std::process::Command` desde Rust, sin plugin shell (D16).
- [x] `engine::detect` — recorre `$PATH`: ccusage global → npx → bunx → ninguno.
- [x] `engine::poll_once` — `ccusage blocks --active --json`, parseo serde,
      evento `blocks-update` cada **15 s** (D13). Hilo dedicado, sin panics.
- [x] Modelos serde contra el JSON real de ccusage v20 (tokens, coste, burn, proy.).
- [x] Manejo de errores: motor ausente → `engine-missing`; fallo puntual →
      `engine-error`; sin bloque activo → `blocks-idle`.
- [x] Frontend escucha eventos (guardado fuera de Tauri); log en Fase 1.
- [ ] Aplicar CSP restrictiva y verificar HMR en `tauri dev` (D15) — pendiente.

## Fase 2 — tok/s por respuesta ✅

- [x] **Validar premisa** (2026-07-16): el JSONL solo reporta `usage` al terminar el
      turno → aguja instantánea imposible; se redefine a **por respuesta** (D8).
- [x] `engine::burn` (`burn.rs`) — tail del JSONL más reciente en
      `~/.claude/projects/**/*.jsonl`, EOF-start, re-scan cada 5 s (D17).
- [x] Cálculo `Δoutput / Δt_turno` al cerrar turno (`end_turn`/`stop_sequence`,
      dedup por `message.id`) → evento `burn-tick` (D17). `cargo test` 11/11
      contra JSONL real (caso D8 = 55.0 tok/s verificado).
- [x] Velocímetro en el frontend: spring amortiguado (escalón + overshoot) +
      decaimiento con "muelle" a ralentí; etiqueta honesta, no "instantáneo" (D18).

## Fase 3 — Sensor statusline + cablear display

- [ ] `engine::sensor` — tail del socket donde el binario statusline vuelca el JSON.
- [ ] Sustituir placeholders por datos vivos (odómetro, trip, coste).
- [ ] Barra de segmentos = autonomía **oficial** (`rate_limits.five_hour`, vía sensor).
- [ ] Selector PRND = `model.id`; kickdown = `effort.level`.
- [ ] Aguja/valor semanal (`rate_limits.seven_day`).

## Fase 4 — Cero fricción (auto-cableado, D9)

- [ ] Pantalla "CHECK ENGINE" cuando falta el motor.
- [ ] Botón "INSTALAR MOTOR" (descarga Bun / `bunx ccusage`).
- [ ] **Auto-instalar sensor statusline** (D12): escribir `statusLine` en
      `~/.claude/settings.json` con consentimiento + backup + rollback.
- [ ] (Opc.) Empaquetar Bun como sidecar de Tauri.

## Fase 5 — Integración y pulido

- [ ] Bandeja del sistema (show/hide, salir).
- [ ] Recordar posición/tamaño de la ventana.
- [ ] Fuente dot-matrix real (woff2 local, offline).
- [ ] Modo compacto (barra estrecha).
- [ ] Zona roja del velocímetro con burn alto.

## Fase 6 — Histórico (opcional)

- [ ] Vista semanal/mensual (`ccusage daily|monthly --json`).
- [ ] (Opc.) Integración OTEL → Prometheus/Grafana para tok/s real y dashboards.

## Verificación por fase

Tras cada fase: `npm run tauri dev`, comprobar que el cluster arranca y los datos
nuevos aparecen sin romper lo anterior. Primer `cargo build` es lento (compila
webview); es normal.
