#!/usr/bin/env node

import { mkdir } from "node:fs/promises";
import path from "node:path";
import { fileURLToPath } from "node:url";

import { chromium } from "playwright";
import { createServer } from "vite";

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const outputDir = path.join(root, "docs", "screenshots", "claude-baseline");
const writeSnapshots = process.argv.includes("--update");
const viewport = { width: 550, height: 150 };
const pages = [
  [0, "SINCE START", "live"],
  [1, "HISTORY", "history"],
  [2, "LIMITS", "limits"],
  [3, "SETTINGS", "settings"],
];

const server = await createServer({
  root,
  logLevel: "error",
  server: { host: "127.0.0.1", port: 0, strictPort: false },
});

let browser;
try {
  await server.listen();
  const address = server.httpServer?.address();
  if (!address || typeof address === "string") throw new Error("Vite did not expose a test port");

  browser = await chromium.launch({ headless: true });
  const page = await browser.newPage({ viewport, deviceScaleFactor: 1 });
  await page.goto(`http://127.0.0.1:${address.port}/`);
  await page.waitForSelector(".page.active");
  await page.addStyleTag({
    content: "*, *::before, *::after { animation: none !important; transition: none !important; }",
  });
  await page.evaluate(() => {
    document.getElementById("clock").textContent = "12:00";
    document.querySelector(".fake-cursor")?.remove();
  });

  if (writeSnapshots) await mkdir(outputDir, { recursive: true });

  for (const [index, expectedLabel, filename] of pages) {
    const layout = await page.evaluate(() => {
      const active = document.querySelector(".page.active");
      const cluster = document.querySelector(".cluster");
      const screen = document.querySelector(".screen");
      const rect = screen.getBoundingClientRect();
      return {
        activePage: Number(active.dataset.page),
        label: document.getElementById("page-label").textContent,
        viewport: [document.documentElement.clientWidth, document.documentElement.clientHeight],
        bodyOverflow: [
          document.body.scrollWidth - document.body.clientWidth,
          document.body.scrollHeight - document.body.clientHeight,
        ],
        pageOverflow: [active.scrollWidth - active.clientWidth, active.scrollHeight - active.clientHeight],
        clusterSize: [cluster.clientWidth, cluster.clientHeight],
        screenBounds: [rect.left, rect.top, rect.right, rect.bottom],
      };
    });

    const expected = JSON.stringify([viewport.width, viewport.height]);
    if (layout.activePage !== index || layout.label !== expectedLabel) {
      throw new Error(`page ${index}: expected ${expectedLabel}, got ${layout.label}`);
    }
    if (JSON.stringify(layout.viewport) !== expected || JSON.stringify(layout.clusterSize) !== expected) {
      throw new Error(`page ${index}: expected 550x150, got ${layout.viewport.join("x")}`);
    }
    if (layout.bodyOverflow.some((value) => value > 0) || layout.pageOverflow.some((value) => value > 0)) {
      throw new Error(`page ${index}: content overflow ${JSON.stringify(layout)}`);
    }
    const [left, top, right, bottom] = layout.screenBounds;
    if (left < 0 || top < 0 || right > viewport.width || bottom > viewport.height) {
      throw new Error(`page ${index}: screen clipped ${layout.screenBounds.join(",")}`);
    }

    if (writeSnapshots) {
      await page.screenshot({ path: path.join(outputDir, `${filename}.png`) });
    }

    if (index < pages.length - 1) {
      await page.getByRole("button", { name: "▸" }).click();
      await page.mouse.move(1, viewport.height - 1);
    }
  }

  console.log(
    writeSnapshots
      ? `visual contract passed; wrote ${pages.length} snapshots`
      : `visual contract passed; ${pages.length} screens at 550x150`,
  );
} finally {
  await browser?.close();
  await server.close();
}
