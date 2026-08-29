/**
 * Poster frames for the two formats the 产出 grid is mostly made of and had no
 * thumbnail for: a video's first real frame and a PDF's first page.
 *
 * Sibling of `officeRender` rather than part of it — the Office three share a
 * shape (download a zip, hand it to a library that writes DOM), and these two
 * share a different one (produce a bitmap, hand back a data URL). Keeping the
 * split visible is what lets `ArtifactThumb` cache both in the same map: a
 * `<img src="data:…">` survives `innerHTML` round-tripping, which a `<canvas>`
 * would not — its pixels live outside the markup.
 *
 * Both renderers are lazy in the same way the Office ones are: pdf.js is ~1 MB
 * and must not be in the chunk that paints a session list.
 */

import { fetchBlob } from "./officeRender";

/**
 * Width the poster bitmap is encoded at.
 *
 * A grid well is ~190 px and a retina panel draws it at 2×, so 640 is generous
 * enough that the card never looks resampled while keeping a cached JPEG in the
 * tens of kilobytes. The height follows the source's aspect ratio — a poster
 * that letterboxes a 9:16 phone render into 4:3 is worse than one that crops.
 */
export const POSTER_WIDTH = 640;

/** JPEG rather than PNG: this is a photograph-like frame, and ~6× smaller. */
const POSTER_QUALITY = 0.72;

/** How long a frame grab may take before the card goes back to its icon. */
const GRAB_TIMEOUT_MS = 15_000;

/**
 * Where in a clip to grab the poster from.
 *
 * Frame 0 is the worst possible choice and the obvious one: renders open on a
 * fade-in, a slate, or an empty background, so a grid of first frames is a grid
 * of black rectangles. A short way in is past the fade and still unambiguously
 * "the start of this clip".
 *
 * Exported because the clamping is the part worth testing — `duration` arrives
 * from a decoder and is routinely `NaN` (metadata not parsed yet) or `Infinity`
 * (a stream, or an mp4 whose duration box is unset), and seeking to either
 * leaves the element stuck with no error to catch.
 */
export function posterTime(duration: number): number {
  if (!Number.isFinite(duration) || duration <= 0) return 0;
  // A tenth in, capped at a second: long enough to clear a fade on a 10s clip,
  // never so far into a 2h render that the frame stops being its opening.
  return Math.min(duration / 10, 1);
}

/** Encode a drawn frame as the data URL the thumbnail cache stores. */
function toPoster(source: CanvasImageSource, width: number, height: number): string {
  const scale = POSTER_WIDTH / width;
  const canvas = document.createElement("canvas");
  canvas.width = POSTER_WIDTH;
  canvas.height = Math.max(1, Math.round(height * scale));
  const ctx = canvas.getContext("2d");
  if (!ctx) throw new Error("no 2d context");
  ctx.drawImage(source, 0, 0, canvas.width, canvas.height);
  // Throws a SecurityError if the canvas was tainted, which is exactly the
  // signal `renderVideoPosterInto` retries on.
  return canvas.toDataURL("image/jpeg", POSTER_QUALITY);
}

/** Put a finished poster into the card's well. */
function showPoster(host: HTMLElement, dataUrl: string, alt: string): void {
  const img = document.createElement("img");
  img.src = dataUrl;
  img.alt = alt;
  img.decoding = "async";
  host.replaceChildren(img);
}

/**
 * Decode one frame of `src` and return it as a data URL.
 *
 * `crossOrigin` is the whole reason this takes a parameter: drawing a video
 * onto a canvas taints it unless the media was fetched CORS-clean, and a
 * tainted canvas throws on `toDataURL` rather than returning a blank image. The
 * artifact protocol does answer `Access-Control-Allow-Origin: *`, so the
 * cheap path *should* work — but "should" is not a thing to bet a silent
 * failure on, hence the fallback in the caller.
 */
