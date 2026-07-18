// cc-autobahn — synthetic VFD cursor. The native OS cursor always paints
// above the entire window, so a CSS `cursor: url()` triangle looked like it
// floated outside the display glass no matter what. This renders our own
// .fake-cursor element instead (style.css), positioned on every mousemove
// and given a z-index below .screen::after's scanline grid, so it reads as
// tucked inside the glass — masked by the same horizontal/vertical lines
// that mask everything else behind it.

const HOTSPOT_X = 8;
const HOTSPOT_Y = 6;

// Every selector that previously carried `cursor: pointer` — kept in sync
// by hand since there's no CSS way to ask "what would the cursor have
// been here". :disabled variants are excluded via the .matches() check
// below instead of listed here.
const CLICKABLE_SELECTOR = [
  ".nameplate",
  ".pin-btn",
  ".footer-metric",
  ".gauge",
  ".sensor-btn",
  ".mfd-btn",
  ".vfd-dropdown-btn",
  ".vfd-dropdown-list li",
  ".vfd-check",
  ".vfd-check input",
  ".vfd-reorder-btn",
  ".vfd-theme-accent input[type='color']",
].join(", ");

export function wireCursor() {
  const screen = document.querySelector(".screen");
  const el = document.createElement("div");
  el.className = "fake-cursor";
  screen.appendChild(el);

  function hide() {
    el.style.opacity = "0";
  }

  window.addEventListener(
    "mousemove",
    (e) => {
      el.style.transform = `translate(${e.clientX - HOTSPOT_X}px, ${e.clientY - HOTSPOT_Y}px)`;
      el.style.opacity = "1";

      // In-place nameplate editing gets the text-beam state instead of the
      // arrow — no native `cursor: text` fallback (see style.css), it
      // wasn't reliably showing there either, so this is fully synthetic.
      const editing = e.target.closest('.nameplate[contenteditable="true"]');
      el.classList.toggle("text", !!editing);
      if (editing) {
        el.classList.remove("click");
        return;
      }

      const target = e.target.closest(CLICKABLE_SELECTOR);
      el.classList.toggle("click", !!target && !target.matches(":disabled"));
    },
    { passive: true },
  );

  // "Left the window" detection. mouseleave on <html> is the standard,
  // reliable signal for "pointer exited the viewport" — mouseout + a
  // relatedTarget-null check was tried first but missed slow exits
  // through a window edge (frameless/undecorated Tauri window, no OS
  // chrome to catch the transition), leaving the cursor stuck on-screen.
  // blur is a second safety net for focus-loss without a clean exit
  // (e.g. Cmd+Tab away mid-drag).
  document.documentElement.addEventListener("mouseleave", hide);
  window.addEventListener("blur", hide);
}
