# Registro de decisiones (ADR)

Decisiones tomadas durante el diseño, con su motivo. Formato ligero.

## D1 — No reinventar el motor de datos

**Decisión**: consumir `ccusage` como fuente de datos, no reimplementar el parseo
de logs ni el pricing.
**Motivo**: parsear JSONL, calcular pricing, deduplicar el bloque de 5 h compartido
y aplicar el multiplicador de Opus es complejo y propenso a errores de facturación.
Ya está resuelto y mantenido. El valor de este proyecto está en la capa visual.

## D2 — ccusage sobre las alternativas

**Decisión**: motor = `ccusage` (ryoppippi).
**Motivo**: es el estándar de facto, el más usado y estable, y expone salida
`--json` limpia. Alternativas evaluadas: Claude-Code-Usage-Monitor (Maciek) y
par-cc-usage (buenas pero menos estándar), ccburn/codeburn (más nuevas).
**Consecuencia**: sin fork. Se ejecuta como proceso hijo y se parsea su JSON.

## D3 — No hacer fork

**Decisión**: cero fork de ccusage ni de ningún monitor.
**Motivo (usuario)**: "no quiero un fork". Mantener el motor externo intacto y
actualizable; nosotros solo la capa encima.

## D4 — Estética: display VFD ámbar W203 (no agujas SVG)

**Decisión**: replicar el display de matriz de puntos ámbar del Mercedes W203.
**Motivo**: las fotos de referencia del usuario (W203) muestran un VFD de texto y
barras de segmentos, no agujas analógicas. Es más auténtico, más ligero y más fácil
que dibujar agujas SVG. Se descartó la idea previa de cluster con agujas analógicas.

## D5 — Tauri sobre Electron

**Decisión**: Tauri v2.
**Motivo**: binario ~5 MB vs ~150 MB, backend Rust nativo para exec/tail,
always-on-top + frameless + transparente nativos, cross-OS real. Usuario: "Tauri
me gusta". Requisito: "no incómodo, muy bien integrado, muy fácil de usar".

## D6 — Widget flotante always-on-top (no TUI, no statusline)

**Decisión**: ventana flotante frameless siempre visible.
**Motivo (usuario)**: "en pantalla visible", "bien integrado", "buen diseño como
coche alemán". La TUI unicode no da el look; el statusline de 1 línea no es un
cluster. Se descartaron ambas como forma principal.
**Nota (no choca con D12)**: aquí se rechaza el statusline como **forma de display**.
D12 lo usa como **sensor de datos** (fuente del JSON oficial), no como UI. Cosas
distintas.

## D7 — Selector PRND = modelo activo

**Decisión**: reinterpretar el selector de marchas P R N D como selector de
**modelo** (O/S/H/F), con el activo iluminado.
**Motivo (usuario)**: el PRND real marca la marcha del automático; lo mapeamos al
modelo en ejecución con su inicial. Effort como "kickdown" debajo.

## D8 — tok/s por respuesta propio (tail de JSONL) — corregido

**Decisión**: mostrar `tok/s` **medio por respuesta** (`Δoutput / Δt_turno`),
calculado nosotros desde el tail del JSONL. Aguja que **salta al completar** cada
turno y decae con muelle a ralentí. NO es una aguja instantánea en tiempo real.
**Motivo (validado empíricamente 2026-07-16)**: se inspeccionó un JSONL activo.
El campo `usage` **no se transmite en streaming**: se estampa **idéntico** en todas
las líneas de un turno y solo aparece **al terminar** el turno (p. ej. un turno de
3008 tokens de salida aterriza de golpe tras 36 s de silencio). El log **no tiene
visibilidad del turno en curso**. Por tanto una aguja "instantánea que reacciona al
pisar" es **físicamente imposible** desde el JSONL — lo máximo honesto es el promedio
por respuesta, renderizado como escalón + decaimiento.
**Consecuencia (D11, honestidad)**: prohibido etiquetar el velocímetro como
"instantáneo". Sigue siendo diferencial (ningún competidor muestra tok/s por
respuesta), pero con la etiqueta verdadera. El streaming real requeriría interceptar
la API o métricas OTEL con streaming → parqueado en Fase 6, opcional.

## D9 — Cero fricción = la app se cablea sola (redefinido)

**Decisión**: cero fricción **no** significa "evitar cablear"; significa que **la
máquina hace la configuración**. El conductor gira la llave, nada más. Aplica a dos
cables: (a) el motor de datos —ccusage global → npx → bunx → botón instalar Bun—, y
(b) el **sensor de statusline** (ver D12), que la app instala sola con un
consentimiento y rollback.
**Motivo**: la lectura literal previa ("el usuario no toca nada") creaba un falso
dilema con el dato oficial de `rate_limits` (solo llega vía statusline, que exige
config). Un Mercedes no estima el depósito: lee el sensor real. Estimar cuando existe
el dato oficial es inaceptable (D11). Pero tampoco se pide al conductor que suelde el
sensor: eso es un defecto de instalación, no el precio a pagar. La app se auto-cablea.
**Consecuencia**: la Fase 4 absorbe el auto-cableado del statusline además del motor.

