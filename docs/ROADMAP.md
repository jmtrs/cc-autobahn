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
      `~/.claude/projects/**/*.jsonl`, EOF-start; `stat`+`read` del fichero activo
      cada 200 ms, re-scan de qué fichero es el activo cada 5 s (D17).
- [x] Cálculo `Δoutput / Δt_turno` al cerrar turno (`end_turn`/`stop_sequence`,
      dedup por `message.id`) → evento `burn-tick` (D17). `cargo test` 25/25
      contra JSONL real (caso D8 = 55.0 tok/s verificado).
- [x] Tick **parcial** por mensaje intermedio (`tool_use`, etc.) en turnos con
      herramientas, sin esperar al cierre final del turno (D27).
- [x] Velocímetro en el frontend: spring amortiguado (escalón + overshoot) +
      decaimiento con "muelle" a ralentí; etiqueta honesta, no "instantáneo" (D18).

## Fase 3 — Sensor statusline + cablear display ✅

- [x] **Pista A — cablear `blocks-update`** (frontend puro): `#odo`, `#session-time`,
      `#avg`, `#autonomie` (EST), `#segments` (proyección), `.gear` desde `models[]`.
- [x] `engine::sensor` (`sensor.rs`) — modo dual del binario (`statusline` →
      early-return, 10 ms; D19), chain del statusLine previo (D21), fichero sensor
      escrito atómicamente, tail en hilo dedicado cada 2 s → `sensor-update`/
      `sensor-state`.
- [x] Sustituir placeholders por datos vivos (odómetro/trip/coste por `blocks`;
      barra/gear/effort por el sensor).
- [x] Barra de segmentos = autonomía **oficial** `rate_limits.five_hour`
      (conmuta sobre la estimada, D23).
- [x] Selector PRND = `model.id`. Kickdown (`effort.level` como barritas) se
      implementó y luego se **retiró** por feedback visual — no aportaba (D29).
- [x] `seven_day` como tinte de borde `.screen` al pasar 80 % (D23, sin DOM nuevo).
- [x] **Auto-instalación** del sensor: `install_sensor`/`uninstall_sensor`/
      `sensor_status` (round-trip `Value`, backup+rollback, copia bin estable D20,
      JSON estricto D22) + UI de consentimiento con preview diff.
- [x] CSP restrictiva aplicada y verificada (D15).

## Fase 4 — Cero fricción (auto-cableado, D9)

- [ ] Pantalla "CHECK ENGINE" cuando falta el motor.
- [ ] Botón "INSTALAR MOTOR" (descarga Bun / `bunx ccusage`).
- [ ] **Auto-instalar sensor statusline** (D12): escribir `statusLine` en
      `~/.claude/settings.json` con consentimiento + backup + rollback.
- [ ] (Opc.) Empaquetar Bun como sidecar de Tauri.

## Fase 4.5 — Tray/menu-bar (D24, adelantada, hecha)

- [x] Icono de menu-bar (`TrayIconBuilder`, feature `tray-icon`, sin plugin nuevo).
- [x] Icono dinámico: anillo de progreso (% ventana 5h restante) redibujado en
      runtime desde `engine`/`sensor`, sin deps de dibujo — reemplaza el PNG
      estático inicial (D30).
- [x] `ActivationPolicy::Accessory` en macOS — sin Dock ni Cmd+Tab.
- [x] Click izquierdo muestra/oculta panel anclado bajo el icono (posición desde
      `TrayIconEvent::rect`, clamp contra bordes de pantalla).
- [x] Hide-on-blur (`WindowEvent::Focused(false)`) + guard anti-carrera 300 ms
      (cerrar clicando el icono no lo reabre).
- [x] Menú contextual (click derecho) con "Salir de cc-autobahn".
- [x] `data-tauri-drag-region` retirado; capabilities recortadas a
      `core:default`/`core:event:default`.
- [ ] (Futuro) Windows/Linux — API es cross-platform salvo `set_activation_policy`
      (solo macOS), pendiente de probar en esos SO.

## Fase 5 — Integración y pulido

- [x] Bandeja del sistema (show/hide, salir) — ver Fase 4.5 / D24.
- [x] Footer PACE/AUTO (ritmo reciente vs. medio del bloque; autonomía
      ajustada al ritmo, solo sensor oficial) — sustituye "ÚLT tok/s" (D28).
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
