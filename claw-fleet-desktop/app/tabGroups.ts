/**
 * Editor *groups* for the 启动台 detail column — the split layer above
 * `sessionTabs.ts`.
 *
 * `sessionTabs.ts` owns one strip: which tabs are open and which one has focus.
 * This module holds N of those side by side (or stacked) and routes every
 * single-strip reducer at the group it belongs to, so the fiddly focus rules
 * proven there are reused rather than reimplemented per group.
 *
 * The layout is deliberately **flat** — a list of groups along one shared axis,
 * not VS Code's recursive row/column tree. Flat covers "compare these three
 * sessions" while keeping the state serializable in a dozen lines and the drop
 * targets one-dimensional.
 *
 * Two places where we diverge from VS Code on purpose:
 *
 *  - **Splitting opens an *empty* group.** VS Code shows the same editor in both
 *    groups because an editor is a stateless view of a file. A tab here is one
 *    live session pane with its own poller, scroll layer and loaded transcript;
 *    it cannot exist twice. So a split makes room and waits to be filled.
 *  - **A tab lives in exactly one group** (invariant 3 below), for the same
 *    reason.
 *
 * Four invariants every exported reducer preserves — each one fails silently
 * when broken, which is why `tabGroups.test.ts` asserts them centrally:
 *
 *   1. there is always at least one group
 *   2. `activeGroupId` always names a group that exists
 *   3. no tab id appears in two groups
 *   4. a group's `activeId` is always one of its own `tabIds` (or null)
 */

import {
  closeTab,
  openTab,
  parsePersistedTabs,
  replaceTab,
  type TabState,
} from "./sessionTabs";
import { parseTabKind } from "./tabKind";

/** One group: its open tabs left→right, the one it shows, and its share of the
 *  split axis. `weight` is relative — two groups at 1 each split the axis in
 *  half; the number only means anything against its siblings' total. */
export interface TabGroup {
  id: string;
  tabIds: string[];
  activeId: string | null;
  weight: number;
}

/** Which way the groups are laid out. `row` = side by side (split right),
 *  `column` = stacked (split down). Global, because the layout is flat. */
export type SplitOrientation = "row" | "column";

/** The whole detail column. */
export interface GroupsState {
  groups: TabGroup[];
  activeGroupId: string;
  orientation: SplitOrientation;
}

/** The group a freshly-installed (or freshly-corrupted) layout starts with. */
export const FIRST_GROUP_ID = "g1";

/** Every group starts with an equal share of the axis. */
export const DEFAULT_WEIGHT = 1;

/** Coerce a persisted weight. A non-finite or non-positive value would make a
 *  group zero-height (or poison the whole total via NaN), so anything odd falls
 *  back to an equal share rather than being trusted. */
function sanitizeWeight(raw: unknown): number {
  return typeof raw === "number" && Number.isFinite(raw) && raw > 0
    ? raw
    : DEFAULT_WEIGHT;
}

/**
 * Move the boundary between group `idx` and `idx + 1` by `deltaPx` along an axis
 * `axisPx` long, keeping both sides at least `minPx`.
 *
 * Weights are relative, so this converts to pixels, clamps there, and converts
 * back against the *unchanged* total — which is what keeps the groups either
 * side of the dragged boundary still.
 *
 * Returns the input array unchanged (same reference) for a no-op, so callers can
 * skip a re-render.
 */
export function resizeGroups(
  weights: number[],
  idx: number,
  deltaPx: number,
  axisPx: number,
  minPx: number,
): number[] {
  if (idx < 0 || idx + 1 >= weights.length) return weights;
  if (deltaPx === 0 || axisPx <= 0) return weights;
  const total = weights.reduce((a, b) => a + b, 0);
  if (total <= 0) return weights;
  const pxPerWeight = axisPx / total;
  const aPx = weights[idx] * pxPerWeight;
  const bPx = weights[idx + 1] * pxPerWeight;
  const pairPx = aPx + bPx;
  // The pair can't satisfy both minimums no matter where the boundary goes —
  // leave the layout alone instead of picking which side to violate.
  if (pairPx < minPx * 2) return weights;
  const nextAPx = Math.min(Math.max(aPx + deltaPx, minPx), pairPx - minPx);
  if (nextAPx === aPx) return weights;
  const out = [...weights];
  out[idx] = nextAPx / pxPerWeight;
  out[idx + 1] = (pairPx - nextAPx) / pxPerWeight;
  return out;
}

