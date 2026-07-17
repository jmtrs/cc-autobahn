// Page 1 — history sparkline (Phase 6): last 30 days of Claude cost, one bar
// per day, VFD dot-matrix style. Lazy: only fetched when the page opens.
//
// Detail on hover is a FIXED docked readout (#history-detail), not a floating
// tooltip: the cluster window is 440x150 (tauri.conf.json) — short and wide —
// so anything positioned relative to a hovered bar clips against an edge
// somewhere. A fixed panel below the bars can't overflow (D-review).

import { formatModelCode, formatTokens, formatUsd } from "./format.js";
import { hintOnHover } from "./header-hint.js";
import { latestDay, loadHistory, SPINNER_HTML } from "./history-data.js";

let allDays = [];

/** Fills the detail readout: date, day total, and the top 3 models by cost
 *  with their own cost + tokens. Single-letter codes (formatModelCode) reuse
 *  the PRND lettering so a row never has to truncate a long model id. */
function showDetail(d) {
  document.getElementById("hd-date").textContent = d.date;
  document.getElementById("hd-total").textContent =
    `${formatUsd(d.totalCost)} · ${formatTokens(d.totalTokens)} tok`;

  const top3 = (d.modelBreakdowns ?? [])
    .slice()
    .sort((a, b) => (b.cost || 0) - (a.cost || 0))
    .slice(0, 3);
  document.getElementById("hd-models").innerHTML = top3
    .map((m) => {
      const tokens =
        (m.inputTokens || 0) +
        (m.outputTokens || 0) +
        (m.cacheCreationTokens || 0) +
        (m.cacheReadTokens || 0);
      return `<span class="model-chip"><span class="code">${formatModelCode(m.modelName)}</span>${formatUsd(m.cost)} ${formatTokens(tokens)}</span>`;
    })
    .join("");
}

function showMessage(text) {
  document.getElementById("hd-date").textContent = text;
  document.getElementById("hd-total").textContent = "";
  document.getElementById("hd-models").innerHTML = "";
}

function render(days) {
  allDays = days;
  const bar = document.getElementById("history-bars");
  bar.innerHTML = "";
  const max = Math.max(0.01, ...days.map((d) => Number(d.totalCost) || 0));
  days.forEach((d) => {
    const col = document.createElement("div");
    col.className = "hbar";
    const fill = document.createElement("div");
    fill.className = "hbar-fill";
    const pct = ((Number(d.totalCost) || 0) / max) * 100;
    fill.style.height = `${Math.max(2, Math.round(pct))}%`;
    col.appendChild(fill);
    col.addEventListener("mouseenter", () => showDetail(d));
    col.addEventListener("mouseleave", () => {
      const latest = latestDay(allDays);
      if (latest) showDetail(latest);
    });
    bar.appendChild(col);
  });
  const total = days.reduce((sum, d) => sum + (Number(d.totalCost) || 0), 0);
  document.getElementById("history-total").textContent = formatUsd(total);

  const latest = latestDay(days);
  if (latest) showDetail(latest);
  else showMessage("no usage in range");
}

async function refresh() {
  // Bars stay empty on a cold load (nothing rendered yet) — without this,
  // the whole page looks stuck for however long ccusage takes to spawn.
  document.getElementById("history-bars").innerHTML = SPINNER_HTML;
  showMessage("loading…");
  try {
    render(await loadHistory());
  } catch (e) {
    showMessage("no data");
    console.error("[history] history_daily:", e);
  }
}

export function wireHistoryPage() {
  hintOnHover(document.getElementById("history-bars"), "Hover a day for its cost and tokens");
  hintOnHover(document.getElementById("history-total"), "Sum of cost across the shown days");
  document.addEventListener("mfd-page-changed", (e) => {
    if (e.detail.page === 1) refresh();
  });
}
