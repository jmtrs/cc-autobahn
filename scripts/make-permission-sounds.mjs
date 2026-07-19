// Generates short WAV alert tones for the permission gate (D42 follow-up).
// Zero-dependency PCM synthesis (raw sine waves), same zero-dep spirit as
// make-icon.mjs. Run: node scripts/make-permission-sounds.mjs
//
// D-review: an earlier pass here tried to physically model real automotive
// hardware (a square-wave piezo buzzer, a white-noise relay click) to match
// the W203's era — it sounded like a broken speaker, not a car. Back to
// plain sine tones with a clean attack/decay envelope: unremarkable, but not
// unpleasant, which is the actual bar for a UI alert sound.
import { mkdirSync, writeFileSync } from "node:fs";

const SAMPLE_RATE = 44100;

function wavHeader(dataLength) {
  const buf = Buffer.alloc(44);
  buf.write("RIFF", 0);
  buf.writeUInt32LE(36 + dataLength, 4);
  buf.write("WAVE", 8);
  buf.write("fmt ", 12);
  buf.writeUInt32LE(16, 16); // fmt chunk size
  buf.writeUInt16LE(1, 20); // PCM
  buf.writeUInt16LE(1, 22); // mono
  buf.writeUInt32LE(SAMPLE_RATE, 24);
  buf.writeUInt32LE(SAMPLE_RATE * 2, 28); // byte rate (mono, 16-bit)
  buf.writeUInt16LE(2, 32); // block align
  buf.writeUInt16LE(16, 34); // bits per sample
  buf.write("data", 36);
  buf.writeUInt32LE(dataLength, 40);
  return buf;
}

/** One sine tone, amplitude-enveloped (linear fade in/out) so it doesn't
 *  click at the edges — durationMs total, fadeMs on each end. */
function tone(freqHz, durationMs, fadeMs, amplitude = 0.3) {
  const n = Math.round((SAMPLE_RATE * durationMs) / 1000);
  const fadeN = Math.round((SAMPLE_RATE * fadeMs) / 1000);
  const samples = new Float64Array(n);
  for (let i = 0; i < n; i++) {
    const t = i / SAMPLE_RATE;
    let env = 1;
    if (i < fadeN) env = i / fadeN;
    else if (i > n - fadeN) env = (n - i) / fadeN;
    samples[i] = Math.sin(2 * Math.PI * freqHz * t) * amplitude * env;
  }
  return samples;
}

function silence(durationMs) {
  return new Float64Array(Math.round((SAMPLE_RATE * durationMs) / 1000));
}

function concat(...chunks) {
  const total = chunks.reduce((sum, c) => sum + c.length, 0);
  const out = new Float64Array(total);
  let offset = 0;
  for (const c of chunks) {
    out.set(c, offset);
    offset += c.length;
  }
  return out;
}

function toInt16(floatSamples) {
  const out = new Int16Array(floatSamples.length);
  for (let i = 0; i < floatSamples.length; i++) {
    out[i] = Math.max(-32767, Math.min(32767, Math.round(floatSamples[i] * 32767)));
  }
  return out;
}

function writeWav(path, floatSamples) {
  const int16 = toInt16(floatSamples);
  const data = Buffer.from(int16.buffer, int16.byteOffset, int16.byteLength);
  writeFileSync(path, Buffer.concat([wavHeader(data.length), data]));
  console.log(`wrote ${path}`);
}

const outDir = new URL("../public/sounds/", import.meta.url);
mkdirSync(outDir, { recursive: true });

// CHIME — two-tone descending beep, ~340ms total.
writeWav(new URL("chime.wav", outDir), concat(tone(988, 140, 15), tone(784, 160, 20)));

// BEEP — single short tone, ~140ms.
writeWav(new URL("beep.wav", outDir), tone(880, 140, 15));

// CLICK — two quick soft pips, ~160ms total.
writeWav(new URL("click.wav", outDir), concat(tone(1200, 55, 8, 0.22), silence(35), tone(1200, 55, 8, 0.22)));