/**
 * Would one more group still leave every group at least `minPx`? Gates the split
 * controls so a split can never produce a column too narrow to read.
 *
 * `axisPx === 0` means the axis hasn't been measured yet (first paint, before
 * the ResizeObserver fires) and permits the split — refusing then would leave
 * the buttons dead on load for no reason.
 */
export function canFitAnotherGroup(
  axisPx: number,
  groupCount: number,
  minPx: number,
): boolean {
  if (axisPx <= 0) return true;
  return axisPx / (groupCount + 1) >= minPx;
}

/** Wrap a flat single-strip state into the one-group layout. */
export function singleGroup(tabs: TabState): GroupsState {
  return {
    groups: [
      {
        id: FIRST_GROUP_ID,
        tabIds: tabs.tabIds,
        activeId: tabs.activeId,
        weight: DEFAULT_WEIGHT,
      },
    ],
    activeGroupId: FIRST_GROUP_ID,
    orientation: "row",
  };
}

/**
 * A group id nobody holds. Derived from the highest existing `g<N>` rather than
 * the group count, because reclaiming a middle group would otherwise hand out an
 * id that is still on screen.
 */
export function nextGroupId(groups: TabGroup[]): string {
  let max = 0;
  for (const grp of groups) {
    const m = /^g(\d+)$/.exec(grp.id);
    if (m) max = Math.max(max, Number(m[1]));
  }
  return `g${max + 1}`;
}

/** The focused group. Invariant 2 makes the fallback unreachable in practice. */
export function activeGroup(state: GroupsState): TabGroup {
  return state.groups.find((g) => g.id === state.activeGroupId) ?? state.groups[0];
}

/** Which group holds `tabId`, or null. */
export function findGroupOf(state: GroupsState, tabId: string): TabGroup | null {
  return state.groups.find((g) => g.tabIds.includes(tabId)) ?? null;
}

/** One visible tab per group — exactly the set that must stay *unpaused*.
 *  Everything else is a background tab and keeps its pollers frozen. */
export function visibleTabIds(state: GroupsState): Set<string> {
  const ids = new Set<string>();
  for (const grp of state.groups) if (grp.activeId) ids.add(grp.activeId);
  return ids;
}

/**
 * Drop the group at `idx`, moving focus to its right neighbour and falling back
 * to its left — the same rule `closeTab` uses for tabs, one level up. Only ever
 * called with more than one group, so the result cannot be empty.
 */
function reclaim(state: GroupsState, idx: number): GroupsState {
  const removed = state.groups[idx];
  const groups = state.groups.filter((_, i) => i !== idx);
  // `idx` now indexes the former right neighbour, everything having shifted left.
  const activeGroupId =
    state.activeGroupId === removed.id
      ? (groups[idx] ?? groups[idx - 1]).id
      : state.activeGroupId;
  return { ...state, groups, activeGroupId };
}

/**
 * Run a single-strip reducer against one group. This is the seam that lets every
 * reducer in `sessionTabs.ts` work per group without knowing groups exist.
 *
 * A group emptied by the reducer is reclaimed — the editor convention, and what
 * `workbench.editor.closeEmptyGroups` does by default. The *last* group is
 * exempt: it stays as an empty shell so the column always has somewhere to
 * render the "pick a session" hint.
 */
export function inGroup(
  state: GroupsState,
  groupId: string,
  fn: (tabs: TabState) => TabState,
): GroupsState {
  const idx = state.groups.findIndex((g) => g.id === groupId);
  if (idx < 0) return state;
  const grp = state.groups[idx];
  const next = fn({ tabIds: grp.tabIds, activeId: grp.activeId });
  // Reference-equal tab list plus unchanged focus means the reducer bailed;
  // hand back the same object so React can skip the re-render too.
  if (next.tabIds === grp.tabIds && next.activeId === grp.activeId) return state;
  if (next.tabIds.length === 0 && state.groups.length > 1) return reclaim(state, idx);
  const groups = [...state.groups];
  groups[idx] = { ...grp, tabIds: next.tabIds, activeId: next.activeId };
  return { ...state, groups };
}

/**
 * Split `groupId`, inserting a new empty group immediately after it and moving
 * focus there. `orientation` re-lays out the whole column, since a flat layout
 * has only one axis — splitting down from a side-by-side layout stacks
 * everything.
 */
