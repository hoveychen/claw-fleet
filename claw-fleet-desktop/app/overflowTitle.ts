/** Hover-to-read-full-text for every ellipsis in the app.
 *
 * The UI has well over a hundred `text-overflow: ellipsis` / `line-clamp`
 * sites (task titles, subtask rail rows, breadcrumbs, goal lines …) and most
 * of them never set a `title`, so a cut-off name could not be read anywhere.
 * Instead of threading `title` through each component, one delegated
 * `pointerover` listener finds the clamped element under the pointer and,
 * only when it is actually truncated, gives it a `title` holding its text.
 *
 * Elements that already carry an author-written `title` are left alone; the
 * attributes we add are tagged with `data-auto-title` so they can be refreshed
 * or dropped when the text or the width changes.
 */

const AUTO_ATTR = "data-auto-title";
/** How far up from the hovered node to look for the clamped box. Ellipsis is
 *  usually on the hovered span itself or one or two wrappers above it. */
const MAX_DEPTH = 4;

export interface ClampMetrics {
  textOverflow: string;
  lineClamp: string;
  scrollWidth: number;
  clientWidth: number;
  scrollHeight: number;
  clientHeight: number;
}

/** Whether a box with these computed styles and sizes is visibly cut off. */
export function isClampedTruncated(m: ClampMetrics): boolean {
  if (m.textOverflow === "ellipsis" && m.scrollWidth > m.clientWidth) return true;
  const clamp = m.lineClamp.trim();
  if (clamp && clamp !== "none" && m.scrollHeight > m.clientHeight) return true;
  return false;
}

function metricsOf(el: HTMLElement): ClampMetrics {
  const cs = getComputedStyle(el);
  const lineClamp =
    cs.getPropertyValue("-webkit-line-clamp") || cs.getPropertyValue("line-clamp") || "";
  return {
    textOverflow: cs.textOverflow,
    lineClamp,
    scrollWidth: el.scrollWidth,
    clientWidth: el.clientWidth,
    scrollHeight: el.scrollHeight,
    clientHeight: el.clientHeight,
  };
}

function fullText(el: HTMLElement): string {
  return (el.innerText ?? el.textContent ?? "").replace(/\s+/g, " ").trim();
}

/** Find the clamped element for a hover target and set or clear its auto
 *  title. Exported for tests; production goes through `installOverflowTitles`. */
export function refreshOverflowTitle(target: EventTarget | null): void {
  let el = target instanceof Element ? target : null;
  for (let depth = 0; el && depth < MAX_DEPTH; depth++, el = el.parentElement) {
    if (!(el instanceof HTMLElement)) continue;
    const m = metricsOf(el);
    const clamped = m.textOverflow === "ellipsis" || (m.lineClamp && m.lineClamp !== "none");
    if (!clamped) {
      // An author `title` closer to the pointer already wins the native
      // tooltip; adding one further up would never show.
      if (el.hasAttribute("title")) return;
      continue;
    }
    const auto = el.hasAttribute(AUTO_ATTR);
    if (el.hasAttribute("title") && !auto) return;
    const text = isClampedTruncated(m) ? fullText(el) : "";
    if (text) {
      if (el.getAttribute("title") !== text) el.setAttribute("title", text);
      el.setAttribute(AUTO_ATTR, "");
    } else if (auto) {
      el.removeAttribute("title");
      el.removeAttribute(AUTO_ATTR);
    }
    return;
  }
}

let installed = false;

/** Install the delegated listener once per document. */
export function installOverflowTitles(doc: Document = document): void {
  if (installed || typeof doc === "undefined") return;
  installed = true;
  doc.addEventListener("pointerover", (e) => refreshOverflowTitle(e.target), {
    capture: true,
    passive: true,
  });
}
