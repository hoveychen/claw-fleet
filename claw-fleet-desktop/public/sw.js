/* Service worker for the browser form of the Fleet desktop UI (`fleet webui` /
 * fleet-cloud).
 *
 * Does one thing: cache the /assets/ static chunks vite outputs, so releases
 * don't need to re-download unchanged code. The motivation is the artifacts
 * page's Office preview — docx-preview + read-excel-file + pptx-preview total
 * ~1.6 MB, with pptx-preview alone bundling echarts at 1.25 MB. These chunks
 * are already lazy-loaded (see OfficePreview), but without this SW, every
 * release re-fetches the entire UI and all of them over the network.
 *
 * Why "releases" works: vite's asset names include content hashes, so a chunk
 * stays at the same URL as long as its content unchanged — cache-first is an
 * instant hit; if it changes, it's a new URL and automatically re-fetched. That's
 * why the cache name has no version number — versioning the bucket would void
 * all of them on each release, defeating the mechanism.
 *
 * Only touches GET under /assets/. HTML always re-fetches (else a release would
 * serve old index.html referencing non-existent hash-named assets), and API/SSE
 * is never cached — /events is a long connection, and putting it behind a cache
 * layer just hangs it.
 */

// Changing this name = intentionally invalidate all old assets (e.g., if the
// caching strategy itself changes). Don't touch it for routine releases.
const ASSET_CACHE = "fleet-assets-v1";

self.addEventListener("install", () => {
  self.skipWaiting();
});

self.addEventListener("activate", (event) => {
  event.waitUntil(
    (async () => {
      // Only clear this SW's own old buckets; other names may belong to a
      // different app on the same origin (e.g., the mobile UI at /m/ shares the
      // same domain).
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

/** Hash-named, immutable, worth caching. */
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
      // Only store successful same-origin responses. opaque (no-cors cross-origin)
      // response status code can't be read, so storing it would permanently pin
      // something that might be a 404.
      if (res.ok && res.type === "basic") {
        cache.put(req, res.clone()).catch(() => {});
      }
      return res;
    })(),
  );
});
