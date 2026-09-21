/**
 * Every doc the reader opened in a session, kept past the rail's capacity.
 *
 * The rail holds {@link MAX_AUX_DOCS} doc cards and drops the oldest silently
 * when a ninth arrives (`detailAux.ts`'s `openDoc`), and `useSessionAux` throws
 * the whole stack away on a session switch. Both are right for the rail — it is
 * a viewfinder on what is in play, not a filing cabinet — and both used to mean
 * the same thing for the reader: a file you opened an hour ago is gone with no
 * way back except finding the sentence that named it again.
 *
 * This is the filing cabinet. It records only what it takes to *reopen* a doc —
 * kind, ref, label, when — never its content, so a long session costs a few
 * hundred bytes and reopening is the same `openDoc` call the transcript link
 * makes. The session facet panel reads it; nothing else needs to.
 *
 * Pure module, no React and no storage calls: the map transforms are the part
 * worth testing.
 */

import type { AuxDocKind } from "./detailAux";

export interface DocHistoryEntry {
  kind: AuxDocKind;
  /** Absolute path / wiki slug / url / artifact id, by kind. */
  ref: string;
  label: string;
  /** When it was last opened. Re-opening a known doc moves it to the front. */
  ts: number;
}

/** Keyed by session id. One blob under a single storage key, because the key
 *  list in `storage.ts` is a fixed enum and cannot hold a key per session. */
export type DocHistoryMap = Record<string, DocHistoryEntry[]>;

/** Docs remembered per session. Deep enough that a session's whole reading
 *  list survives (the rail shows 8 of them), shallow enough that the blob
 *  stays small. */
export const MAX_DOC_HISTORY = 60;

/** Sessions remembered at all. The blob is loaded into memory on boot, so it
 *  cannot grow with the number of sessions the user has ever opened; the
 *  least-recently-used session's list is dropped whole. */
export const MAX_HISTORY_SESSIONS = 50;

/** The storage key this map lives under. */
export const DOC_HISTORY_KEY = "session-doc-history";

/** Most recent first — the order the facet panel lists them in. */
export function docHistoryFor(map: DocHistoryMap, sessionId: string): DocHistoryEntry[] {
  return map[sessionId] ?? [];
}

/**
 * Record one opened doc.
 *
 * A doc already in the list is moved to the front with a fresh timestamp
 * rather than duplicated: the question the list answers is "what have I been
 * reading", and the same file opened twice is one answer, not two.
 */
export function recordDoc(
  map: DocHistoryMap,
  sessionId: string,
  entry: DocHistoryEntry,
): DocHistoryMap {
  const prev = map[sessionId] ?? [];
  const kept = prev.filter((e) => !(e.kind === entry.kind && e.ref === entry.ref));
  const next = [entry, ...kept].slice(0, MAX_DOC_HISTORY);
  return pruneSessions({ ...map, [sessionId]: next }, sessionId);
}

/** Forget one doc — the facet panel's per-row ✕, which is the only delete the
 *  reader has. Dropping the last one leaves no empty list behind. */
export function forgetDoc(
  map: DocHistoryMap,
  sessionId: string,
  kind: AuxDocKind,
  ref: string,
): DocHistoryMap {
  const prev = map[sessionId];
  if (!prev) return map;
  const next = prev.filter((e) => !(e.kind === kind && e.ref === ref));
  if (next.length === prev.length) return map;
  const out = { ...map };
  if (next.length === 0) delete out[sessionId];
  else out[sessionId] = next;
  return out;
}

/** Forget a whole session's list. */
export function forgetSession(map: DocHistoryMap, sessionId: string): DocHistoryMap {
  if (!(sessionId in map)) return map;
  const out = { ...map };
  delete out[sessionId];
  return out;
}

/**
 * Cap the number of sessions held, evicting the least recently touched.
 *
 * `keep` is never evicted even if the map is already full and its entry is
 * older than everyone else's — it is the session being written to right now.
 */
function pruneSessions(map: DocHistoryMap, keep: string): DocHistoryMap {
  const ids = Object.keys(map);
  if (ids.length <= MAX_HISTORY_SESSIONS) return map;
  const newest = (id: string) => map[id]?.[0]?.ts ?? 0;
  const doomed = ids
    .filter((id) => id !== keep)
    .sort((a, b) => newest(a) - newest(b))
    .slice(0, ids.length - MAX_HISTORY_SESSIONS);
  if (doomed.length === 0) return map;
  const out = { ...map };
  for (const id of doomed) delete out[id];
  return out;
}

const KINDS: readonly string[] = ["file", "wiki", "web", "artifact"];

/**
 * Read the blob back, tolerating anything.
 *
 * The value is user-writable JSON on disk that survives app upgrades, so a
 * shape that no longer parses must degrade to "no history" rather than throw
 * during boot — losing a reading list is a nuisance, failing to start is not.
 */
export function parseDocHistory(raw: string | null): DocHistoryMap {
  if (!raw) return {};
  let data: unknown;
  try {
    data = JSON.parse(raw);
  } catch {
    return {};
  }
  if (!data || typeof data !== "object" || Array.isArray(data)) return {};
  const out: DocHistoryMap = {};
  for (const [sessionId, list] of Object.entries(data as Record<string, unknown>)) {
    if (!Array.isArray(list)) continue;
    const entries: DocHistoryEntry[] = [];
    for (const item of list) {
      const e = item as Partial<DocHistoryEntry>;
      if (!e || typeof e !== "object") continue;
      if (typeof e.ref !== "string" || !e.ref) continue;
      if (typeof e.kind !== "string" || !KINDS.includes(e.kind)) continue;
      entries.push({
        kind: e.kind as AuxDocKind,
        ref: e.ref,
        label: typeof e.label === "string" && e.label ? e.label : e.ref,
        ts: typeof e.ts === "number" && Number.isFinite(e.ts) ? e.ts : 0,
      });
      if (entries.length >= MAX_DOC_HISTORY) break;
    }
    if (entries.length > 0) out[sessionId] = entries;
  }
  return out;
}

export function serializeDocHistory(map: DocHistoryMap): string {
  return JSON.stringify(map);
}