export function splitGroup(
  state: GroupsState,
  groupId: string,
  orientation: SplitOrientation,
): GroupsState {
  const idx = state.groups.findIndex((g) => g.id === groupId);
  if (idx < 0) return state;
  const id = nextGroupId(state.groups);
  const groups = [...state.groups];
  // Equal share: a split makes room, it doesn't take sides about how much.
  groups.splice(idx + 1, 0, { id, tabIds: [], activeId: null, weight: DEFAULT_WEIGHT });
  return { groups, activeGroupId: id, orientation };
}

/** Move focus to a group. No-op for an id that is gone or already focused. */
export function focusGroup(state: GroupsState, groupId: string): GroupsState {
  if (state.activeGroupId === groupId) return state;
  if (!state.groups.some((g) => g.id === groupId)) return state;
  return { ...state, activeGroupId: groupId };
}

/** Focus the `pos`-th group, 1-based, for the ⌘1..⌘9 bindings. Clamped, so ⌘9
 *  lands on the last group instead of doing nothing — as it does in VS Code. */
export function focusGroupAt(state: GroupsState, pos: number): GroupsState {
  const idx = Math.min(Math.max(pos - 1, 0), state.groups.length - 1);
  return focusGroup(state, state.groups[idx].id);
}

/**
 * Move `tabId` into `toGroupId`, landing before `beforeTabId` when given (so a
 * drop lands where the cursor showed it) and at the right end otherwise. The tab
 * takes focus in its new group, and the group it left re-anchors its own focus
 * exactly as closing the tab would. A source group emptied by the move is
 * reclaimed; a source group that was *already* empty is not, so a group the user
 * split open on purpose survives.
 *
 * Same source and destination degrades to a reorder.
 */
export function moveTabToGroup(
  state: GroupsState,
  tabId: string,
  toGroupId: string,
  beforeTabId?: string,
): GroupsState {
  const from = findGroupOf(state, tabId);
  if (!from) return state;
  if (!state.groups.some((g) => g.id === toGroupId)) return state;

  let groups = state.groups.map((grp) => {
    if (grp.id === from.id && grp.id !== toGroupId) {
      return {
        ...grp,
        tabIds: grp.tabIds.filter((t) => t !== tabId),
        activeId:
          grp.activeId === tabId
            ? closeTab({ tabIds: grp.tabIds, activeId: grp.activeId }, tabId).activeId
            : grp.activeId,
      };
    }
    if (grp.id === toGroupId) {
      const base = grp.tabIds.filter((t) => t !== tabId);
      const at = beforeTabId ? base.indexOf(beforeTabId) : -1;
      const tabIds =
        at >= 0 ? [...base.slice(0, at), tabId, ...base.slice(at)] : [...base, tabId];
      return { ...grp, tabIds, activeId: tabId };
    }
    return grp;
  });

  if (from.id !== toGroupId && groups.length > 1) {
    const srcIdx = groups.findIndex((g) => g.id === from.id);
    if (srcIdx >= 0 && groups[srcIdx].tabIds.length === 0) {
      groups = groups.filter((_, i) => i !== srcIdx);
    }
  }
  return { ...state, groups, activeGroupId: toGroupId };
}

/**
 * Open `tabId` in the focused group. A tab another group already holds is
 * *revealed* there rather than duplicated (invariant 3) — clicking a list row
 * for a session that is open in the other half moves focus to that half, which
 * is also what double-clicking a file does in VS Code.
 */
export function openTabInActiveGroup(state: GroupsState, tabId: string): GroupsState {
  const holder = findGroupOf(state, tabId);
  if (holder) {
    const revealed = inGroup(state, holder.id, (t) => ({ ...t, activeId: tabId }));
    return focusGroup(revealed, holder.id);
  }
  return inGroup(state, state.activeGroupId, (t) => openTab(t, tabId));
}

/**
 * What a tab is *for*. Two kinds of thing end up in this column: the
 * conversation you are working through, and the material it cites.
 */
export type TabAffinity = "primary" | "side";

/** A session (either view of one, or the draft that becomes one) is the work; a
 *  file, a wiki doc and a web page are what you read beside it. A second view is
 *  still a session pane — so a path clicked inside one routes to the docs half
 *  exactly as it does from the first view, rather than landing on top of it. */
