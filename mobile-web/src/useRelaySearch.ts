// Debounced full-text search over the relay, mirroring the desktop
// useSessionSearch hook (claw-fleet-desktop/app/hooks/useSessionSearch.ts).
//
// The desktop hook invokes the local `search_sessions` Tauri command; here we
// issue a `session_search` request over the relay, which the desktop serves
// from the same local SearchIndex (see mobile_relay.rs::serve_request). Same
// contract: queries < 2 chars return empty (the caller's substring filter
// covers those), longer queries debounce 300ms then hit the index.
//
// 每台设备各有自己的索引，所以一次搜索要**向每台设备各问一次**：只问当前那台，
// 别的机器的会话就只剩标题/预览的子串匹配，正文命中全都看不见。结果按
// `itemKey(deviceId, jsonlPath)` 归档 —— 两台 Linux 主机上的 jsonl 路径可以
// 一模一样，裸路径当键会把另一台的片段贴到这一条上。
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
  // 依赖用拼好的字符串：调用方每次快照都会给出一个新数组，但设备集合基本不变。
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
            // 一台离线/超时不该把别的设备的命中一起清空。
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
  /** 命中集合，键是 `itemKey(deviceId, jsonlPath)`。 */
  const ftsMatchKeys = useMemo(
    () => new Set(searchHits.map((h) => itemKey(h.deviceId, h.jsonlPath))),
    [searchHits],
  );

  /** 同款键 → 最佳片段。 */
  const snippetByKey = useMemo(
    () => new Map(searchHits.map((h) => [itemKey(h.deviceId, h.jsonlPath), h.snippet])),
    [searchHits],
  );

  return { searchHits, searching, ftsMatchKeys, snippetByKey };
}
