// Header hint — one line between the nameplate and the MFD/PIN buttons that
// shows what's under the cursor. Replaces every native `title` tooltip: a
// browser tooltip is dark-gray OS chrome with no CSS-reachable fix, breaking
// the amber VFD skin (D-review). A floating custom tooltip was tried once
// for History's bars and overflowed this window's real footprint (440x150)
// — this reuses that lesson ("docked, not floating") but globally in the
// header instead of per-page, so every control in every page can share it.

export function setHeaderHint(text) {
  const el = document.getElementById("header-hint");
  if (el) el.textContent = text || "";
}

/** Wires hover to show/clear a header-hint description for `el`. */
export function hintOnHover(el, text) {
  if (!el) return;
  el.addEventListener("mouseenter", () => setHeaderHint(text));
  el.addEventListener("mouseleave", () => setHeaderHint(""));
}