export function tabAffinity(id: string): TabAffinity {
  const kind = parseTabKind(id).kind;
  return kind === "session" || kind === "sessionview" || kind === "draft"
    ? "primary"
    : "side";
}

/** Does this group already hold tabs of that affinity — i.e. is it a home for
 *  them? A group the user mixed by hand is a home for both, deliberately: the
 *  heuristic exists to save arranging, not to overrule an arrangement. */
function isHomeFor(grp: TabGroup, want: TabAffinity): boolean {
  return grp.tabIds.some((id) => tabAffinity(id) === want);
}

/**
 * Open `tabId` in the group where it belongs, splitting one open if that kind of
 * thing has no home yet.
 *
 * This is the layout doing the arranging: a doc or page cited by a conversation
 * lands *beside* it rather than on top of it, which is the whole reason to have
 * the two on screen at once. The rules, in order:
 *
 *  1. already open somewhere → revealed there (invariant 3), as before;
 *  2. the focused group is empty → it gets the tab, whatever the kind. The user
 *     split it open on purpose; routing past it would leave a blank half;
 *  3. the focused group already holds this kind → it gets the tab, so reading a
 *     second doc while parked among docs doesn't hop the column around;
 *  4. another group holds this kind → that one gets it (nearest to the focus,
 *     the right-hand side winning ties), and takes focus;
 *  5. nothing holds this kind and there is room → a new group, right after the
 *     focused one, along the column's current axis;
 *  6. no room (`canSplit` false — the column is too narrow, or already at the
 *     group cap) → the focused group, exactly as before this heuristic existed.
 *
 * `canSplit` is passed in rather than computed here because the width gate needs
 * the *measured* axis, which only the component knows.
 */
export function openTabRouted(
  state: GroupsState,
  tabId: string,
  canSplit: boolean,
): GroupsState {
  const holder = findGroupOf(state, tabId);
  if (holder) {
    const revealed = inGroup(state, holder.id, (t) => ({ ...t, activeId: tabId }));
    return focusGroup(revealed, holder.id);
  }

  const active = activeGroup(state);
  const want = tabAffinity(tabId);
  if (active.tabIds.length === 0 || isHomeFor(active, want)) {
    return inGroup(state, active.id, (t) => openTab(t, tabId));
  }

  const activeIdx = state.groups.findIndex((g) => g.id === active.id);
  let best: { id: string; dist: number } | null = null;
  state.groups.forEach((grp, i) => {
    if (grp.id === active.id || !isHomeFor(grp, want)) return;
    const dist = Math.abs(i - activeIdx);
    // `<` keeps the first of equal distances, and the forEach reaches the
    // right-hand candidate first only when it is strictly nearer — so a tie
    // between i-1 and i+1 has to be broken explicitly.
    if (!best || dist < best.dist || (dist === best.dist && i > activeIdx)) {
      best = { id: grp.id, dist };
    }
  });
  if (best) {
    const target = (best as { id: string }).id;
    return focusGroup(
      inGroup(state, target, (t) => openTab(t, tabId)),
      target,
    );
  }

  if (!canSplit) return inGroup(state, active.id, (t) => openTab(t, tabId));
  const split = splitGroup(state, active.id, state.orientation);
  return inGroup(split, split.activeGroupId, (t) => openTab(t, tabId));
}

/** Close a tab whichever group holds it — the strip's ✕ and middle-click, plus
 *  the list row's "close" for a tab parked in a background group. */
export function closeTabAnywhere(state: GroupsState, tabId: string): GroupsState {
  const holder = findGroupOf(state, tabId);
  if (!holder) return state;
  return inGroup(state, holder.id, (t) => closeTab(t, tabId));
}

/** Close a whole group and every tab in it. The last group empties instead of
 *  disappearing, per invariant 1. */
export function closeGroup(state: GroupsState, groupId: string): GroupsState {
  const idx = state.groups.findIndex((g) => g.id === groupId);
  if (idx < 0) return state;
  if (state.groups.length === 1) {
    if (state.groups[0].tabIds.length === 0) return state;
    return {
      ...state,
      groups: [{ ...state.groups[0], tabIds: [], activeId: null }],
    };
  }
  return reclaim(state, idx);
}

/** Morph the draft tab into its real session id, in whichever group holds it.
 *  Falls back to opening in the focused group when the draft is already gone. */
