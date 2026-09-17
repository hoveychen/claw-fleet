// "Where is the relay for this device" — pure URL assignment logic without any relay client code.
//
// These functions originally lived in relay.ts, which is **the entire relay client** (WebSocket,
// end-to-end encryption, frame protocol). Anyone wanting to know a relay address had to drag
// in the entire tree: same-origin (webui) builds work around this with dynamic import (see comments
// in main.tsx and push.ts). After adding multiple devices, more places need to know the address
// (device registry needs to remember the relay specified by QR code, push needs the VAPID public key
// per relay), so this small piece was extracted separately.
//
// This module **has no module-load-time side effects**: doesn't read hash, doesn't read env,
// doesn't initialize anything. The old `const RELAY_BASE = resolveRelayBase(...)` in relay.ts
// had to be evaluated at module load because the pairing fragment gets wiped immediately after
// startup; now the relay specified by QR code is saved to the device record (devices.ts) the moment
// it arrives, so there's nothing that needs to be read before the fragment is wiped.

/** Parse `&relay=<encoded origin>` from the QR code fragment into an origin.
 *
 *  Only accepts absolute http/https URLs — the QR code is untrusted input, and this value
 *  becomes the base for all subsequent URLs on the client. Extract origin (discard path,
 *  normalize trailing slash): relay with path prefix is not supported on this client anyway
 *  (PWA base comes from window.location.origin, which also discards the prefix).
 *
 *  Only the Harmony shell writes this parameter (WebShell.ets → RelayStore): its page origin
 *  is fake (`https://fleet.local`), and without this parameter it can only use the relay
 *  baked in at build time; self-hosted relay would never pair without it. */
export function parseRelayParam(hash: string): string | null {
  const match = hash.match(/[#&]relay=([^&]+)/);
  if (!match) return null;
  let candidate: URL;
  try {
    candidate = new URL(decodeURIComponent(match[1]));
  } catch {
    return null;
  }
  if (candidate.protocol !== "https:" && candidate.protocol !== "http:") return null;
  return candidate.origin;
}

/** The relay designated by a complete pairing link. For **native shells**: they receive
 *  the full URL from App Link / Universal Link, not `window.location.hash`.
 *
 *  The QR code is `https://<relay-host>/#k=<secret>` (mobile_relay::pairing_url),
 *  and that host is decided by **the desktop** — regional default (relay_region.rs) or
 *  a self-hosted address the user enters in settings. So the origin of the link itself
 *  is the relay this device should connect to.
 *
 *  Explicit `&relay=` still takes precedence: it describes "this pairing session",
 *  more specific than origin (Harmony shell writes it because its page origin is fake
 *  domain `fleet.local`). Consistent with priority in `resolveRelayBase`.
 *
 *  Non-http/https links (custom scheme) have no available origin, returns `null` —
 *  caller proceeds with pairing normally, but the device doesn't name a relay, and
 *  `relayBaseFor` builds a default for it. */
export function pairingLinkRelayBase(url: string): string | null {
  const hashAt = url.indexOf("#");
  const explicit = parseRelayParam(hashAt < 0 ? "" : url.slice(hashAt));
  if (explicit) return explicit;
  try {
    const parsed = new URL(hashAt < 0 ? url : url.slice(0, hashAt));
    if (parsed.protocol !== "https:" && parsed.protocol !== "http:") return null;
    return parsed.origin;
  } catch {
    return null;
  }
}

/** Which address to use for devices that don't name a relay.
 *
 *  `baked` is `VITE_RELAY_URL`, baked in at build time (Harmony shell gets it via
 *  `mobile-harmony/scripts/sync-web.sh`; its page origin is fake domain, falling back to
 *  origin would make the app call itself). `origin` is `window.location.origin` — the
 *  correct solution for PWA because that page is served by the relay itself.
 *
 *  Takes parameters rather than reading globals directly, so it stays a pure function
 *  that can be tested. */
export function defaultRelayBaseFrom(baked: string | undefined, origin: string): string {
  return baked || origin;
}

/** Live version of the above: build constant + current origin. */
export function defaultRelayBase(): string {
  return defaultRelayBaseFrom(import.meta.env.VITE_RELAY_URL, window.location.origin);
}

/** The relay a device actually connects to: its own named relay, or the built default. */
export function relayBaseFor(relayBase: string | null | undefined): string {
  return relayBase ?? defaultRelayBase();
}

/** Combine three possible sources that can name a relay into one address. Pure function
 *  so it can be tested independently of `window`.
 *
 *  `&relay=` in `hash` takes precedence over `baked`: the former describes **this pairing
 *  session**, the latter is just the default this package happens to have. */
export function resolveRelayBase(
  hash: string,
  baked: string | undefined,
  origin: string,
): string {
  return parseRelayParam(hash) ?? defaultRelayBaseFrom(baked, origin);
}

/** Short human-readable form of the relay hostname for display on the "More" page.
 *  `https` is the normal case so its scheme is dropped as noise; others (like
 *  `http://127.0.0.1:…` for dev relay) keep the scheme — that difference is exactly
 *  what you want to know when looking at this line. */
export function relayDisplayHost(base: string): string {
  try {
    const u = new URL(base);
    return u.protocol === "https:" ? u.host : `${u.protocol}//${u.host}`;
  } catch {
    return base;
  }
}

/** The WebSocket endpoint for this relay. */
export function relayWsUrl(base: string): string {
  return base.replace(/\/$/, "").replace(/^http/, "ws") + "/ws";
}
