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

// ── Relay tunnel ────────────────────────────────────────────────────────────
// When this bundle is mounted on a relay origin under /desktop/, data routes
// live under /desktop/h/… and are carried to the Mac over the relay channel
// (see app/relay/tunnel.ts). This worker only catches the requests the browser
// makes by itself (<img>, <iframe>, <a download>); it cannot hold the relay
// socket (it is torn down whenever idle), so it hands each request to a window
// that does and builds the Response from the answer. Inactive at any other
// scope — `fleet webui` serves this same file at "/".

const TUNNEL_MOUNT = "/desktop/";
const TUNNEL_MESSAGE = "fleet-tunnel-req";
/** Upper bound for one tunnelled request; the page side has its own timeout. */
const TUNNEL_TIMEOUT_MS = 60000;

function scopePath() {
  try {
    return new URL(self.registration.scope).pathname;
  } catch {
    return "/";
  }
}

/** `/desktop/h/` when mounted on a relay, else null. */
function tunnelPrefix() {
  const scope = scopePath();
  return scope.endsWith(TUNNEL_MOUNT) ? `${scope}h/` : null;
}

/**
 * The window that should carry this request: a page of this app (not a
 * document loaded *through* the tunnel, e.g. an iframe), preferring the one
 * that made the request.
 */
async function pickRelayWindow(event, prefix) {
  const scope = scopePath();
  const all = await self.clients.matchAll({ type: "window", includeUncontrolled: true });
  const holders = all.filter((c) => {
    let p;
    try {
      p = new URL(c.url).pathname;
    } catch {
      return false;
    }
    return p.startsWith(scope) && !p.startsWith(prefix);
  });
  return (
    holders.find((c) => c.id === event.clientId) ||
    holders.find((c) => c.focused) ||
    holders[0] ||
    null
  );
}

function plainResponse(status, text) {
  return new Response(text, { status, headers: { "Content-Type": "text/plain; charset=utf-8" } });
}

const NULL_BODY_STATUS = new Set([101, 204, 205, 304]);

async function tunnel(event, prefix) {
  const req = event.request;
  const win = await pickRelayWindow(event, prefix);
  if (!win) return plainResponse(503, "no Fleet window holds the relay connection");
  const body =
    req.method === "GET" || req.method === "HEAD" ? null : await req.arrayBuffer();
  const answer = await new Promise((resolve) => {
    const channel = new MessageChannel();
    const timer = setTimeout(() => resolve(null), TUNNEL_TIMEOUT_MS);
    channel.port1.onmessage = (ev) => {
      clearTimeout(timer);
      resolve(ev.data);
    };
    const msg = {
      type: TUNNEL_MESSAGE,
      method: req.method,
      url: req.url,
      contentType: req.headers.get("Content-Type"),
      body: body && body.byteLength > 0 ? body : null,
    };
    win.postMessage(msg, msg.body ? [channel.port2, msg.body] : [channel.port2]);
  });
  if (!answer) return plainResponse(504, "relay tunnel timed out");
  const status = answer.status >= 200 && answer.status <= 599 ? answer.status : 502;
  const headers = {};
  if (answer.contentType) headers["Content-Type"] = answer.contentType;
  return new Response(NULL_BODY_STATUS.has(status) ? null : answer.body, { status, headers });
}

self.addEventListener("fetch", (event) => {
  const req = event.request;
  let url;
  try {
    url = new URL(req.url);
  } catch {
    return;
  }
  const prefix = tunnelPrefix();
  if (prefix && url.origin === self.location.origin && url.pathname.startsWith(prefix)) {
    event.respondWith(tunnel(event, prefix));
    return;
  }
  if (req.method !== "GET") return;
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
