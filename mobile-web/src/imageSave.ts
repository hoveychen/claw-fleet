// Save / share for the full-screen image lightbox. Same transfer shape as the
// wiki export and the artifacts page: prefer the native share sheet (the OS can
// save to Photos / AirDrop / send), fall back to <a download>. The Harmony shell
// polyfills both halves (WebShell.ets), so one web path covers every host.

const MIME_EXT: Record<string, string> = {
  "image/jpeg": "jpg",
  "image/png": "png",
  "image/gif": "gif",
  "image/webp": "webp",
  "image/svg+xml": "svg",
  "image/bmp": "bmp",
  "image/avif": "avif",
  "image/heic": "heic",
};

/** Pick a filename for a lightbox image. Uses the last path segment of an
 *  http(s) src when it already carries an extension; otherwise builds
 *  `image-<stamp>.<ext>` from the blob's MIME type. */
export function imageFileName(src: string, mime: string, now: Date = new Date()): string {
  if (/^https?:/i.test(src)) {
    try {
      const last = decodeURIComponent(new URL(src).pathname.split("/").pop() ?? "");
      if (/^[^/\\]+\.[a-z0-9]{2,5}$/i.test(last)) return last;
    } catch {
      /* malformed URL — fall through to the generated name */
    }
  }
  const ext = MIME_EXT[mime.toLowerCase()] ?? "png";
  const pad = (n: number) => String(n).padStart(2, "0");
  const stamp =
    `${now.getFullYear()}${pad(now.getMonth() + 1)}${pad(now.getDate())}` +
    `-${pad(now.getHours())}${pad(now.getMinutes())}${pad(now.getSeconds())}`;
  return `image-${stamp}.${ext}`;
}

export type SaveOutcome = "shared" | "downloaded" | "cancelled";

/** Fetch the image behind `src` (data:, blob: or URL) and hand it to the share
 *  sheet, or download it when Web Share with files is unavailable. Throws on a
 *  fetch or share failure; a dismissed share sheet is `"cancelled"`, not an error. */
export async function saveImage(src: string): Promise<SaveOutcome> {
  const res = await fetch(src);
  if (!res.ok) throw new Error(`HTTP ${res.status}`);
  const blob = await res.blob();
  const name = imageFileName(src, blob.type);
  const file = new File([blob], name, { type: blob.type || "image/png" });
  const nav = navigator as Navigator & { canShare?: (d: { files: File[] }) => boolean };
  if (typeof navigator.share === "function" && nav.canShare?.({ files: [file] })) {
    try {
      await navigator.share({ files: [file], title: name });
      return "shared";
    } catch (e) {
      if (e instanceof DOMException && e.name === "AbortError") return "cancelled";
      throw e;
    }
  }
  const url = URL.createObjectURL(file);
  const a = document.createElement("a");
  a.href = url;
  a.download = name;
  document.body.appendChild(a);
  a.click();
  a.remove();
  // Revoke on the next tick: some WebViews start the download asynchronously
  // and would read a URL that is already gone.
  setTimeout(() => URL.revokeObjectURL(url), 0);
  return "downloaded";
}
