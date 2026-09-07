/**
 * State for the session detail's two auxiliary surfaces.
 *
 * They are two *layers* of information, and the whole point of this module is
 * that they no longer share one control:
 *
 * 1. **The rail** — a permanent column of cards to the right of the
 *    conversation: the subagents running right now, and every file / wiki doc /
 *    page the agent named and the reader opened. This is "what is going on and
 *    what am I looking at" — ambient, plural, glanceable, and gone entirely
 *    (zero width) when there is nothing in it. Its content is *derived*: live
 *    subagents come from the sessions store, docs from `docs` below. There is no
 *    open/close state for it, because a column with nothing in it does not
 *    render at all.
 *
 * 2. **The drawer** — an overlay panel that floats over the transcript and
 *    shows exactly one thing at a time: a session facet (Skills, 决策, Token,
 *    任务, 后台任务, 临时文件, Workflow) picked from the header menu, or the
 *    full-width reader for one doc card. This is "go look something up" —
 *    singular, deliberate, dismissed when you are done. `active` is that one
 *    thing.
 *
 * They used to be one tab strip over one panel, which is what made a running
 * subagent and a token receipt compete for the same slot.
 *
 * Pure module, no React: the reducer is the part worth testing, and neither
 * surface should have to be mounted to test it.
 */

/** A session facet — a panel scoped to this session, read in the drawer. */
export type AuxFacet =
  | "skills"
  | "decisions"
  | "tokens"
  | "tasks"
  | "bgtasks"
  | "scratchpad"
  | "workflow";

const FACETS: readonly AuxFacet[] = [
  "skills",
  "decisions",
  "tokens",
  "tasks",
  "bgtasks",
  "scratchpad",
  "workflow",
];

/** A facet as offered in the header's overflow menu: the id to open plus the
 *  label (with its count, when it has one) to show. */
export interface AuxFacetItem {
  id: AuxFacet;
  label: string;
}

export function isAuxFacet(value: unknown): value is AuxFacet {
  return FACETS.includes(value as AuxFacet);
}

/** A document the agent named and the reader opened: a repo file, a wiki doc,
 *  or a web page. */
export type AuxDocKind = "file" | "wiki" | "web";

export interface AuxDoc {
  /** Identity *and* the value stored in `active`. Prefixed by kind, so it can
   *  never collide with a facet name. */
  id: string;
  kind: AuxDocKind;
  /** Absolute path / wiki slug / url, by kind. */
  ref: string;
  /** What the card shows. */
  label: string;
}

export interface AuxState {
  /** Doc cards in the rail, most recently opened last. */
  docs: AuxDoc[];
  /** What the drawer is showing: a facet name or a doc id. `null` means the
   *  drawer is closed — which says nothing about the rail. */
  active: string | null;
  /** The last thing the drawer showed, so the toolbar's switch can put it back
   *  instead of guessing. Survives closing; cleared when the thing itself is
   *  gone (`closeDoc`, `pruneTab`). */
  last: string | null;
}

export const initialAux: AuxState = { docs: [], active: null, last: null };

/** Cap on remembered doc cards. A long session can name dozens of files; the
 *  rail is a "what I have been reading" stack, not a history. Oldest drops
 *  first, never the one just opened. */
export const MAX_AUX_DOCS = 8;

export function docId(kind: AuxDocKind, ref: string): string {
  return `${kind}:${ref}`;
}

/** Last path segment, tolerating either separator so a Windows path reads the
 *  same as a POSIX one. */
function basename(p: string): string {
  const cut = Math.max(p.lastIndexOf("/"), p.lastIndexOf("\\"));
  return cut >= 0 ? p.slice(cut + 1) : p;
}

export function auxDocLabel(kind: AuxDocKind, ref: string): string {
  if (kind === "web") {
    try {
      return new URL(ref).host;
    } catch {
      // A hand-typed or malformed url still deserves a readable label.
      return ref;
    }
  }
  return basename(ref);
}

export function makeAuxDoc(kind: AuxDocKind, ref: string): AuxDoc {
  return { id: docId(kind, ref), kind, ref, label: auxDocLabel(kind, ref) };
}

/**
 * Click a rail card (or a facet already showing): show it in the drawer, or —
 * clicking the one already showing — close the drawer. A toggle, so a card
 * doubles as the off switch for the panel it opened.
 */
export function toggleTab(state: AuxState, id: string): AuxState {
  if (state.active === id) return closeAux(state);
  return showTab(state, id);
}

/** Open something in the drawer without the toggle-off half — for the header
 *  menu's facet items, the toolbar switch, and jumps from elsewhere (a clicked
 *  plan row). */
export function showTab(state: AuxState, id: string): AuxState {
  if (state.active === id) return state;
  return { ...state, active: id, last: id };
}

/** Open a doc: card it if new (never a second copy), then read it in the
 *  drawer. Both layers move, because naming a file in the transcript is both
 *  "this is now part of my context" and "show it to me". */
export function openDoc(state: AuxState, kind: AuxDocKind, ref: string): AuxState {
  const doc = makeAuxDoc(kind, ref);
  const known = state.docs.some((d) => d.id === doc.id);
  let docs = known ? state.docs : [...state.docs, doc];
  // The doc we are about to open is last, so trimming from the front can never
  // drop it.
  if (docs.length > MAX_AUX_DOCS) docs = docs.slice(docs.length - MAX_AUX_DOCS);
  return { ...state, docs, active: doc.id, last: doc.id };
}

/**
 * Dismiss one doc card.
 *
 * If the drawer was reading it, the drawer closes: there is no strip to slide
 * along any more, and silently swapping in a neighbouring file would be the
 * drawer deciding what you read next.
 */
export function closeDoc(state: AuxState, id: string): AuxState {
  const idx = state.docs.findIndex((d) => d.id === id);
  if (idx < 0) return state;
  const docs = state.docs.filter((d) => d.id !== id);
  return {
    docs,
    active: state.active === id ? null : state.active,
    last: state.last === id ? null : state.last,
  };
}

/** The drawer's own close button (and its scrim). The rail is untouched — the
 *  cards are not what you dismissed. */
export function closeAux(state: AuxState): AuxState {
  return { ...state, active: null };
}

/** What the toolbar switch should reopen, or `null` when the drawer has never
 *  shown anything the session still offers (the caller picks a default). */
export function reopenAuxId(state: AuxState): string | null {
  return state.last;
}

/**
 * Drop drawer content whose subject no longer exists.
 *
 * 后台任务 empties as soon as the session takes another turn, and switching
 * sessions can strand the drawer on a facet the new one doesn't offer. The
 * remembered `last` is pruned on the same terms, or the toolbar switch would
 * reopen onto nothing.
 */
export function pruneTab(state: AuxState, exists: (id: string) => boolean): AuxState {
  const active = state.active != null && !exists(state.active) ? null : state.active;
  const last = state.last != null && !exists(state.last) ? null : state.last;
  if (active === state.active && last === state.last) return state;
  return { ...state, active, last };
}
