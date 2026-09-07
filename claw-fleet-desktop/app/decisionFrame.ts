/**
 * Parent half of the decision-card iframe height handshake.
 *
 * The card renders agent-authored HTML in a `sandbox="allow-scripts"` iframe with
 * NO `allow-same-origin`, so the document stays on an opaque origin — the parent
 * cannot reach into `contentDocument` to measure it. The document therefore posts
 * its own height up (script injected server-side, `mcp_ipc::AUTOHEIGHT_SCRIPT`)
 * and these helpers decide what to trust.
 *
 * The message crosses a trust boundary: the payload is authored by an agent, so
 * treat every field as hostile until validated. The clamp is what stops a card
 * from claiming a 10,000px height and shoving the option buttons off-screen.
 */

/** Floor applied to a reported height — below this a card looks broken. */
export const FRAME_MIN_HEIGHT = 120;
/** Ceiling — a preview may scroll internally rather than push the footer away. */
export const FRAME_MAX_HEIGHT = 1400;
/** Ignore sub-pixel churn; also breaks feedback loops in viewport-relative layouts. */
export const FRAME_DEAD_BAND = 2;

/**
 * Validate + clamp a `postMessage` payload from a card iframe.
 * Returns null for anything that isn't a positive finite `__fleetAskHeight`.
 */
export function parseFrameHeight(data: unknown): number | null {
  if (!data || typeof data !== "object") return null;
  const raw = (data as { __fleetAskHeight?: unknown }).__fleetAskHeight;
  if (typeof raw !== "number" || !Number.isFinite(raw) || raw <= 0) return null;
  return Math.min(FRAME_MAX_HEIGHT, Math.max(FRAME_MIN_HEIGHT, Math.ceil(raw)));
}

/**
 * Whether a freshly reported height is worth applying. A document whose layout
 * depends on the viewport (`100vh`, `height: 100%`) re-measures every time we
 * resize it; the dead-band lets that settle instead of oscillating forever.
 */
export function shouldApplyFrameHeight(current: number | null, next: number): boolean {
  return current === null || Math.abs(next - current) > FRAME_DEAD_BAND;
}

/**
 * Foreground the prelude hands an *unstyled* preview, per theme.
 *
 * Hard-coded rather than read from `var(--color-text)` because the iframe is on
 * an opaque origin: the app's custom properties do not cross into it, so the
 * value has to travel inside the document. Keep these two in sync with
 * `--color-text` in App.css (dark root / `[data-theme="light"]`).
 */
const FRAME_TEXT: Record<"dark" | "light", string> = {
  dark: "#f7f8f8",
  light: "#1f2023",
};

/**
 * Wrap an agent-authored `html` preview into the document the card's iframe
 * actually loads.
 *
 * Agents write these fragments against the card they see — the observed failure
 * was a table styled `color:#e6e6e6; background:transparent`, i.e. light text
 * expecting the dark card to show through. The frame used to paint itself
 * `#fff`, so that table rendered light-grey-on-white and was unreadable in the
 * dark theme.
 *
 * The prelude fixes both halves of that:
 * - `color-scheme` states the host theme, so a document that styles *nothing*
 *   gets UA defaults (form controls, scrollbars) matching the app instead of
 *   whatever the OS happens to prefer.
 * - an explicit `color` plus a transparent canvas means the card's own themed
 *   surface shows through and unstyled text stays legible either way.
 *
 * It is a *prelude*: it comes before the agent's own markup, so any rule the
 * agent writes at equal specificity still wins. Nothing here is opaque, so an
 * agent that wants its own background just sets one.
 */
export function framePreviewSrcDoc(html: string, theme: "dark" | "light"): string {
  const prelude =
    `<style>:root{color-scheme:${theme}}` +
    `html,body{background:transparent}` +
    `body{margin:0;color:${FRAME_TEXT[theme]};` +
    `font:13px -apple-system,system-ui,sans-serif}</style>`;
  return `${prelude}${html}`;
}
