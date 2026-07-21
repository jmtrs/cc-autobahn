// Generates the Linux tray icon: an amber VFD disc (same fuel-drop
// silhouette as make-tray-icon.mjs, but with real RGB instead of the
// macOS alpha-only "template" mask). Linux/AppIndicator has no template
// concept, so the macOS template PNG renders as solid black there. Same
// amber (0xFF,0xB0,0x00) the runtime ring painter uses in tray_icon.rs
// (D55), so the cold-launch icon matches the first redraw. Zero-dep PNG
// writer. Run:
//   node scripts/make-tray-icon-linux.mjs
import { deflateSync } from "node:zlib";
import { writeFileSync } from "node:fs";

const S = 44; // 22pt @2x tray icon
const AMBER_R = 0xff;
const AMBER_G = 0xb0;
const AMBER_B = 0x00;

// Raw RGBA, one filter byte (0) per row
const raw = Buffer.alloc(S * (1 + S * 4));
const cx = S / 2;
const cy = S / 2;
for (let y = 0; y < S; y++) {
  const rowStart = y * (1 + S * 4);
  raw[rowStart] = 0; // filtro: ninguno
  for (let x = 0; x < S; x++) {
    // same disc silhouette (fuel-drop) as the app icon, painted amber.
    const d = Math.hypot(x - cx, y - cy);
    const on = d < S * 0.34;
    const i = rowStart + 1 + x * 4;
    raw[i] = AMBER_R;
    raw[i + 1] = AMBER_G;
    raw[i + 2] = AMBER_B;
    raw[i + 3] = on ? 255 : 0;
  }
}

function chunk(type, data) {
  const len = Buffer.alloc(4);
  len.writeUInt32BE(data.length, 0);
  const typeBuf = Buffer.from(type, "ascii");
  const crc = Buffer.alloc(4);
  crc.writeUInt32BE(crc32(Buffer.concat([typeBuf, data])) >>> 0, 0);
  return Buffer.concat([len, typeBuf, data, crc]);
}

function crc32(buf) {
  let c = ~0;
  for (let i = 0; i < buf.length; i++) {
    c ^= buf[i];
    for (let k = 0; k < 8; k++) c = (c >>> 1) ^ (0xedb88320 & -(c & 1));
  }
  return ~c;
}

const sig = Buffer.from([137, 80, 78, 71, 13, 10, 26, 10]);
const ihdr = Buffer.alloc(13);
ihdr.writeUInt32BE(S, 0);
ihdr.writeUInt32BE(S, 4);
ihdr[8] = 8; // bit depth
ihdr[9] = 6; // color type RGBA
const png = Buffer.concat([
  sig,
  chunk("IHDR", ihdr),
  chunk("IDAT", deflateSync(raw)),
  chunk("IEND", Buffer.alloc(0)),
]);

writeFileSync(
  new URL("../src-tauri/icons/tray-icon-linux.png", import.meta.url),
  png
);
console.log("wrote src-tauri/icons/tray-icon-linux.png (44x44 amber)");
