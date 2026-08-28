/* Fleet 桌面 UI 的浏览器形态（`fleet webui` / fleet-cloud）用的 service worker。
 *
 * 只做一件事：把 vite 产出的 /assets/ 静态块缓存下来，让**跨发版**不必重下没变
 * 的代码。动机是产出页的 Office 预览——docx-preview + read-excel-file +
 * pptx-preview 合计约 1.6 MB，其中 pptx-preview 自带 echarts 就占 1.25 MB。这些
 * 块已经是懒加载的（见 OfficePreview），但没有 SW 的话，每发一次版整份 UI 连同
 * 它们都要重新过网。
 *
 * 之所以「跨发版」这句成立：vite 的资源名带内容哈希，所以某个块只要内容没变，
 * URL 就一模一样，cache-first 直接命中；真变了就是另一个 URL，自然回源。这也
 * 正是为什么缓存名**不带**版本号——按版本分桶等于每发一次版全体作废，恰好把
 * 这个机制毁掉。
 *
 * 只碰 /assets/ 下的 GET。HTML 一律回源（否则发版后拿到旧 index.html，它引用的
 * 是已经不存在的哈希名），API/SSE 更不能碰——/events 是长连接，塞进缓存层只会
 * 把它挂死。
 */

// 换这个名字 = 主动丢弃全部旧资源（例如缓存策略本身改了）。日常发版不要动它。
const ASSET_CACHE = "fleet-assets-v1";

self.addEventListener("install", () => {
  self.skipWaiting();
});

self.addEventListener("activate", (event) => {
  event.waitUntil(
    (async () => {
      // 只清本 SW 自己的旧桶；别的名字可能属于同源下的另一个应用（/m/ 的移动端
      // UI 就挂在同一个域上）。
      const names = await caches.keys();
      await Promise.all(
        names
          .filter((n) => n.startsWith("fleet-assets-") && n !== ASSET_CACHE)
          .map((n) => caches.delete(n)),
      );
      await self.clients.claim();
    })(),
  );
});

/** 哈希命名、不可变、值得缓存的东西。 */
function isImmutableAsset(url) {
  return url.origin === self.location.origin && url.pathname.includes("/assets/");
}

self.addEventListener("fetch", (event) => {
  const req = event.request;
  if (req.method !== "GET") return;
  let url;
  try {
    url = new URL(req.url);
  } catch {
    return;
  }
  if (!isImmutableAsset(url)) return;

  event.respondWith(
    (async () => {
      const cache = await caches.open(ASSET_CACHE);
      const hit = await cache.match(req);
      if (hit) return hit;
      const res = await fetch(req);
      // 只存成功的同源响应。opaque（no-cors 跨源）响应的状态码读不到，存进去
      // 就是把一个可能是 404 的东西永久钉住。
      if (res.ok && res.type === "basic") {
        cache.put(req, res.clone()).catch(() => {});
      }
      return res;
    })(),
  );
});