## D10 — Últimas versiones estables fijadas

**Decisión**: fijar dependencias a las últimas estables (vite 8.1.5, tauri 2.11,
@tauri-apps/cli 2.11.4, api 2.11.1, serde 1.0.228, serde_json 1.0.150).
**Motivo (usuario)**: "solo quiero las últimas y más estables". Se corrigió `vite ^6`
(capaba en 6.x) a `^8.1.5`.

## D11 — Honestidad de precisión

**Decisión**: mostrar que el precio con suscripción es **estimado**; la autonomía
(`rate_limits`) es dato **oficial**; el billing real es la Claude Console.
**Motivo**: transparencia; ccusage documenta que el coste es aproximación.

## D12 — Sensor de statusline auto-instalado (dato oficial sin fricción)

**Decisión**: cc-autobahn **es** el comando de statusline de Claude Code, y se
instala solo. Al primer arranque lee `~/.claude/settings.json`; con **un
consentimiento** ("conectar el sensor"), escribe la clave `statusLine` apuntando a su
propio binario, guardando **backup** del valor previo (reversible). Ese binario, en
cada invocación de Claude Code, hace **dos cosas**: (1) emite a **stdout** la línea
de statusline normal —respetando la que el usuario tuviera, o una por defecto— para
no romper su terminal; (2) escribe el JSON completo (`rate_limits`, `model`,
`effort`, `cost`, `context_window`) a un **socket/fichero** (`$XDG_RUNTIME_DIR` o
`~/.claude/cc-autobahn.sock`) que la ventana **tail**ea.
**Motivo**: el JSON de statusline es **push** (Claude Code lo pasa por stdin solo a
un script configurado); una ventana externa no lo recibe pasivamente. Es la única
fuente del dato **oficial** de la ventana de 5 h / 7 d (`rate_limits`). Renunciar a
él y estimar viola D11; pedir edición manual viola el espíritu de D9. La tercera vía
—wrapper que se auto-configura y respeta lo previo— resuelve ambos.
**Consecuencia**: statusline solo dispara cuando Claude Code renderiza → el cuadro se
ilumina con el motor en marcha y se apaga en ralentí (fiel al coche). Es wrapper, no
secuestro: backup + rollback obligatorios.

## D13 — Cadencias separadas por fuente (no un poll único)

**Decisión**: cada sensor tiene su cadencia; **prohibido** un poll único a 1–2 s.
- `ccusage blocks` (coste/proyección/histórico): poll **lento, 10–30 s**, o proceso
  persistente. El bloque de 5 h no cambia por segundo.
- Tail de JSONL (`tok/s` por respuesta): **evento-dirigido** (al escribirse el log),
  no polling.
- Statusline (`rate_limits`, modelo, effort): **push**, llega cuando Claude Code
  renderiza.
**Motivo**: `npx -y ccusage@latest` cada 1–2 s arranca Node + resuelve paquete cada
tick (cientos de ms, CPU) para un dato que apenas cambia. Derroche. Cadencia = tasa
real de cambio del dato.

## D14 — `macOSPrivateApi` para transparencia real

**Decisión**: `app.macOSPrivateApi: true` en `tauri.conf.json`.
**Motivo**: en macOS, `transparent: true` + `decorations: false` requiere la API
privada para transparencia real; sin ella el fondo se ve negro. Coste asumido: no se
puede publicar en la Mac App Store (irrelevante, distribución directa).

## D15 — CSP diferida al primer IPC (no `null` silencioso)

**Decisión**: `security.csp` sigue `null` **mientras el chasis no tenga IPC ni red**.
Al aterrizar el primer comando Tauri (Fase 1), aplicar una CSP restrictiva y
**verificarla en `tauri dev`** (el websocket de HMR de Vite debe sobrevivir):
`default-src 'self'; img-src 'self' data:; style-src 'self'; script-src 'self';
connect-src 'self' ipc: http://ipc.localhost ws://localhost:1420`
**Motivo**: hoy no hay superficie (sin fetch, sin IPC, sin contenido remoto). Flipear
CSP a ciegas puede romper el HMR y no es verificable sin build. Se documenta la
política exacta y su disparador para no olvidarla, en vez de dejar `null` sin
explicar. Endurecer cuando exista algo que proteger.

## D16 — Exec desde Rust con `std::process`, sin `tauri-plugin-shell`

