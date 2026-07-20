#!/usr/bin/env node

import { mkdir, readFile, writeFile } from "node:fs/promises";
import os from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";

import { chromium } from "playwright";
import { createServer } from "vite";

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const writeSnapshots = process.argv.includes("--update");
const writeStateSnapshots = writeSnapshots || process.argv.includes("--update-states");
const providerModels = JSON.parse(
  await readFile(path.join(root, "scripts", "fixtures", "provider-models.json"), "utf8"),
);
const PIXEL_CHANNEL_TOLERANCE = 20;
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
const stateScenarios = [
  { name: "permission-claude", mode: "claude", height: 150 },
  { name: "permission-codex", mode: "codex", height: 150 },
  { name: "permission-queue", mode: "both", height: 290 },
  { name: "permission-long-command", mode: "claude", height: 150 },
  { name: "desktop-permission", mode: "codex", height: 150 },
  { name: "permission-coexist", mode: "both", height: 290 },
  { name: "codex-degraded", mode: "both", height: 290 },
  { name: "provider-diagnostics", mode: "both", height: 290 },
  { name: "provider-diagnostics-tail", mode: "both", height: 290 },
];

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

  browser = await chromium.launch({
    headless: true,
    args: ["--font-render-hinting=none", "--disable-lcd-text", "--disable-gpu"],
  });
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
      const expectedNameplate = scenario.providers.at(-1) === "codex" ? "GPT 5.6 SOL" : "CC 500";
      if (
        layout.currentPage !== index ||
        layout.label !== expectedLabel ||
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

  const stateOutputDir = path.join(root, "docs", "screenshots", "states-baseline");
  if (writeStateSnapshots) await mkdir(stateOutputDir, { recursive: true });
  for (const scenario of stateScenarios) {
    const viewport = { width: 550, height: scenario.height };
    const page = await browser.newPage({ viewport, deviceScaleFactor: 1 });
    await page.addInitScript(({ mode }) => {
      window.setInterval = () => 0;
      window.requestAnimationFrame = () => 0;
      window.cancelAnimationFrame = () => {};
      localStorage.setItem(
        "cc-autobahn.settings",
        JSON.stringify({
          schemaVersion: 2,
          displayMode: mode,
          global: { theme: { themeId: "amber", customAccent: "#ff9a1f" } },
          providers: { claude: {}, codex: {} },
        }),
      );
    }, { mode: scenario.mode });
    await page.goto(`http://127.0.0.1:${address.port}/`);
    await page.waitForSelector(`[data-app-chassis][data-display-mode="${scenario.mode}"]`);
    await page.addStyleTag({
      content: "*, *::before, *::after { animation: none !important; transition: none !important; }",
    });
    await page.evaluate(async ({ name, models }) => {
      const [{ setGear }, { createProviderView }, status] = await Promise.all([
        import("/src/modules/trip-computer.js"),
        import("/src/modules/provider-view.js"),
        import("/src/modules/provider-status.js"),
      ]);
      const providers = name.includes("codex") || name === "desktop-permission" || name === "permission-coexist" || name.startsWith("provider-diagnostics")
        ? ["claude", "codex"]
        : ["claude"];
      providers.forEach((provider) => {
        if (provider === "codex") status.setProviderAvailability("codex", true);
        const view = createProviderView({ provider });
        setGear([models[provider].modelId], view, models[provider]);
      });

      if (name === "desktop-permission") {
        const { onDesktopPermissionPending } = await import("/src/modules/permission-gate.js");
        onDesktopPermissionPending({
          id: name,
          provider: "codex",
          toolName: "Command",
          toolInputSummary: "npm run build",
          cwd: "/Users/example/cc-autobahn",
          observedAtMs: Date.now(),
        });
      } else if (name.startsWith("permission-")) {
        const { onPermissionPending } = await import("/src/modules/permission-gate.js");
        const provider = name === "permission-codex" ? "codex" : "claude";
        onPermissionPending({
          id: name,
          provider,
          toolName: "Bash",
          toolInputSummary: name === "permission-long-command"
            ? "npm run verify -- --configuration production --reporter compact"
            : "npm run build",
          cwd: "/Users/example/cc-autobahn",
          project: "cc-autobahn",
          branch: name === "permission-long-command" ? null : "feature/codex-hardening",
          pendingCount: name === "permission-queue" ? 4 : 1,
          providerPendingCount: name === "permission-queue" ? 2 : 1,
          alwaysAllowAvailable: true,
          expiresAtMs: Date.now() + 60_000,
        });
        document.getElementById("permission-timeout").textContent = "auto-clears in 60s";
        if (name === "permission-coexist") {
          const { onDesktopPermissionPending } = await import("/src/modules/permission-gate.js");
          onDesktopPermissionPending({
            id: "desktop-behind-hook",
            provider: "codex",
            toolName: "Command",
            toolInputSummary: "touch /tmp/desktop-test",
            cwd: "/Users/example/cc-autobahn",
            observedAtMs: Date.now(),
          });
        }
      } else if (name === "codex-degraded") {
        status.setProviderIssue("codex", "compatibility", "LIMITS UNAVAILABLE", true);
      } else if (name.startsWith("provider-diagnostics")) {
        const { renderDiagnostics } = await import("/src/modules/provider-diagnostics.js");
        renderDiagnostics([
          {
            provider: "claude",
            compatibility: "compatible",
            surface: "Claude Code hooks + local history",
            runtimeVersion: "2.1.9",
            runtimeExecutable: "/usr/local/bin/claude",
            capabilities: [
              { id: "usage-history", status: "available", quality: "estimated", source: "ccusage claude" },
              { id: "limits", status: "available", quality: "official", source: "statusLine" },
              { id: "live-activity", status: "available", quality: "local", source: "transcript" },
              { id: "permissions", status: "available", quality: "official", source: "hooks" },
            ],
          },
          {
            provider: "codex",
            compatibility: "degraded",
            surface: "Codex App Server + rollout history",
            runtimeVersion: "codex-cli 0.12.0",
            runtimeExecutable: "/usr/local/bin/codex",
            relatedRuntimes: [{
              surface: "ChatGPT desktop",
              productVersion: "26.715.31925",
              runtimeExecutable: "/Applications/ChatGPT.app/Contents/Resources/codex",
              runtimeVersion: "codex-cli 0.145.0-alpha.18",
            }],
            capabilities: [
              { id: "rate-limits", status: "unavailable", quality: "unavailable", source: "account/rateLimits/read", reason: "method not supported", fallback: "last known snapshot", remediation: "upgrade Codex CLI" },
              { id: "account-usage", status: "available", quality: "official", source: "account/usage/read" },
              { id: "hook-inventory", status: "available", quality: "native", source: "hooks/list" },
              { id: "app-server-connection", status: "available", quality: "official", source: "/usr/local/bin/codex" },
              { id: "permission-hook-installed", status: "available", quality: "native", source: "~/.codex/hooks.json" },
              { id: "permission-hook-enabled", status: "available", quality: "native", source: "~/.codex/hooks.json" },
              { id: "permission-hook-trusted", status: "unavailable", quality: "native", source: "~/.codex/hooks.json", reason: "hook trust is untrusted", fallback: "Codex native approval UI", remediation: "review /hooks" },
              { id: "permission-hook-active", status: "unavailable", quality: "native", source: "~/.codex/hooks.json", reason: "no exchange for current hash", fallback: "Codex native approval UI", remediation: "run a permission request" },
              { id: "native-approval-fallback", status: "available", quality: "native", source: "Codex native approval UI" },
              { id: "live-activity", status: "available", quality: "local", source: "rollout JSONL" },
              { id: "history", status: "available", quality: "local", source: "rollout JSONL" },
            ],
          },
        ], document.getElementById("diagnostics-body"));
        document.getElementById("diagnostics-overlay").hidden = false;
        if (name.endsWith("-tail")) {
          document.getElementById("diagnostics-body").scrollTop =
            document.getElementById("diagnostics-body").scrollHeight;
        }
      }
      document.querySelector(".fake-cursor")?.remove();
    }, { name: scenario.name, models: providerModels });
    await page.evaluate(() => document.fonts.ready);
    await page.waitForTimeout(50);
    if (scenario.name.endsWith("-tail")) {
      await page.evaluate(() => {
        const body = document.getElementById("diagnostics-body");
        body.scrollTop = body.scrollHeight;
      });
      await page.waitForTimeout(50);
    }

    const overflow = await page.evaluate(() => {
      const visible = (element) => element.getClientRects().length > 0;
      const guarded = [...document.querySelectorAll(
        "#permission-gate .sensor-card, #diagnostics-overlay .diagnostics-card",
      )].filter(visible);
      return {
        body: [
          document.body.scrollWidth - document.body.clientWidth,
          document.body.scrollHeight - document.body.clientHeight,
        ],
        guarded: guarded.map((element) => [
          element.scrollWidth - element.clientWidth,
          element.scrollHeight - element.clientHeight,
        ]),
      };
    });
    if ([...overflow.body, ...overflow.guarded.flat()].some((value) => value > 0)) {
      throw new Error(`${scenario.name}: content overflow ${JSON.stringify(overflow)}`);
    }

    const screenshotPath = path.join(stateOutputDir, `${scenario.name}.png`);
    const candidatePath = writeStateSnapshots
      ? screenshotPath
      : path.join(os.tmpdir(), `cc-autobahn-${scenario.name}-candidate.png`);
    await page.screenshot({ path: candidatePath, animations: "disabled", caret: "hide" });
    const screenshot = await readFile(candidatePath);
    if (!writeStateSnapshots) {
      const baseline = await readFile(screenshotPath);
      if (!(await pixelsMatch(page, screenshot, baseline))) {
        const actualPath = path.join(os.tmpdir(), `cc-autobahn-${scenario.name}-actual.png`);
        await writeFile(actualPath, screenshot);
        throw new Error(`${scenario.name}: visual snapshot changed; actual ${actualPath}`);
      }
    }
    await page.close();
  }

  console.log(
    writeSnapshots
      ? `visual contract passed; wrote ${scenarios.length * pages.length + stateScenarios.length} snapshots`
      : writeStateSnapshots
        ? `visual contract passed; verified ${scenarios.length * pages.length} screens and wrote ${stateScenarios.length} states`
        : `visual contract passed; 3 modes × 3 themes × ${pages.length} screens + ${stateScenarios.length} states`,
  );
} finally {
  await browser?.close();
  await server.close();
}
