/**
 * Pure reducers for the 启动台 detail column's tab strip.
 *
 * Kept out of HistoryView so the fiddly part — *which tab holds focus after a
 * close* — is testable on its own. Getting that wrong is invisible in a type
 * check and obvious to the user: close the tab you're reading and you should
 * land on its right neighbour, not get dumped on the empty state.
 */

/** The tab strip's whole state: open tabs left→right, plus the focused one. */
export interface TabState {
  tabIds: string[];
  activeId: string | null;
}

/** Open `id`, or focus it if it is already open. New tabs append to the right. */
export function openTab(state: TabState, id: string): TabState {
  return {
    tabIds: state.tabIds.includes(id) ? state.tabIds : [...state.tabIds, id],
    activeId: id,
  };
}

/**
 * Close one tab. Closing the *focused* tab moves focus to its right neighbour,
 * falling back to its left when it was the last tab — the editor convention.
 * Closing a background tab leaves focus alone.
 */
export function closeTab(state: TabState, id: string): TabState {
  const idx = state.tabIds.indexOf(id);
  if (idx < 0) return state;
  const tabIds = state.tabIds.filter((t) => t !== id);
  if (state.activeId !== id) return { tabIds, activeId: state.activeId };
  // `idx` now indexes the former right neighbour, since everything after the
  // closed tab shifted left by one.
  return { tabIds, activeId: tabIds[idx] ?? tabIds[idx - 1] ?? null };
}

/** Close everything except `id`, which becomes the focused tab. */
export function closeOtherTabs(state: TabState, id: string): TabState {
  if (!state.tabIds.includes(id)) return state;
  return { tabIds: [id], activeId: id };
}

/** Close every tab to the right of `id`. Focus falls back to `id` only if the
 *  tab that had it was one of the ones closed. */
export function closeTabsToRight(state: TabState, id: string): TabState {
  const idx = state.tabIds.indexOf(id);
  if (idx < 0) return state;
  const tabIds = state.tabIds.slice(0, idx + 1);
  const activeId =
    state.activeId && tabIds.includes(state.activeId) ? state.activeId : id;
  return { tabIds, activeId };
}

/** Storage key for the persisted strip. Also listed in storage.ts's ALL_KEYS —
 *  a key missing from that whitelist writes fine but reads back empty. */
export const TABS_STORAGE_KEY = "launchpad-tabs";

const EMPTY: TabState = { tabIds: [], activeId: null };

/**
 * Parse the strip persisted from a previous run. Hostile to its own input on
 * purpose: a corrupt or half-written blob must degrade to "no tabs open", never
 * throw — an unparseable payload otherwise takes the whole 启动台 down with it.
 * `activeId` is also re-anchored, so a payload claiming focus on a tab that
 * isn't in the list falls back to the first one rather than focusing nothing.
 */
export function parsePersistedTabs(raw: string | null): TabState {
  if (!raw) return EMPTY;
  let parsed: unknown;
  try {
    parsed = JSON.parse(raw);
  } catch {
    return EMPTY;
  }
  if (typeof parsed !== "object" || parsed === null) return EMPTY;
  const { tabIds, activeId } = parsed as Record<string, unknown>;
  if (!Array.isArray(tabIds)) return EMPTY;
  const ids = tabIds.filter((id): id is string => typeof id === "string");
  if (ids.length === 0) return EMPTY;
  const active = typeof activeId === "string" && ids.includes(activeId)
    ? activeId
    : ids[0];
  return { tabIds: ids, activeId: active };
}

/** Drop tabs whose session no longer exists, re-anchoring focus if it was one
 *  of them. Run once the first scan lands, against the ids it actually saw. */
export function pruneMissingTabs(
  state: TabState,
  exists: (id: string) => boolean,
): TabState {
  const tabIds = state.tabIds.filter(exists);
  if (tabIds.length === state.tabIds.length) return state;
  const activeId =
    state.activeId && tabIds.includes(state.activeId)
      ? state.activeId
      : tabIds[0] ?? null;
  return { tabIds, activeId };
}

/** Move `fromId` into `toId`'s slot, shifting the rest along. */
export function reorderTabs(
  tabIds: string[],
  fromId: string,
  toId: string,
): string[] {
  const from = tabIds.indexOf(fromId);
  const to = tabIds.indexOf(toId);
  if (from < 0 || to < 0 || from === to) return tabIds;
  const next = [...tabIds];
  const [moved] = next.splice(from, 1);
  next.splice(to, 0, moved);
  return next;
}
