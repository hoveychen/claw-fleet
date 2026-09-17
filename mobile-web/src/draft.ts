// Persistence of unsubmitted form drafts. Partial input in the new session sheet / resume session
// composer should not be lost after unexpected sheet closure, tab switch, or iOS killing the PWA —
// falls to localStorage, auto-recovers on return, cleared after successful submission. Style aligns
// with localStorage usage in theme.ts / i18n.ts / secretStore.ts.

import { useCallback, useRef, useState } from "react";

const PREFIX = "fleet-draft:";

/** Subset of localStorage that we actually use; tests can inject an in-memory implementation (no window in Node). */
export type DraftStorage = Pick<Storage, "getItem" | "setItem" | "removeItem">;

/** window.localStorage may throw or not exist in private mode / SSR; returns null if unavailable.
 *  Draft persistence is best-effort throughout; lack of storage simply degrades to plain useState. */
function defaultStorage(): DraftStorage | null {
  try {
    return window.localStorage;
  } catch {
    return null;
  }
}

function isPlainObject(v: unknown): v is Record<string, unknown> {
  return typeof v === "object" && v !== null && !Array.isArray(v);
}

/** Load draft. Object-shaped drafts are shallow-merged with fallback — so when form fields are added
 *  later, missing fields in old drafts auto-get new defaults, no undefined reads. Non-objects (like plain strings) return parsed value directly. */
export function loadDraft<T>(
  key: string,
  fallback: T,
  store: DraftStorage | null = defaultStorage(),
): T {
  if (!store) return fallback;
  try {
    const raw = store.getItem(PREFIX + key);
    if (raw == null) return fallback;
    const parsed = JSON.parse(raw);
    if (isPlainObject(fallback) && isPlainObject(parsed)) {
      return { ...fallback, ...parsed } as T;
    }
    return parsed as T;
  } catch {
    return fallback;
  }
}

export function saveDraft<T>(
  key: string,
  value: T,
  store: DraftStorage | null = defaultStorage(),
): void {
  if (!store) return;
  try {
    store.setItem(PREFIX + key, JSON.stringify(value));
  } catch {
    // Quota full / private mode write rejected — persistence is a nice-to-have, failure doesn't affect form usability.
  }
}

export function clearDraft(
  key: string,
  store: DraftStorage | null = defaultStorage(),
): void {
  if (!store) return;
  try {
    store.removeItem(PREFIX + key);
  } catch {
    // Same as above, ignore.
  }
}

/** Clear all drafts under a prefix. Use when removing a device to sweep away its namespace
 *  (`d/<deviceId>/…`, see deviceScope.ts) — without clearing, each removal leaves behind unreachable
 *  drafts, attachment paths, and workspace memories.
 *
 *  Only works on stores that can enumerate keys (`Storage` has length/key(i), injected in-memory
 *  implementations usually don't), so enumeration capability is runtime-probed: if not detected, do nothing rather than throw. */
export function clearDraftsByPrefix(
  prefix: string,
  store: DraftStorage | null = defaultStorage(),
): void {
  const enumerable = store as (DraftStorage & Partial<Storage>) | null;
  if (!enumerable || typeof enumerable.length !== "number" || !enumerable.key) return;
  const full = PREFIX + prefix;
  const doomed: string[] = [];
  try {
    for (let i = 0; i < enumerable.length; i++) {
      const k = enumerable.key(i);
      if (k && k.startsWith(full)) doomed.push(k);
    }
    for (const k of doomed) enumerable.removeItem(k);
  } catch {
    // Same as above, ignore.
  }
}

/** Persistent version of useState: initial value recovered from draft, each set persists, clear() clears disk and resets to fallback. */
export function useDraft<T>(
  key: string,
  fallback: T,
): [T, (next: T | ((prev: T) => T)) => void, () => void] {
  // fallback is usually a fresh literal on each render; pin the initial value with ref to avoid reference drift on clear.
  const fallbackRef = useRef(fallback);
  const [value, setValue] = useState<T>(() => loadDraft(key, fallbackRef.current));

  const set = useCallback(
    (next: T | ((prev: T) => T)) => {
      setValue((prev) => {
        const resolved =
          typeof next === "function" ? (next as (p: T) => T)(prev) : next;
        saveDraft(key, resolved);
        return resolved;
      });
    },
    [key],
  );

  const clear = useCallback(() => {
    clearDraft(key);
    setValue(fallbackRef.current);
  }, [key]);

  return [value, set, clear];
}
