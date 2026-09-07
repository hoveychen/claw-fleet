/**
 * State for the session detail's auxiliary column — the tabbed right-hand panel
 * that replaced the old row of view tabs above the conversation.
 *
 * The conversation is no longer *one of* the things you switch between: it owns
 * the left column permanently. Everything else — the live subagents, Skills,
 * 决策, Token, 任务, 后台任务, 临时文件, Workflow, and any file / wiki doc / page
 * the agent named — is a tab in the panel *beside* it. One `active` id spans all
 * three families, which is what makes them one tab strip rather than three
 * stacked sections competing for the same height.
 *
 * Pure module, no React: the reducer is the part worth testing, and the panel
 * should not have to be mounted to test it.
 */

/** A session facet — a panel scoped to this session. */
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

/** The live-subagent deck's tab id. Not a facet: it exists only while something
 *  is running, and it is the tab the panel opens itself on. */
export const AGENTS_TAB = "agents";

export function isAuxFacet(value: unknown): value is AuxFacet {
  return FACETS.includes(value as AuxFacet);
}

/** A document the agent named and the reader opened: a repo file, a wiki doc,
 *  or a web page. */
export type AuxDocKind = "file" | "wiki" | "web";

export interface AuxDoc {
  /** Identity *and* the value stored in `active`. Prefixed by kind, so it can
   *  never collide with a facet name or with `AGENTS_TAB`. */
  id: string;
  kind: AuxDocKind;
  /** Absolute path / wiki slug / url, by kind. */
  ref: string;
  /** What the tab shows. */
  label: string;
}

export interface AuxState {
  /** Docs opened in this pane, most recently opened last. */
  docs: AuxDoc[];
  /** The selected tab: `AGENTS_TAB`, a facet name, or a doc id. `null` means
   *  the reader has picked nothing — the panel then shows only when subagents
   *  are running (see `activeAuxTab`). */
  active: string | null;
  /** The reader closed the panel while it held nothing but the agent deck.
   *  Kept so it stays closed as those cards churn, and reset once the last live
   *  subagent finishes (`syncLiveAgents`). */
  agentsDismissed: boolean;
}

export const initialAux: AuxState = { docs: [], active: null, agentsDismissed: false };

/** Cap on remembered docs. A long session can name dozens of files; the strip
 *  is a "what I have been reading" list, not a history. Oldest drops first,
 *  never the active one. */
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
 * Click a tab: show it, or — clicking the one already showing — close the
 * panel. A toggle, because the strip is also the panel's only on/off control.
 */
export function toggleTab(state: AuxState, id: string): AuxState {
  if (state.active === id) return closeAux(state);
  return { ...state, active: id, agentsDismissed: false };
}

/** Open a tab without the toggle-off half — for the toolbar's "show the panel"
 *  button and for jumps from elsewhere (a clicked plan row). */
export function showTab(state: AuxState, id: string): AuxState {
  if (state.active === id && !state.agentsDismissed) return state;
  return { ...state, active: id, agentsDismissed: false };
}

/** Open a doc: reveal it if already open (no second copy), otherwise append and
 *  focus it. */
export function openDoc(state: AuxState, kind: AuxDocKind, ref: string): AuxState {
  const doc = makeAuxDoc(kind, ref);
  const known = state.docs.some((d) => d.id === doc.id);
  let docs = known ? state.docs : [...state.docs, doc];
  // The doc we are about to focus is last, so trimming from the front can
  // never drop it.
  if (docs.length > MAX_AUX_DOCS) docs = docs.slice(docs.length - MAX_AUX_DOCS);
  return { ...state, docs, active: doc.id, agentsDismissed: false };
}

/** Close one doc. If it was the one on screen, fall back to its neighbour so
 *  the panel doesn't blink shut mid-read; with no docs left it falls back to
 *  no selection, which the tab strip resolves. */
export function closeDoc(state: AuxState, id: string): AuxState {
  const idx = state.docs.findIndex((d) => d.id === id);
  if (idx < 0) return state;
  const docs = state.docs.filter((d) => d.id !== id);
  if (state.active !== id) return { ...state, docs };
  const fallback = docs[idx] ?? docs[idx - 1] ?? null;
  return { ...state, docs, active: fallback ? fallback.id : null };
}

/** The panel's own close button. Also marks the agent deck dismissed, so
 *  closing a panel that holds nothing else actually closes it. */
export function closeAux(state: AuxState): AuxState {
  return { ...state, active: null, agentsDismissed: true };
}

/**
 * Which tab is on screen, or `null` when the panel is closed.
 *
 * Two ways to be open: the reader picked a tab, or subagents are running and
 * the deck has not been dismissed — the second is what makes "一个页看完整个任
 * 务的所有 agent 状态" true without asking for a click.
 */
export function activeAuxTab(state: AuxState, liveAgentCount: number): string | null {
  if (state.active != null) return state.active;
  return liveAgentCount > 0 && !state.agentsDismissed ? AGENTS_TAB : null;
}

/** Called as the live-subagent count changes. Once the last one finishes the
 *  dismissal is spent — the next fan-out earns a fresh auto-open. */
export function syncLiveAgents(state: AuxState, liveAgentCount: number): AuxState {
  if (liveAgentCount === 0 && state.agentsDismissed) {
    return { ...state, agentsDismissed: false };
  }
  return state;
}

/**
 * Drop a selection whose tab no longer exists.
 *
 * 后台任务 empties as soon as the session takes another turn, the agent deck
 * disappears when the last subagent finishes, and switching sessions can strand
 * the panel on a facet the new one doesn't offer.
 */
export function pruneTab(state: AuxState, exists: (id: string) => boolean): AuxState {
  if (state.active == null) return state;
  return exists(state.active) ? state : { ...state, active: null };
}
