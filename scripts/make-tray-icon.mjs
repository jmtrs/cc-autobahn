// Generates the macOS menu-bar icon: a monochrome "template" PNG
// (black silhouette + alpha, no color) so macOS tints it according to
// light/dark mode (D24). Zero-dependency PNG writer, same pattern as
// make-icon.mjs. Run:
//   node scripts/make-tray-icon.mjs
import { deflateSync } from "node:zlib";
import { writeFileSync } from "node:fs";

const S = 44; // 22pt @2x retina icon for the menu bar

// Raw RGBA, one filter byte (0) per row
const raw = Buffer.alloc(S * (1 + S * 4));
const cx = S / 2;
const cy = S / 2;
for (let y = 0; y < S; y++) {
  const rowStart = y * (1 + S * 4);
  raw[rowStart] = 0; // filtro: ninguno
  for (let x = 0; x < S; x++) {
    // same disc silhouette (fuel-drop) as the app icon, in template
    // mode: opaque black where it's "on", transparent where it's not.
    const d = Math.hypot(x - cx, y - cy);
    const on = d < S * 0.34;
    const i = rowStart + 1 + x * 4;
    raw[i] = 0;
    raw[i + 1] = 0;
    raw[i + 2] = 0;
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
  new URL("../src-tauri/icons/tray-icon-template.png", import.meta.url),
  png
);
console.log("wrote src-tauri/icons/tray-icon-template.png (44x44)");
