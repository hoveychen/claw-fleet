import { useCallback, useSyncExternalStore } from "react";

import {
  DOC_HISTORY_KEY,
  docHistoryFor,
  forgetDoc,
  forgetSession,
  parseDocHistory,
  recordDoc,
  serializeDocHistory,
  type DocHistoryEntry,
  type DocHistoryMap,
} from "../docHistory";
import type { AuxDocKind } from "../detailAux";
import { getItem, setItem } from "../storage";

/**
 * The reading list behind the rail, shared by the one surface that writes it
 * (SessionDetail, on every `openDoc`) and the one that reads it (the session
 * facet panel, listing what can be reopened).
 *
 * A module-level snapshot rather than React state or a zustand slice: writes
 * come from a callback deep in SessionDetail and reads from a panel mounted
 * beside it, and the value has to be readable *synchronously* on first render
 * because it is already on disk when the app boots. `useSyncExternalStore` is
 * exactly that contract, and it keeps the persistence in one place — nothing
 * outside this file touches the storage key.
 */

let snapshot: DocHistoryMap | null = null;
const listeners = new Set<() => void>();

function current(): DocHistoryMap {
  // Parsed on first read rather than at import time: `initStorage` has to have
  // populated the synchronous cache first, and module import order is not a
  // guarantee worth resting on.
  if (snapshot == null) snapshot = parseDocHistory(getItem(DOC_HISTORY_KEY));
  return snapshot;
}

function commit(next: DocHistoryMap): void {
  if (next === snapshot) return;
  snapshot = next;
  setItem(DOC_HISTORY_KEY, serializeDocHistory(next));
  for (const fn of listeners) fn();
}

function subscribe(fn: () => void): () => void {
  listeners.add(fn);
  return () => {
    listeners.delete(fn);
  };
}

/** Record an opened doc. Safe to call from anywhere — it is a plain function,
 *  not a hook, so the transcript's link handlers can reach it. */
export function rememberDoc(
  sessionId: string,
  kind: AuxDocKind,
  ref: string,
  label: string,
): void {
  const entry: DocHistoryEntry = { kind, ref, label, ts: Date.now() };
  commit(recordDoc(current(), sessionId, entry));
}

/** Test seam: drop the in-memory snapshot so the next read re-parses storage. */
export function resetDocHistoryCache(): void {
  snapshot = null;
}

/** One session's reading list, newest first, plus the two ways to shorten it. */
export function useDocHistory(sessionId: string | undefined): {
  docs: DocHistoryEntry[];
  forget: (kind: AuxDocKind, ref: string) => void;
  forgetAll: () => void;
} {
  const map = useSyncExternalStore(subscribe, current, current);
  const docs = sessionId ? docHistoryFor(map, sessionId) : [];
  const forget = useCallback(
    (kind: AuxDocKind, ref: string) => {
      if (!sessionId) return;
      commit(forgetDoc(current(), sessionId, kind, ref));
    },
    [sessionId],
  );
  const forgetAll = useCallback(() => {
    if (!sessionId) return;
    commit(forgetSession(current(), sessionId));
  }, [sessionId]);
  return { docs, forget, forgetAll };
}
