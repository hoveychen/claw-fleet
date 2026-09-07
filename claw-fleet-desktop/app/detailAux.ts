/**
 * State for the session detail's two auxiliary surfaces.
 *
 * They are two *layers* of information, and the whole point of this module is
 * that they no longer share one control:
 *
 * 1. **The rail** — a column of cards floating over the right of the
 *    conversation: the subagents running right now, and every file / wiki doc /
 *    page the agent named and the reader opened. This is "what is going on and
 *    what am I looking at" — ambient, plural, glanceable, and gone entirely
 *    (zero width) when there is nothing in it. Its content is *derived*: live
 *    subagents come from the sessions store, docs from `docs` below. One doc
 *    card at a time can be *expanded* in place into a full reader (`expanded`),
 *    which is how a file / wiki doc / web page is read.
 *
 * 2. **The drawer** — an overlay panel that floats over the transcript and
 *    shows exactly one *session facet* (Skills, 决策, Token, 任务, 后台任务,
 *    临时文件, Workflow) picked from the header menu. This is "go look
 *    something up" — singular, deliberate, dismissed when you are done.
 *    `active` is that one thing.
 *
 * Docs used to land in the drawer too, and that is the bug this split closes:
 * clicking a link in the transcript put the *same* name on screen twice (the
 * drawer's title and the rail card that had just registered it) and threw a
 * 560px panel over a conversation that, in a narrow pane, was left with a
 * hundred-pixel sliver. A doc belongs to the ambient layer it was registered
 * in; the drawer is for the lookup surfaces that have no card.
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
  /** Identity *and* the value stored in `expanded`. Prefixed by kind, so it can
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
  /** The facet the drawer is showing. `null` means the drawer is closed —
   *  which says nothing about the rail. */
  active: AuxFacet | null;
  /** The doc card currently expanded into a reader, by id. `null` means every
   *  card is collapsed to its one-line chip. */
  expanded: string | null;
}

export const initialAux: AuxState = { docs: [], active: null, expanded: null };

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
 * Click a rail doc card: expand it into a reader, or — clicking the one
 * already expanded — collapse it back to a chip. A toggle, so the card's own
 * header doubles as the off switch for the reader it opened.
 */
export function toggleDoc(state: AuxState, id: string): AuxState {
  if (state.expanded === id) return collapseDoc(state);
  if (!state.docs.some((d) => d.id === id)) return state;
  return { ...state, expanded: id };
}

/** Collapse whichever card is expanded. The card itself stays in the rail —
 *  collapsing is not dismissing. */
export function collapseDoc(state: AuxState): AuxState {
  if (state.expanded == null) return state;
  return { ...state, expanded: null };
}

/** Open a facet in the drawer. Facets have no card, so this is the only way in
 *  and it never toggles off: a menu item that sometimes closed the panel you
 *  just asked for would read as the click having missed. */
export function showFacet(state: AuxState, facet: AuxFacet): AuxState {
  if (state.active === facet) return state;
  return { ...state, active: facet };
}

/** Open a doc: card it if new (never a second copy), then expand that card.
 *  Both halves move, because naming a file in the transcript is both "this is
 *  now part of my context" and "show it to me". The drawer is not involved. */
export function openDoc(state: AuxState, kind: AuxDocKind, ref: string): AuxState {
  const doc = makeAuxDoc(kind, ref);
  const known = state.docs.some((d) => d.id === doc.id);
  let docs = known ? state.docs : [...state.docs, doc];
  // The doc we are about to open is last, so trimming from the front can never
  // drop it.
  if (docs.length > MAX_AUX_DOCS) docs = docs.slice(docs.length - MAX_AUX_DOCS);
  return { ...state, docs, expanded: doc.id };
}

/**
 * Dismiss one doc card.
 *
 * If it was the expanded one, nothing takes its place: silently expanding a
 * neighbouring file would be the rail deciding what you read next.
 */
export function closeDoc(state: AuxState, id: string): AuxState {
  const idx = state.docs.findIndex((d) => d.id === id);
  if (idx < 0) return state;
  const docs = state.docs.filter((d) => d.id !== id);
  return { ...state, docs, expanded: state.expanded === id ? null : state.expanded };
}

/** The drawer's own close button (and its scrim). The rail is untouched — the
 *  cards are not what you dismissed. */
export function closeAux(state: AuxState): AuxState {
  if (state.active == null) return state;
  return { ...state, active: null };
}

/**
 * Drop drawer content whose subject no longer exists.
 *
 * 后台任务 empties as soon as the session takes another turn, and switching
 * sessions can strand the drawer on a facet the new one doesn't offer.
 */
export function pruneTab(state: AuxState, exists: (id: string) => boolean): AuxState {
  const active = state.active != null && !exists(state.active) ? null : state.active;
  if (active === state.active) return state;
  return { ...state, active };
}
