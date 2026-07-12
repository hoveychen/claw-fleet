// Debounced full-text wiki search over the relay, mirroring useRelaySearch
// (which does the same for session_search). Queries < 2 chars return empty —
// the caller falls back to its grouped-list view; longer queries debounce
// 300ms then hit `wiki_search`, which scans doc metadata + entry body and
// returns WikiSearchHit rows (slug / field / snippet).
import { useEffect, useMemo, useRef, useState } from "react";
import type { RelayClient } from "./relay";
import type { WikiSearchHit } from "./types";
import { searchWikiDocs } from "./wiki";

export function useWikiSearch(client: RelayClient | null, query: string) {
  const [hits, setHits] = useState<WikiSearchHit[]>([]);
  const [searching, setSearching] = useState(false);
  const timerRef = useRef<number>(0);

  useEffect(() => {
    if (!client || query.trim().length < 2) {
      setHits([]);
      setSearching(false);
      return;
    }
    setSearching(true);
    clearTimeout(timerRef.current);
    timerRef.current = window.setTimeout(() => {
      searchWikiDocs(client, query.trim())
        .then((h) => setHits(h ?? []))
        .catch(() => setHits([]))
        .finally(() => setSearching(false));
    }, 300);
    return () => clearTimeout(timerRef.current);
  }, [client, query]);

  /** slug → best snippet, for display next to each result. */
  const snippetBySlug = useMemo(
    () => new Map(hits.map((h) => [h.slug, h.snippet])),
    [hits],
  );
  /** slugs that matched, in relay-returned order. */
  const matchSlugs = useMemo(() => hits.map((h) => h.slug), [hits]);

  return { searching, matchSlugs, snippetBySlug };
}
