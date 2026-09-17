// Notification click → deep-link to the corresponding decision card.
//
// The desktop encodes the decision ID into the `url` fragment in notify frames
// (`/#d=<kind>:<id>`, see mobile_relay's notify_url). All three delivery paths converge here,
// so the web side has only one routing implementation:
//
//   * PWA cold start — service worker's notificationclick calls openWindow(url),
//     fragment comes straight in the address bar
//   * PWA already in foreground — SW just focus-doesn't-reload, URL unchanges,
//     so it postMessage the url separately
//   * Native shell (Harmony WebShell / future Capacitor) — no service worker;
//     native extracts url from the click intent, calls window.__fleetDeepLink to inject it
//
// Isomorphic with shareTarget.ts / nativePush.ts: native hook holds pending queue because
// the shell can deliver before this hook is registered (cold-start notification is exactly
// this case).

/** Native shell's delivery hook entry point. */
const NATIVE_DEEPLINK_HOOK = "__fleetDeepLink";
/** Shell queues early-arriving URLs here before hook is registered. */
const NATIVE_DEEPLINK_PENDING = "__fleetDeepLinkPending";

/** Fragment param name for decision ID, matches `/#d=` in mobile_relay::notify_url. */
const DECISION_PARAM = "d";
/** Source channel marker, stamped by relay during fan-out (fleet-relay/src/notify_target.rs). */
const CHANNEL_PARAM = "ch";

export interface DecisionTarget {
  kind: string;
  id: string;
  /** Source channel (channel id prefix). Multi-device needs this: card ID is unique only
   *  per device, so when two devices have cards, ID alone can't say which to expand.
   *
   *  Old relay doesn't stamp this, so it's optional — caller falls back to finding
   *  the first ID match (wrong card is better than no response). */
  channelMark?: string;
}

/**
 * Extract the decision to focus from a notify URL.
 *
 * Accepts full URL, path, or bare fragment — the three delivery paths give different shapes
 * (address bar is full URL, SW forwards notify's raw `/#d=...`), unified here.
 *
 * Split on **first** colon only: kind doesn't contain colons, but ID is external and might.
 * Return null if no ID — desktop degrades tag to bare kind when request lacks ID; those
 * links have no focusable target, treated as plain "open app".
 */
export function parseDecisionDeepLink(url: string): DecisionTarget | null {
  const hash = url.indexOf("#");
  if (hash < 0) return null;
  // Fragment is `&`-delimited params (`d=guard:g1&ch=105e300f`). Parse by param, not
  // "prefix + rest is id": relay appends source mark, so that naive parse would include
  // `&ch=…` in the card id.
  const params = new Map<string, string>();
  for (const part of url.slice(hash + 1).split("&")) {
    const eq = part.indexOf("=");
    if (eq <= 0) continue;
    params.set(part.slice(0, eq), part.slice(eq + 1));
  }
  const value = params.get(DECISION_PARAM);
  if (!value) return null;
  // Split on **first** colon only: kind lacks colons, but id is external and might contain them.
  const colon = value.indexOf(":");
  if (colon <= 0) return null;
  const kind = value.slice(0, colon);
  const id = value.slice(colon + 1);
  if (!id) return null;
  const channelMark = params.get(CHANNEL_PARAM);
  return channelMark ? { kind, id, channelMark } : { kind, id };
}

/**
 * Subscribe to "notification click should open a decision card". Returns unsubscribe function.
 *
 * On mount, read the current address once — the cold-start path's fragment is already in
 * the address bar; no event will replay it.
 */
export function onDecisionDeepLink(handler: (target: DecisionTarget) => void): () => void {
  const deliver = (url: unknown) => {
    if (typeof url !== "string") return;
    const target = parseDecisionDeepLink(url);
    if (target) handler(target);
  };

  // Cold start: fragment is already in address bar.
  deliver(window.location.href);

  // Click notification again in same page (browser changes hash, not reload).
  const onHashChange = () => deliver(window.location.href);
  window.addEventListener("hashchange", onHashChange);

  // PWA already foreground: SW just focuses-doesn't-reload, URL unchanged, so it postMessage.
  const onSwMessage = (e: MessageEvent) => {
    const data = e.data as { type?: string; url?: string } | undefined;
    if (data?.type === "fleet-deeplink") deliver(data.url);
  };
  navigator.serviceWorker?.addEventListener("message", onSwMessage);

  // Native shell injection channel. Drain backlog first — shell delivers before React effect on cold start.
  const w = window as unknown as Record<string, unknown>;
  const pending = w[NATIVE_DEEPLINK_PENDING];
  w[NATIVE_DEEPLINK_HOOK] = (url: string) => deliver(url);
  if (Array.isArray(pending)) {
    for (const item of pending as unknown[]) deliver(item);
    (pending as unknown[]).length = 0;
  }

  return () => {
    window.removeEventListener("hashchange", onHashChange);
    navigator.serviceWorker?.removeEventListener("message", onSwMessage);
    delete w[NATIVE_DEEPLINK_HOOK];
  };
}
