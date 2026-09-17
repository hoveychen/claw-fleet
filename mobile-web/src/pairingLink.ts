// How to read a pairing link—shared parsing logic for three entry points.
//
// The desktop QR code is `https://<relay-host>/#k=<secret>`
// (claw-fleet-core::mobile_relay::pairing_url). The same format enters from three places:
//
//   - PWA: opened by this URL; `window.location.hash` contains it (devices.ts)
//   - Native shell: App Link / Universal Link passes the full URL (deepLink.ts)
//   - Manual paste: user pastes the link into the pairing gate (PairPasteForm.tsx)
//
// The third route is the **only** entry for custom relays: App Link requires the host
// to be hardcoded at compile time in the manifest, but a custom relay's host isn't
// known at compile time, so scanning a custom relay's QR only opens the browser, never
// the app. This route has no host-declaration dependency.

import { pairingLinkRelayBase } from "./relayBase";
import { extractSecretFromUrl } from "./secretStore";

/** What a pairing carries: a secret, plus the relay it names (null = unnamed; use
 *  build default). **Both are required**—taking only the secret means scanning a
 *  custom relay's QR still tries to connect to the official relay baked in at build
 *  time, and the symptom is just "never connects". */
export interface PairedLink {
  secret: string;
  relayBase: string | null;
}

/** Minimum length the relay requires for an auth frame (fleet-relay/src/ws.rs
 *  `MIN_SECRET_LEN`). Desktop generates 64-bit hex, so this gate only blocks clearly
 *  wrong input—letting the user know "this link is bad" immediately, not after pairing
 *  and getting stuck on an unreachable channel. */
const MIN_SECRET_LEN = 16;

/** Parse a complete pairing link. null = not a usable pairing link. */
export function parsePairingLink(raw: string): PairedLink | null {
  const url = raw.trim();
  if (!url) return null;
  const secret = extractSecretFromUrl(url);
  if (!secret || secret.length < MIN_SECRET_LEN) return null;
  return { secret, relayBase: pairingLinkRelayBase(url) };
}
