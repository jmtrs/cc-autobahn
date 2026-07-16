# Diseño visual — Mercedes W203

## Referencia

El **Kombiinstrument del Mercedes W203** (2000–2007): display central **VFD de
matriz de puntos**, monocromo **ámbar/naranja** sobre negro. NO son agujas
analógicas — es texto y barras de segmentos luminosos. Elementos de referencia:

- Ordenador de viaje "AFTER START": `46 Km`, `0:40 h`, `67 Km/h`, `6.4 L/100Km`,
  velocidad grande abajo-izquierda, reloj abajo-derecha.
- Odómetro ámbar: total km, cuentakilómetros parcial, temperatura, hora.
- Gauge de refrigerante: **barra horizontal de segmentos** `40 · 80 · 120 °C`.
- Selector de marchas automático **P R N D** en marco lateral, marcha activa
  iluminada.

## Mapeo coche → tokens

| Elemento W203              | Métrica cc-autobahn                              |
| -------------------------- | ----------------------------------------------- |
| Velocímetro (Km/h)         | `tok/s` **por respuesta** (`Δoutput / Δt_turno`, D8) |
| Consumo (L/100 Km)         | Coste medio `$/Mtok`                            |
| Autonomía / depósito ⛽    | Ventana de 5 h restante (barra de segmentos)    |
| Trip "AFTER START"         | Tokens y tiempo desde el último reset            |
| Odómetro                   | Tokens totales acumulados                        |
| Selector PRND              | **Modelo activo** (O/S/H/F), iluminado          |
| Kickdown (patada gas)      | Effort level (low/med/high/max)                  |
| Reloj                      | Hora real                                        |
| Barra refrigerante         | Ventana semanal (7 días) — variante secundaria   |

## Selector de modelo (reinterpretación del PRND)

El PRND del automático marca la **marcha activa** iluminada. Nosotros marcamos
el **modelo en uso**, con su inicial:

```
┌─┐
│O│  Opus     ← activo: ámbar brillante + glow
│S│  Sonnet   ← inactivo: ámbar tenue
│H│  Haiku
│F│  Fable
└─┘
```

- Modelo activo = `--amber-glow` a full brightness.
- Resto = `--amber-dim`.
- Dato: `model.id` del JSON de statusline / ccusage.
- **Effort** debajo, como kickdown: `▪▪▪▪` (max = pisar a fondo).

## Layout del cluster

```
┌─────────────────────────────────────┐
│        AFTER START            ┌─┐   │
│                               │O│   │
│   1.24M tok      0:40 h        │S│   │
│                               │H│   │
│   4.1k tok/s    $0.42/Mtok    │F│   │
│  106 tok/s ················· 16:57  │
├─────────────────────────────────────┤
│ ⛽ ▐███████░░░░░  3h12         62%  │
└─────────────────────────────────────┘
```

## Paleta

| Variable       | Valor      | Uso                                  |
| -------------- | ---------- | ------------------------------------ |
| `--amber`      | `#ff9a1f`  | ámbar principal                      |
| `--amber-glow` | `#ffb347`  | resaltado / dígitos grandes / activo |
| `--amber-dim`  | `#7a3d08`  | segmentos apagados / modelo inactivo |
| `--bg`         | `#0a0705`  | cristal del display                  |
| `--bezel`      | `#17120d`  | marco alrededor                      |

## Detalles de estilo (efecto VFD)

- Fondo casi negro con degradado radial suave (glow superior).
- `text-shadow` ámbar para simular fósforo/emisión.
- **Scanlines**: `repeating-linear-gradient` + `mix-blend-mode: multiply`.
- `font-variant-numeric: tabular-nums` → dígitos que no bailan.
- `letter-spacing` amplio, mayúsculas en etiquetas.
- Barra de segmentos: divs `.seg` / `.seg.on`, gap de 2 px (look segmentado).

## Hecho

- Curva de easing de la aguja/velocímetro: muelle amortiguado con overshoot
  (D18), no una interpolación lineal.

## Ideas aparcadas (fuera del roadmap activo, ver `docs/ROADMAP.md`)

- **Fuente dot-matrix real** (hoy: monospace del sistema + glow). Candidata:
  fuente de 5×7 puntos embebida como woff2 local (sin CDN, offline).
- Zona roja al final del velocímetro con burn rate alto.
- Modo compacto (solo velocímetro + autonomía) para barra estrecha.

Se sacaron del checklist de Fase 5 sin decisión documentada (ADR) de por qué
— si se retoman, registrar el motivo en `docs/DECISIONS.md` antes de picar código.
