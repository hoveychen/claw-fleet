import { useCallback, useEffect, useMemo, useRef, useState } from "react";

import {
  dismissExplanation,
  explainSelection,
  getExplanation,
  listExplanations,
  pollExplanation,
  type ExplainRecord,
  type ExplainRequest,
} from "../explainApi";

/**
 * The side questions asked about one session, kept current.
 *
 * Records live on disk (`~/.fleet/explain/<session>/`), so on every session
 * switch the list is re-read and any record still `running` is polled until it
 * settles — including one started by another surface (the phone) or before the
 * app was last closed. `ask` submits a new one and starts polling it; the
 * caller sees the `running` record at once and its text grow from there.
 *
 * `dismiss` hides a card for good: the flag is written to the store, so the ✕
 * survives a session switch and an app restart (it used to live in this hook's
 * state alone, which meant every switch handed the card straight back). The
 * record itself is never deleted — they are the reader's notes on the run,
 * "放着" was the ask — so `restore` can always bring one back.
 *
 * `all` is the unfiltered list — what the library facet lists, so a question
 * dismissed from the rail still has somewhere to be found and `restore` can
 * put it back.
 */
export function useSessionExplains(sessionId: string | undefined): {
  explains: ExplainRecord[];
  all: ExplainRecord[];
  hidden: ReadonlySet<string>;
  ask: (req: ExplainRequest) => Promise<ExplainRecord>;
  dismiss: (id: string) => void;
  restore: (id: string) => void;
} {
  const [records, setRecords] = useState<ExplainRecord[]>([]);
  const [hidden, setHidden] = useState<ReadonlySet<string>>(() => new Set());
  const pollers = useRef(new Map<string, AbortController>());
  // The session the state belongs to, readable from async callbacks so a
  // reply that lands after a switch cannot seed the next session's rail.
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
      if (pollers.current.has(id)) return;
      const ctl = new AbortController();
      pollers.current.set(id, ctl);
      pollExplanation(() => getExplanation(sid, id), upsert, { signal: ctl.signal })
        .catch((e) => console.error("explain poll failed:", e))
        .finally(() => {
          if (pollers.current.get(id) === ctl) pollers.current.delete(id);
        });
    },
    [upsert],
  );

  const stopAll = useCallback(() => {
    for (const ctl of pollers.current.values()) ctl.abort();
    pollers.current.clear();
  }, []);

  useEffect(() => {
    stopAll();
    setRecords([]);
    setHidden(new Set());
    if (!sessionId) return;
    let alive = true;
    listExplanations(sessionId)
      .then((list) => {
        if (!alive || current.current !== sessionId) return;
        setRecords([...list].sort((a, b) => a.createdMs - b.createdMs || a.id.localeCompare(b.id)));
        // The store carries the dismissals, so the rail opens where it was left.
        setHidden(new Set(list.filter((r) => r.dismissed).map((r) => r.id)));
        for (const r of list) if (r.status === "running") track(sessionId, r.id);
      })
      .catch((e) => console.error("list_explanations failed:", e));
    return () => {
      alive = false;
      stopAll();
    };
  }, [sessionId, track, stopAll]);

  const ask = useCallback(
    async (req: ExplainRequest) => {
      let rec: ExplainRecord;
      try {
        rec = await explainSelection(req);
      } catch (e) {
        // A refused ask (no source owns the session, the fork failed to
        // spawn) is shown where the answer would have been: a failed card
        // carrying the message, local to this view since nothing was stored.
        const now = Date.now();
        rec = {
          id: `local-${now}`,
          sessionId: req.sessionId,
          source: "",
          createdMs: now,
          updatedMs: now,
          preset: req.preset,
          quote: req.quote,
          question: req.question ?? "",
          anchor: req.anchor,
          thread: req.thread ?? [],
          status: "error",
          text: "",
          error: e instanceof Error ? e.message : String(e),
          inputTokens: 0,
          outputTokens: 0,
          cacheReadTokens: 0,
          cacheCreationTokens: 0,
          durationMs: 0,
          // Never reached the store, so there is no dismissal to read back.
          dismissed: false,
        };
      }
      upsert(rec);
      if (rec.status === "running") track(req.sessionId, rec.id);
      return rec;
    },
    [upsert, track],
  );

  /* The card leaves (or returns to) the rail on the click; the store catches up
     behind it. A refused write is logged rather than bounced back into the UI:
     an ask that never reached the store (a `local-` error card) has nothing to
     persist, and the card is still where the reader put it for this view. */
  const persistDismissed = useCallback(
    (id: string, dismissed: boolean) => {
      const sid = current.current;
      if (!sid || id.startsWith("local-")) return;
      dismissExplanation(sid, id, dismissed).catch((e) =>
        console.error("dismiss_explanation failed:", e),
      );
    },
    [],
  );

  const dismiss = useCallback(
    (id: string) => {
      persistDismissed(id, true);
      setHidden((prev) => {
        if (prev.has(id)) return prev;
        const next = new Set(prev);
        next.add(id);
        return next;
      });
    },
    [persistDismissed],
  );

  const restore = useCallback(
    (id: string) => {
      persistDismissed(id, false);
      setHidden((prev) => {
        if (!prev.has(id)) return prev;
        const next = new Set(prev);
        next.delete(id);
        return next;
      });
    },
    [persistDismissed],
  );

  const explains = useMemo(() => records.filter((r) => !hidden.has(r.id)), [records, hidden]);
  return { explains, all: records, hidden, ask, dismiss, restore };
}