**Decisión**: ejecutar ccusage con `std::process::Command` en el backend Rust. **No**
se usa `tauri-plugin-shell`.
**Motivo**: el plugin shell existe para invocar procesos desde el **frontend JS** no
confiable (con allowlist en capabilities). Nuestro I/O vive en Rust (confiable), así
que `std::process::Command` basta: cero dependencia, cero capability extra, más
simple y más sólido. Fiel al espíritu W203: mínimas piezas, todas serviciables.
**Consecuencia**: el poll corre en un `std::thread` dedicado con `sleep` (sin async
framework). Revisar solo si en Fase 4 empaquetamos Bun como *sidecar* (eso sí puede
querer el plugin). Corrige el hallazgo previo que daba el plugin por necesario.

## D17 — Sensor de tok/s: turno = secuencia hasta `end_turn`, tail por `stat`

**Decisión**: el sensor `burn` (Fase 2) calcula `tok/s` **por turno completo**, donde
un turno = la secuencia de mensajes `assistant` que cierra en `stop_reason` ∈
{`end_turn`, `stop_sequence`}.

- `Δoutput` = Σ `output_tokens` de los mensajes `assistant` del turno,
  **deduplicados por `message.id`** (las reescrituras traen el mismo valor — contarlo
  una sola vez). Incluye los `tool_use` intermedios, no solo el mensaje final: todo
  es output generado en esa respuesta.
- `Δt_turno` = wall-clock `ts(cierre actual) − ts(cierre anterior)`; si no hay cierre
  previo (al enganchar el fichero a mitad de sesión), desde el primer mensaje
  acumulado. `durationMs` del JSONL es `null` → no hay tiempo de API separable, así
  que el wall-clock incluye el tiempo de ejecución de herramientas (honesto y medible).
- Selección del fichero: el `.jsonl` con mayor `mtime` bajo
  `~/.claude/projects/**/*.jsonl` (= actividad ahora). Re-scan (qué fichero es el
  activo) cada 5 s; al rotar sesión se **empieza en EOF** — cero ruido histórico,
  la aguja arranca en ralentí.
- Tail **por `stat` + `read` cada 200 ms en hilo dedicado** (bajado de 1 s, D27),
  sin `notify`/kqueue.

**Motivo**: medición empírica del JSONL real (2026-07-16, `cargo test` 11/11). El
caso D8 (turno del `end_turn` de 3008 tok + un `tool_use` previo de 583) da
`Δoutput=3591, Δt=65.278 s → 55.0 tok/s`. El `stat` por segundo **no** es el derroche
que D13 prohíbe (eso era spawn de Node por tick): es una syscall trivial. kqueue
exigiría la crate `notify` — rechazada por el principio W203 de mínimas piezas. El
timestamp Zulu se parsea a mano (sin `chrono`): el formato de Claude Code es siempre
`YYYY-MM-DDTHH:MM:SS.mmmZ`. `pos` avanza solo hasta el último `\n` (buffer residual)
→ nunca se pierde una línea por escritura parcial.

**Corrección a D8**: D8 decía literalmente que `usage` se estampa "idéntico en todas
las líneas de un turno". En realidad cada mensaje `assistant` trae su propio `usage`
con sus propios `output_tokens`; lo idéntico son las **reescrituras de un mismo
`message.id`**. La conclusión de D8 se mantiene intacta (no hay streaming; el dato
llega al cerrar el mensaje), solo se afina el mecanismo.

## D18 — Aguja con muelle físico (escalón + decaimiento)

**Decisión**: el velocímetro (`#burn`) no es un valor plano: tras cada `burn-tick` el
`target` salta al `tok/s` del turno y un **spring amortiguado underdamped** lleva la
aguja hasta ahí con overshoot mecánico (`v += (target−pos)·k; v *= damp; pos += v`).
Sin tick fresco durante 2 s, el `target` decae a 0 (ralentí). La lectura secundaria
`#burn-inst` muestra el `tok/s` crudo del último turno, sin muelle.

**Motivo**: fidelidad al cuero W203 (aguja analógica con inercia, no dígito que
salta). Y honestidad (D11): la etiqueta es "tok/s por respuesta", **nunca
"instantáneo"** — la aguja decae porque el dato solo llega al cerrar el turno (D8).

## D19 — Binario dual: mismo bin + early-return (no bin aparte para statusline)

**Decisión**: el comando `statusLine` de Claude Code es el **mismo binario**
`cc-autobahn`. `main` parsea `argv[1]=="statusline"` y retorna **antes** de
construir `tauri::Builder` → no arranca GUI/webview. Medido: **10 ms** por
invocación (debug, 7 runs, p95 < 30 ms).
**Motivo**: partir un `[[bin]]` mínimo separado añade complejidad de workspace y
una lib compartida para ahorrar <30 ms que el early-return ya logra. Si la
cadencia de invocación subiera y el overhead fuera notable, se reconsidera.

## D20 — Path estable: copiar el bin, nunca `current_exe()` en settings

