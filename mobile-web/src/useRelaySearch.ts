// Debounced full-text search over the relay, mirroring the desktop
// useSessionSearch hook (claw-fleet-desktop/app/hooks/useSessionSearch.ts).
//
// The desktop hook invokes the local `search_sessions` Tauri command; here we
// issue a `session_search` request over the relay, which the desktop serves
// from the same local SearchIndex (see mobile_relay.rs::serve_request). Same
// contract: queries < 2 chars return empty (the caller's substring filter
// covers those), longer queries debounce 300ms then hit the index.
import { useEffect, useMemo, useRef, useState } from "react";
import type { RelayClient } from "./relay";
import type { SearchHit } from "./types";

export function useRelaySearch(client: RelayClient | null, filter: string) {
  const [searchHits, setSearchHits] = useState<SearchHit[]>([]);
  const [searching, setSearching] = useState(false);
  const timerRef = useRef<number>(0);

  useEffect(() => {
    if (!client || filter.trim().length < 2) {
      setSearchHits([]);
      setSearching(false);
      return;
    }

    setSearching(true);
    clearTimeout(timerRef.current);
    timerRef.current = window.setTimeout(() => {
      client
        .request<SearchHit[]>("session_search", { query: filter.trim(), limit: 50 })
        .then((hits) => setSearchHits(hits ?? []))
        .catch(() => setSearchHits([]))
        .finally(() => setSearching(false));
    }, 300);

    return () => clearTimeout(timerRef.current);
  }, [client, filter]);

  // Memoised on `searchHits` so the consumer's filter/sort useMemo isn't handed
  // a fresh Set/Map reference every render (same reasoning as the desktop hook).
  /** Set of jsonlPaths that matched FTS, for quick lookup. */
  const ftsMatchPaths = useMemo(
    () => new Set(searchHits.map((h) => h.jsonlPath)),
    [searchHits],
  );

  /** Map from jsonlPath to best snippet for display. */
  const snippetByPath = useMemo(
    () => new Map(searchHits.map((h) => [h.jsonlPath, h.snippet])),
    [searchHits],
  );

  return { searchHits, searching, ftsMatchPaths, snippetByPath };
}
