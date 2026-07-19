import assert from "node:assert/strict";
import test from "node:test";

import {
  clearHistoryCache,
  formatHistoryCost,
  loadHistory,
  loadSessionHistory,
  localDateKey,
  todayEntry,
} from "./history-data.js";

test("daily report sends a provider-discriminated command", async () => {
  clearHistoryCache();
  const calls = [];
  const rows = [{ provider: "codex", date: "2026-07-19" }];
  const result = await loadHistory("codex", false, {
    invoke: async (...args) => {
      calls.push(args);
      return rows;
    },
    now: () => 100,
  });
  assert.strictEqual(result, rows);
  assert.deepEqual(calls, [["history_daily", { provider: "codex" }]]);
});

test("concurrent consumers share one in-flight report per provider", async () => {
  clearHistoryCache();
  let resolve;
  let calls = 0;
  const invoke = () => {
    calls += 1;
    return new Promise((done) => {
      resolve = done;
    });
  };
  const history = loadHistory("codex", false, { invoke, now: () => 100 });
  const limits = loadHistory("codex", false, { invoke, now: () => 100 });
  resolve([{ provider: "codex", date: "2026-07-19" }]);

  assert.strictEqual(await history, await limits);
  assert.equal(calls, 1);
});

test("Claude and Codex caches remain independent", async () => {
  clearHistoryCache();
  const calls = [];
  const invoke = async (_command, { provider }) => {
    calls.push(provider);
    return [{ provider }];
  };
  const codex = await loadHistory("codex", false, { invoke, now: () => 100 });
  const claude = await loadHistory("claude", false, { invoke, now: () => 100 });
  await loadHistory("codex", false, { invoke, now: () => 101 });

  assert.deepEqual(codex, [{ provider: "codex" }]);
  assert.deepEqual(claude, [{ provider: "claude" }]);
  assert.deepEqual(calls, ["codex", "claude"]);
});

test("failed reports clear in-flight state so retry can recover", async () => {
  clearHistoryCache();
  let calls = 0;
  const invoke = async () => {
    calls += 1;
    if (calls === 1) throw new Error("temporary failure");
    return [{ provider: "codex" }];
  };
  await assert.rejects(loadHistory("codex", false, { invoke, now: () => 100 }));
  assert.deepEqual(
    await loadHistory("codex", false, { invoke, now: () => 101 }),
    [{ provider: "codex" }],
  );
});

test("history rejects cross-provider payloads", async () => {
  clearHistoryCache();
  await assert.rejects(
    loadHistory("codex", false, {
      invoke: async () => [{ provider: "claude" }],
      now: () => 100,
    }),
    /mismatched provider/,
  );
});

test("session history uses its own provider-scoped cache", async () => {
  clearHistoryCache();
  const calls = [];
  await loadSessionHistory("codex", false, {
    invoke: async (...args) => {
      calls.push(args);
      return [];
    },
    now: () => 100,
  });
  assert.deepEqual(calls, [["history_sessions", { provider: "codex" }]]);
});

test("Codex costs are explicitly estimated and nullable model costs stay unknown", () => {
  assert.equal(formatHistoryCost(1.25, "codex"), "EST $1.25");
  assert.equal(formatHistoryCost(1.25, "claude"), "$1.25");
  assert.equal(formatHistoryCost(null, "codex"), "—");
});

test("today lookup does not relabel stale usage as today", () => {
  const now = new Date(2026, 6, 19, 1, 0, 0);
  assert.equal(localDateKey(now), "2026-07-19");
  assert.equal(todayEntry([{ date: "2026-07-18" }], now), null);
  assert.deepEqual(todayEntry([{ date: "2026-07-19", totalTokens: 1 }], now), {
    date: "2026-07-19",
    totalTokens: 1,
  });
});
