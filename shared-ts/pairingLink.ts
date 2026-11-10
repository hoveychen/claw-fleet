// Pairing-link parsing shared by mobile-web and the desktop bundle served at
// `<relay>/desktop/`. Both read the same `#k=<secret>[&relay=<origin>]` fragment
// that `claw-fleet-core::mobile_relay::pairing_url` emits; keeping one parser means
// a pairing taken on either page lands in the device book identically.

/** Pull the pairing secret out of a URL (or a bare `#…` fragment).
 *
 *  Deliberately **fragment-only**: a fragment is never sent to the server, and
 *  the relay is a blind forwarder that must never see the secret (see
 *  `claw-fleet-core/src/relay_crypto.rs`). Accepting `?k=` here would leak it
 *  to the relay every time the pairing link is opened in a browser instead of
 *  the app — so if a platform strips fragments from deep links, fix it on that
 *  platform rather than widening this to the query string.
 *
 *  Shared with `deepLink.ts`, which feeds it the URL a Universal Link / App
 *  Link handed to the native shell instead of `window.location`. */
export function extractSecretFromUrl(url: string): string | null {
  const hashAt = url.indexOf("#");
  if (hashAt < 0) return null;
  const match = url.slice(hashAt).match(/[#&]k=([A-Za-z0-9_-]+)/);
  return match ? match[1] : null;
}

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
  try {
    return relayOriginOf(decodeURIComponent(match[1]));
  } catch {
    return null;
  }
}

/** Normalise a relay address to its origin, or `null` when it is not an absolute
 *  http/https URL. Shared by the QR `&relay=` parameter and the custom address the
 *  user types on the "More" page — both become the base of every relay URL. */
export function relayOriginOf(raw: string): string | null {
  let candidate: URL;
  try {
    candidate = new URL(raw.trim());
  } catch {
    return null;
  }
  if (candidate.protocol !== "https:" && candidate.protocol !== "http:") return null;
  return candidate.origin;
}

/** A pairing carried by a page's fragment: `#k=<secret>&relay=<url>&boot=1`. */
export type HashPairing = {
  kind: "relay";
  secret: string;
  relayBase: string | null;
  boot: boolean;
};

/** Pure parse of a fragment into a pairing (no scrubbing, no storage). `null` = the
 *  fragment carries no secret. */
export function parseHashPairing(hash: string): HashPairing | null {
  const secret = extractSecretFromUrl(hash);
  if (!secret) return null;
  return {
    kind: "relay",
    secret,
    relayBase: parseRelayParam(hash),
    boot: /[#&]boot=1\b/.test(hash),
  };
}

/** Mount path of the desktop bundle on a relay origin (fleet-relay serves it there). */
export const DESKTOP_WEB_MOUNT = "/desktop/";

/** The "open on a computer" link for a phone pairing link.
 *
 *  Derived from the phone link (`https://<relay>/#k=<secret>[&…]`) rather than
 *  rebuilt from the secret, so the two can never disagree about relay or secret:
 *  same origin, same fragment, only the path moves to the desktop mount. The
 *  desktop bundle is served at the relay root's `/desktop/` regardless of any
 *  path the configured relay URL carries (mobile-web likewise uses only the origin).
 *  `null` when the input is not an http(s) URL with a `#k=` secret. */
export function desktopPairingLink(phoneLink: string): string | null {
  const raw = phoneLink.trim();
  const hashAt = raw.indexOf("#");
  if (hashAt < 0 || !extractSecretFromUrl(raw)) return null;
  let url: URL;
  try {
    url = new URL(raw.slice(0, hashAt));
  } catch {
    return null;
  }
  if (url.protocol !== "https:" && url.protocol !== "http:") return null;
  return `${url.origin}${DESKTOP_WEB_MOUNT}${raw.slice(hashAt)}`;
}
