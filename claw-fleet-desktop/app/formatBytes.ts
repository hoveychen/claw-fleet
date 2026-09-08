/** Human-readable byte count for the 产出 page and its zip browser.
 *
 *  Pulled out of ArtifactsView so `ZipBrowser` can size a member without
 *  importing the page that renders it — that import would be a cycle, since
 *  the page is what mounts the browser. ArtifactsView re-exports it, so its
 *  existing callers (and `ArtifactsView.test.ts`) are unchanged. */
export function formatBytes(n: number): string {
  if (n < 1024) return `${n} B`;
  const units = ["KB", "MB", "GB", "TB"];
  let v = n / 1024;
  let i = 0;
  while (v >= 1024 && i < units.length - 1) {
    v /= 1024;
    i += 1;
  }
  // One decimal below 10 so "1.4 MB" doesn't round to a useless "1 MB", none
  // above it where the extra digit is noise.
  return `${v < 10 ? v.toFixed(1) : Math.round(v)} ${units[i]}`;
}