function grabFrame(src: string, crossOrigin: boolean): Promise<string> {
  return new Promise((resolve, reject) => {
    const video = document.createElement("video");
    if (crossOrigin) video.crossOrigin = "anonymous";
    video.preload = "metadata";
    // Muted + playsInline so no autoplay policy or fullscreen takeover can be
    // triggered by an element that exists only to be sampled.
    video.muted = true;
    video.playsInline = true;

    let settled = false;
    const finish = (fn: () => void) => {
      if (settled) return;
      settled = true;
      clearTimeout(timer);
      fn();
      // Detach the source so the decoder stops working on a clip nobody is
      // watching; a fast scroll would otherwise leave a dozen of them running.
      video.removeAttribute("src");
      video.load();
    };
    const fail = (why: string) => finish(() => reject(new Error(why)));

    const timer = setTimeout(() => fail("video poster timed out"), GRAB_TIMEOUT_MS);

    video.onerror = () => fail("video failed to load");
    video.onloadedmetadata = () => {
      if (!video.videoWidth || !video.videoHeight) {
        fail("video has no visual track");
        return;
      }
      // Assigning currentTime is what fetches the frame; `seeked` is when it
      // has actually been decoded and is safe to draw.
      video.currentTime = posterTime(video.duration);
    };
    video.onseeked = () => {
      try {
        const poster = toPoster(video, video.videoWidth, video.videoHeight);
        finish(() => resolve(poster));
      } catch (e) {
        fail(String(e));
      }
    };

    video.src = src;
  });
}

/**
 * Draw a video's poster frame into `host`.
 *
 * Two paths, cheap one first:
 *
 *   1. **Ranged** — point a `<video>` straight at the artifact URL. The element
 *      pulls the header and the one fragment the frame lives in over `Range`,
 *      so a 200 MB render costs a few hundred kilobytes.
 *   2. **Downloaded** — if that throws (a tainted canvas, or a webview that
 *      declines CORS on a custom scheme), fetch the bytes and sample a blob
 *      URL instead, which is same-origin and can never taint.
 *
 * The fallback is not free — it is the whole file — so it is gated on size. A
 * clip past the gate keeps its icon rather than quietly pulling 200 MB to fill
 * a 190 px well.
 */
export async function renderVideoPosterInto(
  url: string,
  host: HTMLElement,
  alt: string,
  maxDownloadBytes: number,
  sizeBytes: number,
): Promise<void> {
  let poster: string;
  try {
    poster = await grabFrame(url, true);
  } catch (rangedError) {
    if (sizeBytes > maxDownloadBytes) throw rangedError;
    const objectUrl = URL.createObjectURL(await fetchBlob(url));
    try {
      poster = await grabFrame(objectUrl, false);
    } finally {
      URL.revokeObjectURL(objectUrl);
    }
  }
  showPoster(host, poster, alt);
}

/**
 * Draw a PDF's first page into `host`.
 *
 * The bytes are fetched here rather than handed to pdf.js as a URL: its own
 * transport only uses `fetch` for http(s) and falls back to XHR for anything
 * else, and `fleet-artifact://` is very much anything else. `fetchBlob` is the
 * same call the Office thumbnails already make on this protocol, so this reuses
 * a path that is known to work instead of betting on the library's fallback.
 *
 * One download plus a parse, then — a PDF's cross-reference table lives at the
 * end of the file, so there is no ranged shortcut to page 1 the way there is to
 * a video frame. That is what `MAX_PDF_THUMB_BYTES` is sized against.
 */
export async function renderPdfPosterInto(
  url: string,
  host: HTMLElement,
  alt: string,
): Promise<void> {
  const [pdfjs, workerUrl, bytes] = await Promise.all([
    import("pdfjs-dist"),
    // `?url` so Vite fingerprints the worker and serves it from our own origin.
    // Left to itself pdf.js resolves a path relative to the document, which in
    // a Tauri webview is not where the asset ended up.
    import("pdfjs-dist/build/pdf.worker.min.mjs?url").then((m) => m.default),
    fetchBlob(url).then((b) => b.arrayBuffer()),
  ]);
  pdfjs.GlobalWorkerOptions.workerSrc = workerUrl;

  // Keep the loading task, not just the document: tearing down the worker is
  // `task.destroy()`, and a document proxy only exposes `cleanup()`.
  const task = pdfjs.getDocument({ data: bytes });
  const doc = await task.promise;
  try {
    const page = await doc.getPage(1);
    // Lay the page out at poster width and render at that scale — rendering at
    // scale 1 and shrinking afterwards would resample text pdf.js could have
    // rasterised sharp.
    const unscaled = page.getViewport({ scale: 1 });
    const viewport = page.getViewport({ scale: POSTER_WIDTH / unscaled.width });
    const canvas = document.createElement("canvas");
    canvas.width = Math.round(viewport.width);
    canvas.height = Math.round(viewport.height);
    // A PDF page paints nothing where it is blank, and a JPEG has no alpha, so
    // without an explicit background the empty margins encode as black.
    await page.render({ canvas, viewport, background: "#ffffff" }).promise;
    showPoster(host, canvas.toDataURL("image/jpeg", POSTER_QUALITY), alt);
  } finally {
    // pdf.js holds the parsed document and its worker until told to let go.
    await task.destroy();
  }
}