**Decisión**: al instalar, se **copia** el binario a
`${CLAUDE_CONFIG_DIR:-~/.claude}/cc-autobahn/cc-autobahn-statusline` (0755) y se
escribe **ese** path en `settings.json`. Nunca `std::env::current_exe()`.
**Motivo**: en macOS, un `.app` descargado sin notarizar se ejecuta desde
`/private/var/folders/.../AppTranslocation/<hash>/...` (translocación de
Gatekeeper). `current_exe()` devuelve ese path **efímero**; el siguiente arranque
cambia el hash y el statusline apuntaría a la nada. La copia a un path estable
bajo el config dir lo resuelve en dev y release por igual.

## D21 — Chain passthrough del statusLine previo (respeta lo que ya había)

**Decisión**: el modo statusline lee stdin, **re-ejecuta** el `statusLine`
previo del usuario (guardado en `cc-autobahn/prev-statusline`) vía `sh -c` con ese
mismo stdin, reemite su stdout y **además** vuelca el JSON al fichero sensor. Si
no hay prev o falla, línea por defecto.
**Motivo**: D12 promete "respeta la que tuvierera". Claude Code solo invoca un
`command`; el wrapper no recibe el output previo, pero sí puede re-ejecutarlo. Sin
chain, se destruiría silenciosamente cualquier statusline existente (ej. el plugin
caveman). Idempotente: si el statusLine actual ya apunta a nosotros, no nos
capturamos a nosotros mismos como prev (evita un chain recursivo infinito).

## D22 — settings.json solo si parsea como JSON estricto

**Decisión**: la mutación de `settings.json` se hace con round-trip
`serde_json::Value` (nunca struct tipado — no dropear campos desconocidos) y solo
si el fichero parsea como **JSON estricto**. Si tiene comentarios/JSONC o está
malformado → no se toca, CTA "configura manualmente". Backup 0600 sin pisar,
escritura tmp+rename atómica, re-validación post-escritura + rollback.
**Motivo**: Claude Code valida `settings.json` con Zod estricto; un campo mal
escrito deja al usuario sin config. El round-trip con `Value` preserva todo lo que
no tocamos; la validación + rollback evita dejarlo inservible.

## D23 — Métricas honestas y sin DOM nuevo (Pista A vs B)

**Decisión**: la barra `#segments` refleja la **proyección** de ccusage (con marca
`EST`) mientras no haya sensor, y el **% oficial** `rate_limits.five_hour` cuando
lo hay (sin `EST`). `seven_day` no añade DOM: tiñe el borde `.screen` de rojo al
pasar 80 % (testigo de reserva W203). `#odo` muestra tokens del **bloque 5h**, no
acumulados vitalicios (esos requieren `ccusage daily`, Fase 6).
**Motivo**: D11 (no estimar cuando existe oficial) y fidelidad al layout W203. La
conmutación automática entre fuente estimada y oficial prioriza siempre lo oficial
sin añadir elementos ni modos ocultos.
**Bug corregido (post-D24)**: la fuente oficial llenaba los segmentos según
`fiveHourPct` (% **gastado**), al revés que la estimada (`applyEstimated`, que ya
usaba minutos **restantes**) — un depósito que se llena al gastar en vez de
vaciarse, inconsistente entre las dos fuentes y contra el icono de surtidor.
Corregido a `100 - fiveHourPct` en `onSensorUpdate` (`src/main.js`).

## D24 — Tray/menu-bar sustituye la ventana flotante siempre visible (supera D6)

**Decisión**: el cluster deja de ser una ventana flotante always-on-top permanente
y pasa a un icono fijo en la barra de menú de macOS (`TrayIconBuilder`, feature
`tray-icon` del propio crate `tauri` — sin plugin nuevo, D10/D16). Click izquierdo
muestra/oculta un panel anclado bajo el icono (posición calculada a mano desde
`TrayIconEvent::Click { rect, .. }`, sin `tauri-plugin-positioner`); click fuera lo
oculta (`WindowEvent::Focused(false)`); click derecho abre un menú con "Salir de
cc-autobahn". `ActivationPolicy::Accessory` en macOS quita el icono de Dock/Cmd+Tab.
La ventana arranca oculta (`"visible": false`, sin `center`) y conserva
`alwaysOnTop` (para flotar sobre cualquier app mientras esté visible — no es lo
mismo que "posicionado bajo el tray").
**Motivo (usuario)**: ya no quiere arrastrar/mover la ventana a mano; prefiere el
modelo de utilidades de menu-bar (Maccy/Ice/Bartender) — icono siempre accesible,
panel bajo demanda, cero fricción de posicionamiento manual.
**Supera D6**: D6 documentaba la ventana flotante siempre visible como decisión
deliberada ("en pantalla visible", citado del usuario). Se sustituye porque el
propio usuario cambió de preferencia; D6 queda como registro histórico de por qué
se llegó al diseño anterior, no se borra.
**Consecuencia**: `data-tauri-drag-region` se retira de `index.html` (ya no hace
falta arrastrar). `capabilities/default.json` pierde los permisos `core:window:*`
(vestigiales — todo el control de tray/ventana ocurre en Rust puro, nunca vía IPC
desde JS). Se añade un ítem de menú "Salir" porque `ActivationPolicy::Accessory` no
deja icono de Dock para cerrar la app de otro modo. Guard anti-carrera (300 ms)
entre hide-por-blur y click-de-tray para que cerrar el panel clicando el icono no
lo reabra. Solo `set_activation_policy` va tras `#[cfg(target_os = "macos")]` — el
resto es API cross-platform de Tauri v2, Windows/Linux quedan para después sin
requerir cambio de arquitectura.
**Alcance**: solo macOS por ahora. Verificado en vivo (`tauri dev`): icono visible
en barra de menú, panel se ancla correctamente bajo el icono, hide-on-blur
funciona, toggle de cierre por tray no se reabre, menú "Salir" funciona, ausente de
Dock/Cmd+Tab.

