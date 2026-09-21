/* Fleet mobile service worker — receives Web Push and focuses on notification clicks.
 * Notifications with the same tag replace each other (naturally deduplicates when desktop and fleet serve both send). */

self.addEventListener("install", () => {
  self.skipWaiting();
});

self.addEventListener("activate", (event) => {
  event.waitUntil(self.clients.claim());
});

self.addEventListener("push", (event) => {
  let data = {};
  try {
    data = event.data ? event.data.json() : {};
  } catch {
    data = { body: event.data ? event.data.text() : "" };
  }
  // Launcher icon badge = how many decision cards are still waiting (the count
  // the relay stamps on every notify). This is the only path that updates it
  // while the app is closed; the page re-syncs it on open (src/appBadge.ts).
  // Not to be confused with the `badge:` notification option below, which is
  // the monochrome status-bar glyph.
  if (typeof data.badge === "number" && self.navigator.setAppBadge) {
    const n = Math.max(0, Math.floor(data.badge));
    const pending =
      n === 0 ? self.navigator.clearAppBadge?.() : self.navigator.setAppBadge(n);
    if (pending) pending.catch(() => {});
  }
  event.waitUntil(
    self.registration.showNotification(data.title || "Fleet", {
      body: data.body || "",
      tag: data.tag || undefined,
      data: { url: data.url || "/" },
      icon: "/icons/icon-192.png",
      badge: "/icons/icon-192.png",
    }),
  );
});

self.addEventListener("notificationclick", (event) => {
  event.notification.close();
  const url = (event.notification.data && event.notification.data.url) || "/";
  event.waitUntil(
    self.clients
      .matchAll({ type: "window", includeUncontrolled: true })
      .then((list) => {
        for (const client of list) {
          if ("focus" in client) {
            // When an already-open window calls focus(), the URL doesn't move — only the
            // openWindow path brings the fragment into the address bar. So send the target
            // url separately; otherwise clicking a notification while the app is open stays on the current page.
            client.postMessage({ type: "fleet-deeplink", url });
            return client.focus();
          }
        }
        return self.clients.openWindow(url);
      }),
  );
});
