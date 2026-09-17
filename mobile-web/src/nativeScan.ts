// Enable the page to invoke the native shell's QR code scanner.
//
// In a PWA, adding a device is straightforward: open the QR link from the second desktop on
// the first (the link fragment carries a secret key that `devices.ts::consumeHashSecret`
// receives). The native shell has no such path — it starts from rawfile, there's no address
// bar, and the shell's own scanner entry is only reachable when not yet paired (WebShell's
// `build()` only draws the pairing page when src is empty). So a user with the app installed
// can't add a second device, yet that's the primary entry point for multi-device.
//
// The fix is to expose scanning from the shell to the page: the shell registers
// `fleetNative.scanPairing()`, and the page offers one "Scan to add device" line in the device
// list. After the shell scans, it reloads the WebView via the old path and injects `#k=…`,
// and the web side receives it as a new device as usual — so the web side needs no Harmony
// branch except this one entry point.
//
// The Capacitor shell currently uses App Links (deepLink.ts) and doesn't need this; when it
// gets scanner capability, just registering a method with the same name will automatically work.

/** Name of the native bridge object injected by the shell
 *  (mobile-harmony's WebShell.ets: javaScriptProxy). */
const BRIDGE = "fleetNative";

interface NativeBridge {
  scanPairing?: () => void;
}

function bridge(): NativeBridge | undefined {
  return (window as unknown as Record<string, NativeBridge | undefined>)[BRIDGE];
}

/** Whether this shell can invoke the scanner. Always false in browsers/PWAs — not needed, and
 *  no shell to call. */
export function canScanPairing(): boolean {
  return typeof bridge()?.scanPairing === "function";
}

/** Invoke the scanner. After the shell scans, it reloads the page itself and injects the new
 *  pairing, so there's no callback here — the next page load tells whether it was added. */
export function scanPairing(): void {
  bridge()?.scanPairing?.();
}