## D25 — Esquinas redondeadas vía CALayer nativo (addendum D24)

**Decisión**: el panel usa `objc2-app-kit`/`objc2-quartz-core` para clipear el
`NSWindow` a nivel de `CALayer.cornerRadius` (macOS-only, `#[cfg(target_os =
"macos")]`), en vez de confiar solo en el CSS `border-radius` de `.cluster`.
**Motivo**: con `transparent:true` + `decorations:false`, Tauri/WebKit en macOS
no clipea bien el CSS `border-radius` al canal alpha de la ventana — deja un
"pico" cuadrado en las 4 esquinas (bug conocido y documentado en varios issues
del repo oficial de `tauri-apps/tauri`, sin fix limpio en el framework a día de
hoy). Se descartaron dos vías antes de esta:
1. `overflow: hidden` en `.cluster` — no era un problema de overflow CSS.
2. `window.set_shadow(false)` — no era la sombra nativa la causa.
3. Ventana con borde exterior recto (sin `border-radius`) — funcionaba sin
   artefactos pero perdía la estética redondeada; descartada por preferencia.
**Por qué sin plugin de terceros**: se evaluó `tauri-plugin-mac-rounded-corners`
(cloudworxx), pero no es un crate normal — el instalador copia código fuente
(`mod.rs`) directo al repo, añade el stack legacy `cocoa`/`objc` 0.2.x (FFI
unsafe, duplicado del stack que ya usa Tauri) y trae funciones de "Traffic
Lights" irrelevantes aquí (el panel no tiene botones nativos de ventana).
**Consecuencia**: `objc2` (0.6), `objc2-app-kit` y `objc2-quartz-core` (0.3,
ambos ya resueltos en `Cargo.lock` como deps transitivas de `macos-private-api`
— D10 spirit: se exponen a nuestro código sin añadir versiones nuevas al árbol)
se declaran en `[target.'cfg(target_os = "macos")'.dependencies]`. `main.rs`
llama `content_view.setWantsLayer(true)` + `layer.setCornerRadius(12.0)` +
`layer.setMasksToBounds(true)` una vez en `setup()`. `transparent:true` y
`shadow:true` vuelven a `tauri.conf.json`; el CSS `border-radius:12px` de
`.cluster` se restaura (ahora clipeado correctamente por el layer nativo, debe
coincidir con el radio de 12px). Verificado en vivo: esquinas limpias en las 4,
sin pico, con la ventana transparente + sombra nativa activas.

## D26 — Botón PIN (addendum D24)

**Decisión**: botón "PIN" en el header del panel (`index.html`/`style.css`) que
al activarse desactiva el hide-on-blur (`WindowEvent::Focused(false)` ya no
oculta la ventana mientras esté fijado).
**Motivo (usuario)**: quería poder dejar el panel abierto sin que se cierre al
hacer click fuera, para consultarlo mientras trabaja en otra ventana.
**Consecuencia**: nuevo estado compartido `PinnedState` (`Arc<Mutex<bool>>`)
gestionado por Tauri (`.manage(...)`), nuevo comando `set_pinned` invocado desde
`main.js` (`wirePinButton`). El guard se aplica dentro del propio handler de
`on_window_event`, antes de tocar `last_blur_hide` — si está fijado, ni se oculta
ni se registra el hide, dejando el guard anti-carrera (D24) intacto para cuando
se desactive el PIN.

## D27 — Tick parcial por mensaje intermedio + cadencia de tail a 200 ms

