// Generates the 1024x1024 amber source PNG for Tauri's icon generator.
// W203-cluster motif: an amber speedometer redlining into "AUTOBAHN" — the
// needle is buried in the red zone (no Autobahn speed limit). Typography is
// Futura (Renner, 1927 Bauhaus geometric), tracked like a German nameplate.
// Renders an inline SVG with @resvg/resvg-js (devDependency). Run:
//   node scripts/make-icon.mjs
// then fan out all platform icons (.icns / .ico / PNGs, macOS + Linux) with:
//   npx @tauri-apps/cli icon scripts/source-icon.png
import { Resvg } from "@resvg/resvg-js";
import { writeFileSync } from "node:fs";

const S = 512, cx = 256, cy = 242;
const AMBER = "#FF9A1F", AMBER_HI = "#FFC061", RED = "#FF3B2F";
const rad = (d) => (d * Math.PI) / 180;
const pt = (a, r) => [cx + r * Math.cos(rad(a)), cy + r * Math.sin(rad(a))];

const defs = `<defs>
  <radialGradient id="glow" cx="50%" cy="40%" r="62%">
    <stop offset="0%" stop-color="#140c05"/><stop offset="72%" stop-color="#0a0705"/><stop offset="100%" stop-color="#050302"/>
  </radialGradient>
  <linearGradient id="amberGrad" x1="0" y1="0" x2="0" y2="1">
    <stop offset="0%" stop-color="${AMBER_HI}"/><stop offset="100%" stop-color="#FF8A0A"/>
  </linearGradient>
</defs>`;
const glass = `<rect x="24" y="24" width="464" height="464" rx="112" fill="url(#glow)" stroke="#1c1207" stroke-width="4"/>`;

// open-bottom gauge arc from a0 to a1 (screen deg, sweeping over the top)
function arcSeg(a0, a1, r, w, color) {
  const [x0, y0] = pt(a0, r), [x1, y1] = pt(a1, r);
  const large = ((a1 - a0 + 360) % 360) > 180 ? 1 : 0;
  return `<path d="M ${x0} ${y0} A ${r} ${r} 0 ${large} 1 ${x1} ${y1}" fill="none" stroke="${color}" stroke-width="${w}" stroke-linecap="round"/>`;
}
function ticks(r0, r1) {
  let s = "";
  for (let i = 0; i <= 8; i++) {
    const a = 150 + (240 / 8) * i;
    const [xa, ya] = pt(a, r0), [xb, yb] = pt(a, r1);
    s += `<line x1="${xa}" y1="${ya}" x2="${xb}" y2="${yb}" stroke="${a > 342 ? RED : AMBER}" stroke-width="4" stroke-linecap="round" opacity="0.9"/>`;
  }
  return s;
}
function needle(pct, len) {
  const a = 150 + 240 * pct;
  const [tx, ty] = pt(a, len);
  const [txT, tyT] = pt(a + 180, 30);
  const [bxL, byL] = pt(a + 90, 8), [bxR, byR] = pt(a - 90, 8);
  return `<polygon points="${tx},${ty} ${bxL},${byL} ${txT},${tyT} ${bxR},${byR}" fill="url(#amberGrad)"/>`;
}
const hub = `<circle cx="${cx}" cy="${cy}" r="15" fill="#0a0705" stroke="${AMBER}" stroke-width="5"/><circle cx="${cx}" cy="${cy}" r="5" fill="${AMBER}"/>`;
const plate = (txt, size, tracking, y, weight, fill) =>
  `<text x="256" y="${y}" font-family="Futura" font-size="${size}" font-weight="${weight}" fill="${fill}" text-anchor="middle" letter-spacing="${tracking}">${txt}</text>`;

const svg = `<svg xmlns="http://www.w3.org/2000/svg" width="${S}" height="${S}">${defs}${glass}
  ${arcSeg(150, 342, 158, 6, AMBER)}
  ${arcSeg(342, 30, 158, 6, RED)}
  ${ticks(140, 152)}
  ${needle(0.9, 148)}
  ${hub}
  ${plate("CC", 52, 14, 382, 500, AMBER)}
  ${plate("AUTOBAHN", 26, 10, 430, 400, RED)}
</svg>`;

const png = new Resvg(svg, {
  font: { loadSystemFonts: true, defaultFontFamily: "Futura" },
  fitTo: { mode: "width", value: 1024 },
}).render().asPng();
writeFileSync(new URL("./source-icon.png", import.meta.url), png);
console.log("wrote scripts/source-icon.png (1024x1024)");
