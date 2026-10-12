// `?mock` switch, a separate zero-dependency module.
//
// It originally lived in mock/relay.ts, which `extends RelayClient`. So App.tsx
// had to statically import the entire relay dependency tree just to read one
// query param — the same things main.tsx's dynamic import dropped in same-origin
// builds got pulled back in by this chain (verified: dist-webui contains
// `fleet-relay/hkdf/v1` and `new WebSocket`).
//
// The check itself is two lines and has nothing to do with mock data, so it
// belongs here (the one import, exitGuard, is dependency-free too).
//
// Demo mode is the same fixtures behind a persistent flag instead of a query
// param. Store reviewers (and curious users) have no desktop to pair with, and
// the packaged native shells have no URL bar to type `?mock` into, so the
// pairing gate offers a "try the demo" button that sets the flag and reloads.
// The mock is read once at module load (App.tsx `MOCK`), which is why entering
// and leaving both go through a reload rather than a state change.

import { reloadDeliberately } from "./exitGuard";

const DEMO_KEY = "fleet-demo-mode";

export function isMockMode(): boolean {
  return new URLSearchParams(window.location.search).has("mock") || isDemoMode();
}

/** True while the persistent demo flag is set. A page load that carries a real
 *  pairing (`#k=` in the fragment) drops the flag first, so scanning a desktop's QR
 *  in the browser always lands in the real app rather than back in the demo. */
export function isDemoMode(): boolean {
  try {
    if (/[#&]k=/.test(window.location.hash)) {
      localStorage.removeItem(DEMO_KEY);
      return false;
    }
    return localStorage.getItem(DEMO_KEY) === "1";
  } catch {
    return false;
  }
}

export function enterDemoMode(): void {
  localStorage.setItem(DEMO_KEY, "1");
  reloadDeliberately();
}

/** Leave the demo. `pairingHash` (a `#k=…` fragment) carries a pairing that arrived
 *  while the demo was running — a native-shell deep link — into the real app's boot,
 *  which adopts it from the fragment exactly like a scanned PWA link. */
export function exitDemoMode(pairingHash?: string): void {
  localStorage.removeItem(DEMO_KEY);
  if (pairingHash) window.location.hash = pairingHash;
  reloadDeliberately();
}