**Decisión**: dos cambios en `burn.rs` para bajar la latencia percibida del
velocímetro tok/s hasta el suelo real que impone D8:
1. `TAIL_INTERVAL_MS` baja de 1000 a 200 ms — el `stat`+`read` de un único
   fichero ya conocido es una syscall trivial; reducirlo no tiene coste real
   (la cadencia de `ACTIVE_RESCAN_SECS = 5 s`, que sí recorre TODOS los
   proyectos, queda intacta).
2. `TurnState::ingest` ahora emite un `burn-tick` **parcial** por cada mensaje
   `assistant` intermedio (p. ej. `tool_use`) que no sea el primero del turno,
   con el tok/s de SOLO ese mensaje sobre el Δt desde el mensaje anterior — sin
   esperar al `end_turn`/`stop_sequence` final. El tick agregado de cierre de
   turno (con el total del turno) se mantiene exactamente igual que antes.
**Motivo (usuario)**: en una respuesta de una sola pieza (sin herramientas) NO
hay nada que hacer — el JSONL solo tiene el dato al terminar esa única escritura
(D8, validado 2026-07-16, no es una cadencia ajustable). Pero en turnos con
varias llamadas a herramientas (la mayoría del trabajo de código real: Read,
Edit, Bash) SÍ hay varios mensajes escritos progresivamente antes del cierre —
esperar al final entero desperdiciaba esa información ya disponible en disco.
**Por qué el primer mensaje del turno no emite tick parcial**: su Δt contra sí
mismo es 0 (nada que medir todavía); el segundo mensaje en adelante sí tiene un
Δt real desde el anterior. Verificado con test dedicado
(`intermediate_tool_use_emits_partial_tick`) y contra los 24 tests previos, que
siguen pasando sin modificación (cambio aditivo, no reemplaza el tick final).
**Consecuencia**: el payload `burn-tick` ahora puede llegar más seguido en
turnos largos con herramientas; el frontend no cambia el velocímetro (ya trata
cada tick igual: salto de aguja). El footer "ÚLT tok/s" que leía este mismo
payload fue sustituido por PACE/AUTO (D28) precisamente porque D27 lo volvió
ambiguo (turno completo vs. mensaje intermedio, sin marca que lo distinga).
`ACTIVE_RESCAN_SECS` sigue en 5 s, sin tocar.

## D28 — Footer: PACE (ritmo reciente) / AUTO (autonomía ajustada al ritmo)

**Decisión**: el footer "ÚLT tok/s" (D26 lo etiquetó, D27 lo volvió ambiguo) se
sustituye por dos métricas nuevas, alternables con click y persistidas en
`localStorage` (clave `cc-autobahn.footerMetric`, primera vez que el proyecto
usa Web Storage):
- **PACE**: `▲/▼ N%` — diferencia entre el ritmo de los últimos 5 min
  (`Σ turnOutputTokens` de los `burn-tick` recibidos, sobre el span real
  cubierto) y la media de OUTPUT del bloque, calculada a mano con
  `block.tokenCounts.outputTokens / minutos transcurridos` (ver corrección más
  abajo — NO usa `burnRate.tokensPerMinute` de ccusage). `—` si no hay ticks
  recientes o no hay bloque activo.
- **AUTO**: minutos restantes reproyectando la TENDENCIA reciente de
  `rate_limits.five_hour.used_percentage` (Δ%/Δt de los últimos 10 min, mínimo
  2 muestras separadas ≥2 min) — NO la proyección lineal de ccusage. `—` sin
  sensor conectado, sin muestras suficientes, o ritmo ≤0.
