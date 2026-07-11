import { invoke } from "@tauri-apps/api/core";
import { useEffect, useMemo, useRef, useState } from "react";
import type { SearchHit } from "../types";

/**
 * Debounced full-text search hook.
 * - For queries < 2 chars, returns empty (client-side filter handles it).
 * - For longer queries, debounces 300ms then calls the Rust search_sessions command.
 */
export function useSessionSearch(filter: string) {
  const [searchHits, setSearchHits] = useState<SearchHit[]>([]);
  const [searching, setSearching] = useState(false);
  const timerRef = useRef<number>(0);

  useEffect(() => {
    if (filter.trim().length < 2) {
      setSearchHits([]);
      setSearching(false);
      return;
    }

    setSearching(true);
    clearTimeout(timerRef.current);
    timerRef.current = window.setTimeout(async () => {
      try {
        const hits = await invoke<SearchHit[]>("search_sessions", {
          query: filter.trim(),
          limit: 50,
        });
        setSearchHits(hits);
      } catch {
        setSearchHits([]);
      } finally {
        setSearching(false);
      }
    }, 300);

    return () => clearTimeout(timerRef.current);
  }, [filter]);

  // Memoised on `searchHits` because HistoryView's row filter/sort useMemo
  // lists these in its dependency array. Rebuilding them every render would
  // hand it a fresh reference each time and silently defeat that memo, making
  // the launchpad re-filter and re-sort the whole session list on every render.
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
