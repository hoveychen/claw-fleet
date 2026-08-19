/* Fleet 移动端 service worker — Web Push 接收与点击聚焦。
 * 同 tag 的通知互相替换（桌面端与 fleet serve 双发时自然去重）。 */

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
            // 已经开着的窗口 focus 后 URL 一动不动 —— openWindow 那条路才会把
            // fragment 带进地址栏。所以把目标 url 单独投一份过去,否则「app 开
            // 着时点通知」永远停在当前页面。
            client.postMessage({ type: "fleet-deeplink", url });
            return client.focus();
          }
        }
        return self.clients.openWindow(url);
      }),
  );
});
