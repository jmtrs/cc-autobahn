import assert from "node:assert/strict";
import test from "node:test";

globalThis.localStorage = { getItem: () => null, setItem() {} };

const { activateMfdPage } = await import("./mfd-nav.js");

function page(number) {
  const classes = new Set();
  return {
    dataset: { page: String(number) },
    classList: {
      toggle(name, active) {
        if (active) classes.add(name);
        else classes.delete(name);
      },
      contains: (name) => classes.has(name),
    },
  };
}

test("one MFD action synchronizes provider pages and keeps one Settings page", () => {
  const pages = [page(0), page(0), page(1), page(1), page(2), page(2), page(3)];
  const chassis = { dataset: {} };
  const label = { textContent: "" };
  const events = [];
  const documentRoot = {
    querySelectorAll: () => pages,
    querySelector: () => chassis,
    getElementById: () => label,
    dispatchEvent: (event) => events.push(event),
  };

  activateMfdPage(2, documentRoot);
  assert.deepEqual(
    pages.map((item) => item.classList.contains("active")),
    [false, false, false, false, true, true, false],
  );
  assert.equal(chassis.dataset.currentPage, "2");
  assert.equal(label.textContent, "LIMITS");

  activateMfdPage(3, documentRoot);
  assert.equal(pages.filter((item) => item.classList.contains("active")).length, 1);
  assert.deepEqual(events.map((event) => event.detail.page), [2, 3]);
});
