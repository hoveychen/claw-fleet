import { useCallback, useEffect, useRef, useState } from "react";

import { pollExplanation } from "../../../shared-ts/sessionExplain";
import {
  askExplanation,
  getExplanation,
  listExplanations,
  refusedExplanation,
  type ExplainRecord,
  type ExplainRequest,
} from "../sessionExplain";
import type { FleetTransport } from "../transport";

/**
 * The side questions asked about one session, kept current over the relay.
 *
 * Records live on the host (`~/.fleet/explain/<session>/`), so on every
 * session switch the list is re-read and any record still `running` is polled
 * until it settles — including one started from the desktop, or before the
 * app was last in the foreground. `ask` submits a new one and starts polling
 * it; the caller sees the `running` record at once and its text grow from
 * there. Same contract as the desktop's `useSessionExplains`, with the
 * transport injected instead of Tauri `invoke`.
 *
 * `loaded` tells "the list came back empty" from "we have not heard yet", so
 * the sheet's readout can say 无 without lying about a slow link.
 */
export function useSessionExplains(
  client: FleetTransport | null,
  sessionId: string | undefined,
): {
  explains: ExplainRecord[];
  loaded: boolean;
  ask: (req: ExplainRequest) => Promise<ExplainRecord>;
} {
  const [records, setRecords] = useState<ExplainRecord[]>([]);
  const [loaded, setLoaded] = useState(false);
  const pollers = useRef(new Map<string, AbortController>());
  // The session the state belongs to, readable from async callbacks so a
  // reply that lands after a switch cannot seed the next session's list.
  const current = useRef<string | undefined>(sessionId);
  current.current = sessionId;

  const upsert = useCallback((rec: ExplainRecord) => {
    if (rec.sessionId !== current.current) return;
    setRecords((prev) => {
      const i = prev.findIndex((r) => r.id === rec.id);
      if (i < 0) {
        return [...prev, rec].sort((a, b) => a.createdMs - b.createdMs || a.id.localeCompare(b.id));
      }
      const next = prev.slice();
      next[i] = rec;
      return next;
    });
  }, []);

  const track = useCallback(
    (sid: string, id: string) => {
      if (!client || pollers.current.has(id)) return;
      const ctl = new AbortController();
      pollers.current.set(id, ctl);
      pollExplanation(() => getExplanation(client, sid, id), upsert, { signal: ctl.signal })
        .catch((e) => console.error("explain poll failed:", e))
        .finally(() => {
          if (pollers.current.get(id) === ctl) pollers.current.delete(id);
        });
    },
    [client, upsert],
  );

  const stopAll = useCallback(() => {
    for (const ctl of pollers.current.values()) ctl.abort();
    pollers.current.clear();
  }, []);

  useEffect(() => {
    stopAll();
    setRecords([]);
    setLoaded(false);
    if (!client || !sessionId) return;
    let alive = true;
    listExplanations(client, sessionId)
      .then((list) => {
        if (!alive || current.current !== sessionId) return;
        setRecords([...list].sort((a, b) => a.createdMs - b.createdMs || a.id.localeCompare(b.id)));
        setLoaded(true);
        for (const r of list) if (r.status === "running") track(sessionId, r.id);
      })
      .catch((e) => console.error("session_explain_list failed:", e));
    return () => {
      alive = false;
      stopAll();
    };
  }, [client, sessionId, track, stopAll]);

  const ask = useCallback(
    async (req: ExplainRequest) => {
      let rec: ExplainRecord;
      if (!client) {
        rec = refusedExplanation(req, new Error("offline"));
      } else {
        try {
          rec = await askExplanation(client, req);
        } catch (e) {
          rec = refusedExplanation(req, e);
        }
      }
      upsert(rec);
      if (rec.status === "running") track(req.sessionId, rec.id);
      return rec;
    },
    [client, upsert, track],
  );

  return { explains: records, loaded, ask };
}
