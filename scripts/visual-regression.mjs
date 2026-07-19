#!/usr/bin/env node

import { mkdir, readFile, writeFile } from "node:fs/promises";
import os from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";

import { chromium } from "playwright";
import { createServer } from "vite";

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const writeSnapshots = process.argv.includes("--update");
const providerModels = JSON.parse(
  await readFile(path.join(root, "scripts", "fixtures", "provider-models.json"), "utf8"),
);
const PIXEL_CHANNEL_TOLERANCE = 2;
const pages = [
  [0, "SINCE START", "live"],
  [1, "HISTORY", "history"],
  [2, "LIMITS", "limits"],
  [3, "SETTINGS", "settings"],
];
const displays = [
  { mode: "claude", viewport: { width: 550, height: 150 }, providers: ["claude"] },
  { mode: "codex", viewport: { width: 550, height: 150 }, providers: ["codex"] },
  { mode: "both", viewport: { width: 550, height: 290 }, providers: ["claude", "codex"] },
];
const themes = [
  { id: "amber", settings: { themeId: "amber", customAccent: "#ff9a1f" } },
  { id: "emerald", settings: { themeId: "emerald", customAccent: "#ff9a1f" } },
  { id: "magenta", settings: { themeId: "custom", customAccent: "#ff3bff" } },
];
const scenarios = displays.flatMap((display) =>
  themes.map((theme) => ({ ...display, theme })),
);

