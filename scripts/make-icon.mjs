// Generates a 512x512 amber source PNG for Tauri's icon generator.
// Zero-dependency PNG writer (uses Node's built-in zlib). Run:
//   node scripts/make-icon.mjs
// then fan out all platform icons with:
//   npx @tauri-apps/cli icon scripts/source-icon.png
import { deflateSync } from "node:zlib";
import { writeFileSync } from "node:fs";

const S = 512;
const bg = [10, 7, 5]; // near-black display glass
const amber = [255, 154, 31];

// raw RGBA, one filter byte (0) per row
const raw = Buffer.alloc(S * (1 + S * 4));
const cx = S / 2;
const cy = S / 2;
for (let y = 0; y < S; y++) {
  const rowStart = y * (1 + S * 4);
  raw[rowStart] = 0; // filter: none
  for (let x = 0; x < S; x++) {
    // draw a fuel-drop-ish amber disc on dark glass
    const d = Math.hypot(x - cx, y - cy);
    const on = d < S * 0.34;
    const [r, g, b] = on ? amber : bg;
    const i = rowStart + 1 + x * 4;
    raw[i] = r;
    raw[i + 1] = g;
    raw[i + 2] = b;
    raw[i + 3] = 255;
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

writeFileSync(new URL("./source-icon.png", import.meta.url), png);
console.log("wrote scripts/source-icon.png (512x512)");
