// Pairing over deep links, for the native (Capacitor) shell only.
//
// The PWA gets its pairing secret from the URL it was opened with — the desktop
// QR encodes `https://<relay>/#k=<secret>`. The shell has no such URL: it boots
// from bundled assets at `capacitor://localhost`, so `consumeHashSecret()` always
// comes up empty and the app is stuck on the pairing gate forever.
//
// Universal Links (iOS) / App Links (Android) close that gap: the OS hands the
// scanned https URL to the app instead of the browser, and we pull the secret
// out of it. Both the warm path (app already running) and the cold path (the
// link is what launched the app, so the event may predate our listener) have to
// be covered — hence `getLaunchUrl()` alongside `appUrlOpen`.
//
// On the web this whole module is inert: `isNativePlatform()` is false, so
// nothing is subscribed and the PWA keeps using `window.location`.

import { App } from "@capacitor/app";
import { Capacitor } from "@capacitor/core";
import { extractSecretFromUrl } from "./secretStore";

/** True inside the Capacitor shell, false in the browser/PWA. */
export function isNativeShell(): boolean {
  return Capacitor.isNativePlatform();
}

/**
 * Call `handler` with the pairing secret whenever the OS delivers a pairing
 * link, including the link that cold-launched the app. No-op on web.
 *
 * Returns an unsubscribe function.
 */
export function onPairingLink(handler: (secret: string) => void): () => void {
  if (!Capacitor.isNativePlatform()) return () => {};

  let cancelled = false;
  const deliver = (url: string | undefined | null) => {
    if (cancelled || !url) return;
    const secret = extractSecretFromUrl(url);
    if (secret) handler(secret);
  };

  // Cold start — the launch URL is already spent by the time React mounts, so
  // `appUrlOpen` will never fire for it.
  App.getLaunchUrl()
    .then((result) => deliver(result?.url))
    .catch(() => {
      // No launch URL (normal icon tap), or the plugin is unavailable.
    });

  // Warm path — app already running when the link is opened.
  const listener = App.addListener("appUrlOpen", (event) => deliver(event.url));

  return () => {
    cancelled = true;
    listener.then((handle) => handle.remove()).catch(() => {});
  };
}