**Motivo (usuario)**: el footer antiguo no aportaba frente al velocímetro y
quedó ambiguo tras D27. Se pidieron métricas de verdad útiles: cuánto se está
gastando AHORA comparado con la media (PACE), y una autonomía tipo "range to
empty" que se ajuste al ritmo real en vez de una proyección fija (AUTO).
**Por qué AUTO es solo-sensor**: verificado leyendo el código fuente real de
`ccusage` v20 (Rust, `gh api repos/ccusage/ccusage/.../blocks.rs`,
`project_block_usage`): `projection.remainingMinutes` = `block.end_time −
now()`, **puro reloj**, no depende del ritmo de consumo en absoluto.
Reproyectar esa cantidad por ritmo no tendría sentido matemático (D11: no
estimar/inventar donde el dato no lo sustenta). Solo `rate_limits.five_hour`
(oficial) mide consumo real de cupo, así que solo ahí la reproyección es
honesta.
**Corrección (probado en vivo el mismo día)**: el diseño inicial reusaba
`burnRate.tokensPerMinute` de ccusage como media del bloque. Probando con datos
reales, PACE se quedaba clavado en `▼ -100%` pese a actividad real (turnos de
3438, 784, 3625 tokens de output). Causa, confirmada contra
`TokenCounts::total()` en el código fuente de ccusage: `tokensPerMinute` suma
`input + output + cache_creation + cache_read` — y `cache_read_tokens` puede
ser enorme en sesiones largas (reuso de contexto cacheado en cada llamada),
inflando el denominador muy por encima del `output_tokens` puro que mide
`burn-tick`. Comparar "reciente (solo output)" contra "media (input+output+
caché)" es comparar magnitudes distintas — el resultado da siempre cerca de
-100% sin importar el ritmo real. **Corregido**: la media del bloque se
calcula a mano con `block.tokenCounts.outputTokens / minutos transcurridos`
(mismo `startTime` que ya usa `session-time`) — misma magnitud que
`burn-tick.turnOutputTokens`, comparación coherente. Lección: verificar una
fórmula ajena contra datos reales antes de confiar en que mide lo mismo que
uno cree, no solo contra el código fuente en abstracto. Confirmado además con
`npx ccusage blocks --active --json` en vivo: `tokensPerMinute` llegó a
**1 872 536** (dominado por `cacheReadInputTokens: 37 386 004`) frente a
`outputTokens: 46 631` reales — la magnitud del error habría sido de ~40x, no
un matiz menor. (ccusage también expone `tokensPerMinuteForIndicator`
—input+output, sin caché— pero sigue mezclando input con output; se descarta
igualmente por no ser la misma magnitud que `burn-tick`, que es 100% output.)
**Corrección 2 (misma revisión, con datos reales del sensor)**: `computeAdjustedAutonomy`
no tenía techo — con datos reales (`five_hour.used_percentage: 85`, reset en
16 min reales) se confirmó que un ritmo lento podría reproyectar MÁS
autonomía de la que existe de verdad (la ventana resetea en su hora fija pase
lo que pase con el %). **Corregido**: `minutesLeft = min(reproyección,
minutos_reales_hasta_fiveHourResetsAtMs)` — techo duro contra el dato oficial
de reset, que es 100% cierto.
**Corrección 3**: `recentTicks` (buffer de PACE) no se limpiaba al rotar el
bloque de 5h — si la rotación ocurre dentro de los últimos 5 min de buffer,
"reciente" podía mezclar tokens del bloque viejo con la media del nuevo.
**Corregido**: se limpia el buffer cuando `block.id` cambia (`onBlocksUpdate`).
**Corrección 4**: `formatHMin` redondeaba horas y minutos por separado
(`floor(min/60)` + `round(min%60)`), lo que podía dar `m=60` (ej. 119.5 min →
"1h60" en vez de "2h00"). **Corregido**: redondear una sola vez a minuto
entero antes de partir en h/m.
**Corrección 5**: `computePace` no tenía guardas de "datos insuficientes"
análogas a las de AUTO — un bloque recién empezado (elapsed≈0) o un solo tick
muy reciente (span≈0) podían inflar el ratio artificialmente por división
entre casi-cero. **Corregido**: mínimo 1 min de bloque transcurrido y mínimo
30 s de span de ticks antes de calcular, si no `—`.
**Corrección 6 (barra de autonomía "surtidor", no PACE/AUTO)**: hallada con
capturas reales del usuario: la barra oficial mostraba "0h17" (85% usado) y,
tras una pausa normal (Claude Code sin renderizar un rato — el sensor lo
marca "desconectado" a los 60 s, `STALE_SECS`, `sensor.rs`), saltaba a
"EST 4h31" — la proyección de ccusage, un sistema de ventana de 5h
**independiente** del oficial (`rate_limits`). El salto entre los dos era un
número sin sentido, no solo estético. **Corregido**: nuevo flag pegajoso
`everSensorConnected` — una vez que hubo dato oficial alguna vez, una
desconexión momentánea ya NO cae a la proyección de ccusage; se **congela**
tal cual (`onBlocksUpdate`/`onSensorState` dejan de tocar
segments/autonomie/gear/kick/warn) y la cuenta atrás sigue viva con el último
`fiveHourResetsAtMs` conocido (`refreshAutonomie` ya no depende de
`sensorConnected`, solo de tener un reset válido — ese dato no deja de ser
cierto solo porque el sensor calle un rato). El fallback a "EST" de ccusage
queda reservado exclusivamente para cuando el sensor NUNCA se conectó.
**Idioma de UI**: las etiquetas visibles (`PACE`, `AUTO`) van en inglés,
coherente con el resto del cluster (`AFTER START`, `tok/s`, `Mtok`) — la regla
de comentarios en español de CLAUDE.md aplica a código/documentación, no al
copy del display.
**Sin colisión con Fase 6** (`docs/ROADMAP.md`): ambas métricas usan datos ya
emitidos hoy vía `blocks-update`/`sensor-update` (`Block.tokenCounts`,
`rate_limits`); Fase 6 es sobre `ccusage daily/monthly`, dato histórico
distinto, no tocado aquí.

## D29 — Kickdown (indicador de effort) retirado del selector

