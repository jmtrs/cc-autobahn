// Icono de bandeja como anillo de progreso (D-review: "el punto blanco no
// sirve para nada, que sea como el anillo de autonomía").
// Redibuja el PNG a pixel cada vez que llega un dato nuevo de autonomía
// (engine::poll o sensor::tail) — mismo criterio "restante" que el gauge de
// segmentos del panel: 100% = ventana de 5h llena, 0% = agotada. Sin deps de
// dibujo: mismo patrón a mano que scripts/make-tray-icon.mjs, pero en
// runtime y con hueco (anillo, no disco) para poder pintar el arco.
use std::f64::consts::TAU;

use tauri::tray::TrayIcon;
use tauri::{AppHandle, Manager, Wry};

const S: u32 = 44; // 22pt @2x retina, igual que el icono estático anterior
const OUTER_R: f64 = S as f64 * 0.42;
const INNER_R: f64 = S as f64 * 0.28;
const TRACK_ALPHA: u8 = 55; // pista tenue, siempre visible (referencia del 100%)
const ARC_ALPHA: u8 = 255; // arco de progreso, opaco

/// Ventana de facturación de 5h en minutos — igual que WINDOW_MIN en main.js.
pub const WINDOW_MIN: f64 = 300.0;

/// Redibuja el icono de bandeja con el anillo al `pct_remaining` dado
/// (0–100, se clampa). Si el tray aún no está gestionado (arranque muy
/// temprano) no hace nada — se reintentará en el siguiente tick.
pub fn set_progress(app: &AppHandle, pct_remaining: f64) {
    let Some(tray) = app.try_state::<TrayIcon<Wry>>() else {
        return;
    };
    let pct = pct_remaining.clamp(0.0, 100.0);
    let image = tauri::image::Image::new_owned(render(pct), S, S);
    // set_icon() por sí solo NO conserva el flag "template" de macOS (el icono
    // se repinta como imagen normal, negro fijo, sin adaptarse a modo claro/
    // oscuro — bug hallado en revisión visual). set_icon_with_as_template()
    // fija ambos atómicamente en cada redibujado.
    let _ = tray.set_icon_with_as_template(Some(image), true);
}

/// RGBA crudo del anillo: negro opaco en el arco recorrido, negro tenue en
/// el resto de la pista. En modo template (D24) macOS ignora el RGB y usa
/// el alpha como máscara — el alpha bajo de la pista se ve como "atenuado".
fn render(pct: f64) -> Vec<u8> {
    let cx = S as f64 / 2.0;
    let cy = S as f64 / 2.0;
    let sweep = pct / 100.0 * TAU;
    let mut buf = vec![0u8; (S * S * 4) as usize];
    for y in 0..S {
        for x in 0..S {
            let dx = x as f64 + 0.5 - cx;
            let dy = y as f64 + 0.5 - cy;
            let d = (dx * dx + dy * dy).sqrt();
            if !(INNER_R..=OUTER_R).contains(&d) {
                continue; // fuera del anillo: transparente (buf ya está a 0)
            }
            // Ángulo desde arriba (12 en punto), horario, en [0, TAU).
            let mut angle = dx.atan2(-dy);
            if angle < 0.0 {
                angle += TAU;
            }
            let on = angle <= sweep;
            let i = ((y * S + x) * 4) as usize;
            buf[i + 3] = if on { ARC_ALPHA } else { TRACK_ALPHA };
        }
    }
    buf
}
