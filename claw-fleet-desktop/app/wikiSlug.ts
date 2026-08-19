/**
 * Frontend mirror of core's `wiki::normalize_slug`.
 *
 * The backend is still the authority — it normalizes whatever it is sent, and
 * its answer is what lands on disk. This exists because the publish dialog has
 * to decide *before* submitting whether the slug names an existing doc: typing
 * `Notes/Today` must be recognised as the already-published `notes/today` and
 * offered as an append, not silently published as a replace that wipes it.
 *
 * Kept deliberately small: the ceilings core enforces (segment/total length,
 * depth) are left to the backend, which reports them as errors the dialog
 * shows. Only the character folding is duplicated, because only that decides
 * identity.
 */

/** Lowercase, collapse runs of non-alphanumerics to one hyphen, trim hyphens. */
function normalizeSegment(raw: string): string {
  let out = "";
  let prevHyphen = true; // suppresses a leading hyphen
  for (const ch of raw) {
    const c = ch.toLowerCase();
    // ASCII-alphanumeric only, matching core: CJK and accented characters are
    // not slug material, so `中文 report` normalizes to `report`.
    if (c.length === 1 && /[a-z0-9]/.test(c)) {
      out += c;
      prevHyphen = false;
    } else if (!prevHyphen) {
      out += "-";
      prevHyphen = true;
    }
  }
  return out.replace(/^-+|-+$/g, "");
}

/**
 * Normalize a raw name into a slug: `/` separates virtual directories and
 * survives, empty segments are dropped. Returns `""` when nothing survives —
 * the caller treats that as "not a usable slug yet" rather than an error,
 * since the user is mid-typing.
 */
export function normalizeSlug(raw: string): string {
  return raw
    .split("/")
    .map(normalizeSegment)
    .filter((s) => s.length > 0)
    .join("/");
}

/** Everything after the last `/` — the doc's own name, without directories. */
export function slugBasename(slug: string): string {
  const at = slug.lastIndexOf("/");
  return at < 0 ? slug : slug.slice(at + 1);
}
