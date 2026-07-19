#!/usr/bin/env node
// Screenshots the permission-gate card (D42) at the app's real window size
// (550×150, tauri.conf.json) without needing a live PermissionRequest hook
// round-trip — injects the same DOM state `onPermissionPending` would, via
// the running Vite dev server (`npm run dev`, fixed port 1420).
//
// Usage: node scripts/screenshot-permission-gate.mjs [out.png] [--tool=Bash]
//   [--summary="npm run build"] [--cwd=/path] [--context="repo · branch"]
//   [--timeout="auto-clears in 60s"]
//
// Requires the dev server already running (`npm run dev` in another shell)
// and `playwright` installed (`npm install`, then once:
// `npx playwright install chromium`).

import { chromium } from "playwright";

const args = process.argv.slice(2);
const out = args.find((a) => !a.startsWith("--")) ?? "permission-gate.png";
const opt = (name, fallback) => {
  const hit = args.find((a) => a.startsWith(`--${name}=`));
  return hit ? hit.slice(name.length + 3) : fallback;
};

const payload = {
  id: "preview",
  toolName: opt("tool", "Bash"),
  toolInputSummary: opt("summary", "npm run build 2>&1 | tail -20"),
  cwd: opt("cwd", process.cwd()),
  context: opt("context", "cc-autobahn · main"),
  timeoutLabel: opt("timeout", "auto-clears in 60s"),
};

const DEV_URL = "http://localhost:1420/";

const browser = await chromium.launch();
try {
  const page = await browser.newPage({ viewport: { width: 550, height: 150 } });
  await page.goto(`${DEV_URL}?t=${Date.now()}`);
  await page.waitForSelector("#permission-gate", { state: "attached" });

  await page.evaluate((p) => {
    document.getElementById("permission-tool").textContent = p.toolName;
    const summary = p.toolName === "Bash" ? `$ ${p.toolInputSummary}` : p.toolInputSummary;
    document.getElementById("permission-summary").textContent = summary;
    document.getElementById("permission-cwd").textContent = p.cwd;
    document.getElementById("permission-context").textContent = p.context;
    document.getElementById("permission-timeout").textContent = p.timeoutLabel;
    document.getElementById("permission-gate").hidden = false;
  }, payload);

  await page.waitForTimeout(150);

  const overflow = await page.evaluate(() => {
    const body = document.querySelector("#permission-gate .sensor-body");
    return body.scrollHeight - body.clientHeight;
  });
  if (overflow > 0) {
    console.warn(`[warn] .sensor-body overflows by ${overflow}px — content won't fully fit`);
  }

  await page.screenshot({ path: out });
  console.log(`wrote ${out} (body overflow: ${overflow}px)`);
} finally {
  await browser.close();
}
