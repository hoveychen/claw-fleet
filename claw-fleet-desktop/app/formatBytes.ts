/** Human-readable byte size. Lifted out of `ArtifactsView` so surfaces that
 *  only need the formatter — a decision card, say — don't pull a whole view
 *  module (and its store / i18n graph) in with it. `ArtifactsView` re-exports
 *  this, so its existing tests keep covering the behaviour.
 *
 *  Note: four other copies of this function still live in `MemoryView`,
 *  `WikiView`, `blocks/DocumentBlock` and `blocks/toolPresenters`, and they do
 *  not all agree (DocumentBlock renders 1024 as "1 KB", this one as "1.0 KB").
 *  Consolidating them is its own change. */
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