**Decisión**: se retira el elemento `.kick` (`#kick`, cuatro barritas `▂▂▂▂`
que representaban `effort.level`) del selector PRND. Eliminado sin dejar
vestigios: `index.html` (`<span class="kick">`), `style.css` (`.gear .kick`),
`main.js` (`KICK_FULL`, `EFFORT_BARS`, `setKick()` y sus dos call sites en
`onSensorUpdate`/`onSensorState`) — verificado con `grep -i kick` sobre los
tres ficheros tras el cambio, cero coincidencias.
**Motivo (usuario)**: feedback directo sobre captura del panel real: "las tres
barritas esas horizontales yo las quitaria, no aportan nada".
**Consecuencia**: `effort.level` sigue llegando en el payload de
`sensor-update` (`SensorUpdate.effortLevel`, `sensor.rs`, no tocado) — solo se
dejó de pintar, no de emitir; retomable sin cambios de backend si hiciera
falta. D7 y D28 documentan el kickdown como parte del diseño original y del
estado congelado respectivamente en su momento — quedan como registro
histórico sin editar retroactivamente (mismo criterio que D24 sobre D6).

## D30 — Icono de bandeja como anillo de progreso (reemplaza disco estático)

**Decisión**: el icono de menu-bar (D24) deja el PNG estático fijo (disco
relleno, generado por `scripts/make-tray-icon.mjs` → `tray-icon-template.png`)
y pasa a un **anillo de progreso redibujado en runtime**
(`src-tauri/src/tray_icon.rs`, módulo nuevo), pixel a pixel y sin
dependencias de dibujo — mismo patrón manual que `make-tray-icon.mjs` (D16:
cero deps nuevas). Representa el **% restante** de la ventana de 5h con el
mismo criterio que `#segments` en el panel (depósito que se vacía, no que se
llena, D23): pista tenue siempre visible (alpha 55/255) + arco opaco (alpha
255) trazado desde las 12 en punto, en sentido horario. Se redibuja en cada
dato nuevo, en el mismo punto donde ya se emitía el evento correspondiente:
- `engine.rs` (poll ~15s): `remaining_minutes / WINDOW_MIN * 100` de la
  proyección de ccusage; sin bloque activo → anillo lleno (100%).
- `sensor.rs` (push oficial): `100 - five_hour_pct`.

Sin lógica de precedencia estimado-vs-oficial replicada del frontend (D23):
el tray es un vistazo de bajo compromiso, gana el último dato que llegue de
cualquiera de las dos fuentes — simplificación deliberada, no un descuido.
**Motivo (usuario)**: "el icono es un poco malo, deberiamos poner algo que
sirva para algo" → dato real en vez de decoración fija; "como un cargador
circular ... que se vaya actualizando cada poco tiempo" — mismo lenguaje
visual que un anillo probado antes para el gauge del panel (descartado ahí
porque el icono de gasolina "estaba bien"; el concepto sí encajaba en el
tray).
**Bug encontrado y corregido (verificado en vivo, `tauri dev`)**:
`TrayIcon::set_icon()` **no conserva el flag "template" de macOS** entre
llamadas — cada redibujado se repintaba como imagen de color normal (negro
fijo), sin adaptarse a modo claro/oscuro de la barra de menú (confirmado
visualmente por el usuario: "se ve oscuro si tengo el fondo oscuro, se
deberia adaptar al tema"). La documentación de Tauri lo insinúa de pasada
("calling set_icon followed by set_icon_as_template causes a visible
flicker") pero no deja claro que haga falta en **cada** frame, no solo una
vez al construir el tray. **Corregido**: usar
`TrayIcon::set_icon_with_as_template(icon, true)` (fija imagen + flag
atómicamente, pensado por Tauri precisamente para evitar el flicker de las
dos llamadas separadas) en cada invocación de `set_progress()`, no solo
`.icon_as_template(true)` una vez en el `TrayIconBuilder` inicial.
**Consecuencia**: `app.manage(tray)` guarda el handle `TrayIcon<Wry>` tras
`.build()` para poder recuperarlo desde `engine.rs`/`sensor.rs` vía
`app.try_state::<TrayIcon<Wry>>()`, sin acoplar esos módulos al código de
construcción del tray en `main.rs`. `tray-icon-template.png` y
`scripts/make-tray-icon.mjs` se mantienen como icono inicial (requerido por
`TrayIconBuilder::icon()` antes de tener datos), sobreescrito casi de
inmediato por el primer `set_progress(100.0)` en `setup()`. Sin deps nuevas
(D16): `Image::new_owned(rgba, w, h)` construye la imagen desde bytes crudos,
sin pasar por el decodificador PNG (el feature `image-png` ya presente solo
hacía falta para el `.icon(bytes)` estático original).
**Verificado**: `cargo test` 26/26, `cargo clippy` sin warnings, confirmado
visualmente por el usuario en `tauri dev` tras el fix del flag template.
