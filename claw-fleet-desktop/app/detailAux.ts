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
 *    临时文件, 笔记, Workflow) picked from the header menu. This is "go look
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
  | "notes"
  | "workflow";

const FACETS: readonly AuxFacet[] = [
  "skills",
  "decisions",
  "tokens",
  "tasks",
  "bgtasks",
  "scratchpad",
  "notes",
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
 *  a web page, or a deliverable the run filed into the 产出 store. */
export type AuxDocKind = "file" | "wiki" | "web" | "artifact";

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
  /** The card currently expanded into a reader, by id — a doc's `kind:ref` or
   *  a subagent's `agentCardId`. `null` means every card is collapsed to its
   *  one-line chip. One expansion at a time, across both kinds: the rail only
   *  reserves one band of the conversation. */
  expanded: string | null;
  /** Session id of a subagent whose card must survive the agent finishing.
   *
   *  The agent cards are derived from the live-session set, so a subagent that
   *  ends is pulled out of the rail — which is right for a chip nobody is
   *  looking at, and wrong for the transcript you are in the middle of
   *  reading. Expanding a card pins it here; the rail keeps rendering it from
   *  the last snapshot it saw until the reader dismisses it. */
  pinnedAgent: string | null;
}

export const initialAux: AuxState = {
  docs: [],
  active: null,
  expanded: null,
  pinnedAgent: null,
};

/** Rail id for a subagent card. Prefixed like a doc's, so `expanded` can hold
 *  either and the two can never collide. */
export function agentCardId(sessionId: string): string {
  return `agent:${sessionId}`;
}

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
  // An artifact is addressed by its store id (`20260909-080326`), which names
  // nothing to a reader. Callers pass the deliverable's title instead; this is
  // only the fallback for one opened without one.
  if (kind === "artifact") return ref;
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

/**
 * The one extra value a *collapsed* chip shows beside its name.
 *
 * Derived from the ref alone, on purpose. The obvious thing to print is a size,
 * but a chip has no loaded document behind it, and reading eight files (and
 * eight deliverable blobs) so the rail can print eight sizes would be a fetch
 * per chip for a number nobody asked for. What a stack of chips actually has to
 * answer is *which one is this* — two `mod.rs` chips, two posters from the same
 * run — and the ref already says that:
 *
 * - a file → its parent directory's name
 * - a wiki doc → its slug's folder prefix (`arch/deep/x` → `arch/deep`)
 * - a web page → the last path segment
 * - a deliverable → the date its store id starts with
 *
 * The *expanded* card is where sizes, versions and timestamps belong; its
 * header has already loaded the document and prints all of them.
 */
export function auxDocMeta(kind: AuxDocKind, ref: string): string {
  switch (kind) {
    case "file": {
      const dir = ref.slice(0, Math.max(ref.lastIndexOf("/"), ref.lastIndexOf("\\")));
      return dir ? basename(dir) : "";
    }
    case "wiki": {
      const cut = ref.lastIndexOf("/");
      return cut > 0 ? ref.slice(0, cut) : "";
    }
    case "web": {
      try {
        const segments = new URL(ref).pathname.split("/").filter((s) => s.length > 0);
        return segments.length > 0 ? segments[segments.length - 1] : "";
      } catch {
        return "";
      }
    }
    case "artifact": {
      // Store ids are `YYYYMMDD-HHMMSS`; the day is what tells two runs apart.
      const m = /^(\d{4})(\d{2})(\d{2})-/.exec(ref);
      return m ? `${m[2]}-${m[3]}` : "";
    }
  }
}

export function makeAuxDoc(kind: AuxDocKind, ref: string, label?: string): AuxDoc {
  return { id: docId(kind, ref), kind, ref, label: label || auxDocLabel(kind, ref) };
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

/**
 * Click a rail subagent card: expand it in place into a transcript preview, or
 * — clicking the one already expanded — collapse it back to a chip.
 *
 * Clicking used to *navigate*: the detail view swapped to the subagent's own
 * session, so checking what a fan-out was doing cost you the conversation you
 * were reading and a trip back. A subagent has nothing to show but its
 * messages, so it does not need a page of its own — it needs the same in-place
 * reader a file gets, which is what this is. (Going there is still one click,
 * from the expanded card's header.)
 *
 * Expanding also *pins* the agent: see `pinnedAgent`.
 */
export function toggleAgent(state: AuxState, sessionId: string): AuxState {
  const id = agentCardId(sessionId);
  if (state.expanded === id) return { ...state, expanded: null };
  return { ...state, expanded: id, pinnedAgent: sessionId };
}

/**
 * Dismiss a pinned subagent card — the ✕ on a card the rail is only still
 * showing because it was read after the agent finished.
 *
 * A live agent's card has no ✕: it is derived from the live set and dismissing
 * it would last until the next scan tick. Only the pin is dismissible.
 */
export function closeAgent(state: AuxState, sessionId: string): AuxState {
  const id = agentCardId(sessionId);
  if (state.pinnedAgent !== sessionId && state.expanded !== id) return state;
  return {
    ...state,
    pinnedAgent: state.pinnedAgent === sessionId ? null : state.pinnedAgent,
    expanded: state.expanded === id ? null : state.expanded,
  };
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
export function openDoc(
  state: AuxState,
  kind: AuxDocKind,
  ref: string,
  label?: string,
): AuxState {
  const doc = makeAuxDoc(kind, ref, label);
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

/**
 * Keep one card, drop the rest.
 *
 * The rail caps at {@link MAX_AUX_DOCS}, and a session that names files freely
 * fills it: by the time you are reading one card, the seven chips above it are
 * things you finished with. Dismissing them one ✕ at a time is the friction
 * this removes. The kept card stays expanded if it was.
 */
export function closeOtherDocs(state: AuxState, id: string): AuxState {
  const keep = state.docs.find((d) => d.id === id);
  if (!keep || state.docs.length === 1) return state;
  return { ...state, docs: [keep], expanded: state.expanded === id ? id : null };
}

/** Clear the stack. Nothing is left expanded, because nothing is left. */
export function closeAllDocs(state: AuxState): AuxState {
  if (state.docs.length === 0) return state;
  return { ...state, docs: [], expanded: null };
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
