// Debounced full-text search over the relay, mirroring the desktop
// useSessionSearch hook (claw-fleet-desktop/app/hooks/useSessionSearch.ts).
//
// The desktop hook invokes the local `search_sessions` Tauri command; here we
// issue a `session_search` request over the relay, which the desktop serves
// from the same local SearchIndex (see mobile_relay.rs::serve_request). Same
// contract: queries < 2 chars return empty (the caller's substring filter
// covers those), longer queries debounce 300ms then hit the index.
//
// Each device has its own index, so one search must **query each device separately**:
// asking only the current one means sessions on other machines get only title/preview
// substring matches, full-text hits are all invisible. Results are keyed by
// `itemKey(deviceId, jsonlPath)` — two Linux hosts can have identical jsonl paths,
// using bare path as key would splice snippets from one device into the other's.
import { useEffect, useMemo, useRef, useState } from "react";
import { itemKey } from "./deviceRuntime";
import type { FleetTransport } from "./transport";
import type { SearchHit } from "./types";

export function useRelaySearch(
  deviceIds: readonly string[],
  clientFor: (deviceId: string) => FleetTransport | null,
  filter: string,
) {
  const [searchHits, setSearchHits] = useState<Array<SearchHit & { deviceId: string }>>([]);
  const [searching, setSearching] = useState(false);
  const timerRef = useRef<number>(0);
  // Dependency uses string from joining: caller gives a new array each render, but device
  // set is basically stable.
  const key = useMemo(() => [...deviceIds].sort().join(" "), [deviceIds]);

  useEffect(() => {
    const ids = key ? key.split(" ") : [];
    if (ids.length === 0 || filter.trim().length < 2) {
      setSearchHits([]);
      setSearching(false);
      return;
    }

    setSearching(true);
    clearTimeout(timerRef.current);
    let alive = true;
    timerRef.current = window.setTimeout(() => {
      const q = filter.trim();
      Promise.all(
        ids.map((id) => {
          const transport = clientFor(id);
          if (!transport) return Promise.resolve([] as Array<SearchHit & { deviceId: string }>);
          return transport
            .request<SearchHit[]>("session_search", { query: q, limit: 50 })
            .then((hits) => (hits ?? []).map((h) => ({ ...h, deviceId: id })))
            // One device offline/timeout shouldn't wipe hits from other devices.
            .catch(() => [] as Array<SearchHit & { deviceId: string }>);
        }),
      )
        .then((perDevice) => {
          if (alive) setSearchHits(perDevice.flat());
        })
        .finally(() => {
          if (alive) setSearching(false);
        });
    }, 300);

    return () => {
      alive = false;
      clearTimeout(timerRef.current);
    };
  }, [key, clientFor, filter]);

  // Memoised on `searchHits` so the consumer's filter/sort useMemo isn't handed
  // a fresh Set/Map reference every render (same reasoning as the desktop hook).
  /** Set of hits, keyed by `itemKey(deviceId, jsonlPath)`. */
  const ftsMatchKeys = useMemo(
    () => new Set(searchHits.map((h) => itemKey(h.deviceId, h.jsonlPath))),
    [searchHits],
  );

  /** Same key → best snippet. */
  const snippetByKey = useMemo(
    () => new Map(searchHits.map((h) => [itemKey(h.deviceId, h.jsonlPath), h.snippet])),
    [searchHits],
  );

  return { searchHits, searching, ftsMatchKeys, snippetByKey };
}