export function replaceTabAnywhere(
  state: GroupsState,
  oldId: string,
  newId: string,
): GroupsState {
  const holder = findGroupOf(state, oldId);
  if (!holder) return openTabInActiveGroup(state, newId);
  return inGroup(state, holder.id, (t) => replaceTab(t, oldId, newId));
}

/**
 * Drop tabs whose session no longer exists, across every group. A group emptied
 * *by this prune* is reclaimed; a group that was already empty is left alone, so
 * a freshly split group isn't eaten by the next scan.
 */
export function pruneMissingGroupTabs(
  state: GroupsState,
  exists: (id: string) => boolean,
): GroupsState {
  let changed = false;
  const mapped = state.groups.map((grp) => {
    const tabIds = grp.tabIds.filter(exists);
    if (tabIds.length === grp.tabIds.length) return grp;
    changed = true;
    const activeId =
      grp.activeId && tabIds.includes(grp.activeId) ? grp.activeId : tabIds[0] ?? null;
    return { ...grp, tabIds, activeId };
  });
  if (!changed) return state;

  let groups = mapped.filter(
    (grp, i) => grp.tabIds.length > 0 || state.groups[i].tabIds.length === 0,
  );
  if (groups.length === 0) groups = [{ ...mapped[0], tabIds: [], activeId: null }];
  const activeGroupId = groups.some((g) => g.id === state.activeGroupId)
    ? state.activeGroupId
    : groups[0].id;
  return { ...state, groups, activeGroupId };
}

/** What goes into localStorage. Key and call site are unchanged from the
 *  pre-split layout — see `TABS_STORAGE_KEY` in sessionTabs.ts. */
export function serializeGroups(state: GroupsState): string {
  return JSON.stringify(state);
}

/**
 * Parse the layout persisted by a previous run. Hostile to its own input, and
 * responsible for two migrations:
 *
 *  - a **pre-split blob** (`{tabIds, activeId}`, everything written before this
 *    module existed) becomes one group, so upgrading doesn't silently close
 *    every tab the user had open;
 *  - a blob whose groups disagree (same tab twice, focus on a tab that isn't
 *    there, `activeGroupId` naming a group that isn't) is repaired against the
 *    four invariants rather than trusted.
 *
 * Empty groups are dropped on the way in: a split the user never filled is not
 * worth restoring across a restart, and it would otherwise come back as a blank
 * half of the column with no obvious way to read it.
 */
export function parsePersistedGroups(raw: string | null): GroupsState {
  const empty = () => singleGroup({ tabIds: [], activeId: null });
  if (!raw) return empty();
  let parsed: unknown;
  try {
    parsed = JSON.parse(raw);
  } catch {
    return empty();
  }
  if (typeof parsed !== "object" || parsed === null) return empty();
  const rec = parsed as Record<string, unknown>;

  // Pre-split shape: no `groups` key. Reuse the old parser, which already
  // re-anchors focus and rejects junk, then wrap its result.
  if (!Array.isArray(rec.groups)) return singleGroup(parsePersistedTabs(raw));

  const seen = new Set<string>();
  const groups: TabGroup[] = [];
  for (const entry of rec.groups) {
    if (typeof entry !== "object" || entry === null) continue;
    const gr = entry as Record<string, unknown>;
    if (typeof gr.id !== "string" || !Array.isArray(gr.tabIds)) continue;
    if (groups.some((g) => g.id === gr.id)) continue;
    const tabIds: string[] = [];
    for (const t of gr.tabIds) {
      if (typeof t !== "string" || seen.has(t)) continue;
      seen.add(t);
      tabIds.push(t);
    }
    if (tabIds.length === 0) continue;
    const activeId =
      typeof gr.activeId === "string" && tabIds.includes(gr.activeId)
        ? gr.activeId
        : tabIds[0];
    groups.push({ id: gr.id, tabIds, activeId, weight: sanitizeWeight(gr.weight) });
  }
  if (groups.length === 0) return empty();

  const activeGroupId =
    typeof rec.activeGroupId === "string" && groups.some((g) => g.id === rec.activeGroupId)
      ? rec.activeGroupId
      : groups[0].id;
  return {
    groups,
    activeGroupId,
    orientation: rec.orientation === "column" ? "column" : "row",
  };
}