async function pixelsMatch(page, actual, baseline) {
  return page.evaluate(async ({ actualBase64, baselineBase64, tolerance }) => {
    const bitmap = async (base64) => {
      const response = await fetch(`data:image/png;base64,${base64}`);
      return createImageBitmap(await response.blob());
    };
    const [actualImage, baselineImage] = await Promise.all([
      bitmap(actualBase64),
      bitmap(baselineBase64),
    ]);
    if (
      actualImage.width !== baselineImage.width ||
      actualImage.height !== baselineImage.height
    ) return false;
    const pixels = (image) => {
      const canvas = document.createElement("canvas");
      canvas.width = image.width;
      canvas.height = image.height;
      const context = canvas.getContext("2d");
      context.drawImage(image, 0, 0);
      return context.getImageData(0, 0, image.width, image.height).data;
    };
    const actualPixels = pixels(actualImage);
    const baselinePixels = pixels(baselineImage);
    return actualPixels.every(
      (value, index) => Math.abs(value - baselinePixels[index]) <= tolerance,
    );
  }, {
    actualBase64: actual.toString("base64"),
    baselineBase64: baseline.toString("base64"),
    tolerance: PIXEL_CHANNEL_TOLERANCE,
  });
}

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
  for (const scenario of scenarios) {
    const page = await browser.newPage({ viewport: scenario.viewport, deviceScaleFactor: 1 });
    await page.addInitScript(({ mode, theme }) => {
      window.setInterval = () => 0;
      window.requestAnimationFrame = () => 0;
      window.cancelAnimationFrame = () => {};
      localStorage.setItem(
        "cc-autobahn.settings",
        JSON.stringify({
          schemaVersion: 2,
          displayMode: mode,
          global: { theme },
          providers: { claude: {}, codex: {} },
        }),
      );
    }, { mode: scenario.mode, theme: scenario.theme.settings });
    await page.goto(`http://127.0.0.1:${address.port}/`);
    await page.waitForSelector(`[data-app-chassis][data-display-mode="${scenario.mode}"]`);
    const coldMarkersHidden = await page
      .locator('[data-provider-role="gear-marker"]')
      .evaluateAll((markers) => markers.every((marker) => marker.hidden));
    if (!coldMarkersHidden) throw new Error(`${scenario.mode}: cold model marker claims activity`);
    await page.evaluate(async ({ models, providers }) => {
      const [{ setGear }, { createProviderView }, { setProviderAvailability }] = await Promise.all([
        import("/src/modules/trip-computer.js"),
        import("/src/modules/provider-view.js"),
        import("/src/modules/provider-status.js"),
      ]);
      providers.forEach((provider) => {
        if (provider === "codex") setProviderAvailability("codex", true);
        const fixture = models[provider];
        const view = createProviderView({ provider });
        setGear([fixture.modelId], view, fixture);
      });
    }, { models: providerModels, providers: scenario.providers });
    await page.addStyleTag({
      content: "*, *::before, *::after { animation: none !important; transition: none !important; }",
    });
    await page.evaluate(() => document.fonts.ready);
    await page.evaluate(() => {
      document.querySelector('[data-chassis-role="clock"]').textContent = "12:00";
      document.querySelector(".fake-cursor")?.remove();
    });

    const suffix = scenario.theme.id === "amber" ? "" : `-${scenario.theme.id}`;
    const outputDir = path.join(root, "docs", "screenshots", `${scenario.mode}${suffix}-baseline`);
    if (writeSnapshots) await mkdir(outputDir, { recursive: true });

    for (const [index, expectedLabel, filename] of pages) {
      if (index === 1) {
        await page.waitForFunction(() =>
          [...document.querySelectorAll('[data-provider-role="hd-date"]')]
            .filter((element) => element.getClientRects().length > 0)
            .every((element) => element.textContent !== "loading…"),
        );
      }
      if (index === 2) {
        await page.waitForFunction(() =>
          [...document.querySelectorAll('[data-provider-role="breakdown-list"]')]
            .filter((element) => element.getClientRects().length > 0)
            .every((element) => !element.querySelector(".engine-spinner")),
        );
      }
      await page.evaluate(() => {
        document.querySelector('[data-chassis-role="clock"]').textContent = "12:00";
      });
      const layout = await page.evaluate(() => {
        const visible = (element) => element.getClientRects().length > 0;
        const activePages = [...document.querySelectorAll(".page.active")].filter(visible);
        const visibleModules = [...document.querySelectorAll("[data-provider-module]")].filter(visible);
        const cluster = document.querySelector(".cluster");
        const screen = document.querySelector(".screen");
        const rect = screen.getBoundingClientRect();
        return {
          activePages: activePages.map((element) => Number(element.dataset.page)),
          label: document.getElementById("page-label").textContent,
          activeProvider: document.querySelector('[data-chassis-role="active-provider-tag"]')?.textContent,
          nameplate: document.querySelector('[data-chassis-role="nameplate"]')?.textContent,
          currentPage: Number(cluster.dataset.currentPage),
          visibleProviders: visibleModules.map((element) => element.dataset.providerModule),
          settingsCopies: document.querySelectorAll('.shared-pages .page[data-page="3"]').length,
          viewport: [document.documentElement.clientWidth, document.documentElement.clientHeight],
          bodyOverflow: [
            document.body.scrollWidth - document.body.clientWidth,
            document.body.scrollHeight - document.body.clientHeight,
          ],
          pageOverflow: activePages.map((element) => [
            element.scrollWidth - element.clientWidth,
            element.scrollHeight - element.clientHeight,
          ]),
          moduleOverflow: visibleModules.map((element) => [
            element.scrollWidth - element.clientWidth,
            element.scrollHeight - element.clientHeight,
          ]),
          clusterSize: [cluster.clientWidth, cluster.clientHeight],
          screenBounds: [rect.left, rect.top, rect.right, rect.bottom],
        };
      });

      const expectedSize = [scenario.viewport.width, scenario.viewport.height];
      const expectedActive = Array(index === 3 ? 1 : scenario.providers.length).fill(index);
      const expectedProvider = scenario.providers.at(-1).toUpperCase();
      const expectedNameplate = scenario.providers.at(-1) === "codex" ? "GPT 5.6 SOL" : "CC 500";
      if (
        layout.currentPage !== index ||
        layout.label !== expectedLabel ||
        layout.activeProvider !== expectedProvider ||
        layout.nameplate !== expectedNameplate ||
        JSON.stringify(layout.activePages) !== JSON.stringify(expectedActive)
      ) {
        throw new Error(`${scenario.mode} page ${index}: navigation mismatch ${JSON.stringify(layout)}`);
      }
      if (JSON.stringify(layout.visibleProviders) !== JSON.stringify(index === 3 ? [] : scenario.providers)) {
        throw new Error(`${scenario.mode} page ${index}: provider visibility ${JSON.stringify(layout)}`);
      }
      if (layout.settingsCopies !== 1) {
        throw new Error(`${scenario.mode}: expected one Settings page, got ${layout.settingsCopies}`);
      }
      if (
        JSON.stringify(layout.viewport) !== JSON.stringify(expectedSize) ||
        JSON.stringify(layout.clusterSize) !== JSON.stringify(expectedSize)
      ) {
        throw new Error(`${scenario.mode} page ${index}: expected ${expectedSize.join("x")}`);
      }
      const overflow = [...layout.bodyOverflow, ...layout.pageOverflow.flat(), ...layout.moduleOverflow.flat()];
      if (overflow.some((value) => value > 0)) {
        throw new Error(`${scenario.mode} page ${index}: content overflow ${JSON.stringify(layout)}`);
      }
      const [left, top, right, bottom] = layout.screenBounds;
      if (left < 0 || top < 0 || right > scenario.viewport.width || bottom > scenario.viewport.height) {
        throw new Error(`${scenario.mode} page ${index}: screen clipped ${layout.screenBounds.join(",")}`);
      }

      const screenshotPath = path.join(outputDir, `${filename}.png`);
      const screenshot = await page.screenshot(
        writeSnapshots
          ? { path: screenshotPath, animations: "disabled", caret: "hide" }
          : { animations: "disabled", caret: "hide" },
      );
      if (!writeSnapshots) {
        const baseline = await readFile(screenshotPath);
        if (!(await pixelsMatch(page, screenshot, baseline))) {
          const actualPath = path.join(
            os.tmpdir(),
            `cc-autobahn-${scenario.mode}-${scenario.theme.id}-${filename}-actual.png`,
          );
          await writeFile(actualPath, screenshot);
          throw new Error(
            `${scenario.mode}/${scenario.theme.id} ${filename}: visual snapshot changed; actual ${actualPath}`,
          );
        }
      }
      if (index < pages.length - 1) {
        await page.getByRole("button", { name: "▸" }).click();
        await page.mouse.move(1, scenario.viewport.height - 1);
      }
    }
    await page.close();
  }

  console.log(
    writeSnapshots
      ? `visual contract passed; wrote ${scenarios.length * pages.length} snapshots`
      : `visual contract passed; 3 modes × 3 themes × ${pages.length} screens`,
  );
} finally {
  await browser?.close();
  await server.close();
}
