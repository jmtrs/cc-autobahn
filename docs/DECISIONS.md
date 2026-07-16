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
  `~/.claude/projects/**/*.jsonl` (= actividad ahora). Re-scan cada 5 s; al rotar
  sesión se **empieza en EOF** — cero ruido histórico, la aguja arranca en ralentí.
- Tail **por `stat` + `read` cada 1 s en hilo dedicado**, sin `notify`/kqueue.

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
