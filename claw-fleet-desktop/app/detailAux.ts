/**
 * State for the session detail's auxiliary column — the right-hand panel that
 * replaced the old row of mutually-exclusive view tabs.
 *
 * The conversation is no longer *one of* the facets you switch between: it owns
 * the left column permanently, and everything else (Skills, 决策, Token, 任务,
 * 后台任务, 临时文件, Workflow) is something you pull up *beside* it. Same for a
 * path, a `[[slug]]` or a url clicked in agent prose: it opens here rather than
 * as a tab in the window's strip, so the thing the agent named sits next to the
 * sentence that named it.
 *
 * Pure module, no React: the reducer is the part worth testing, and the panel
 * component should not have to be mounted to test it.
 */

/** A session facet — one of the buttons above the conversation. */
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
  /** What the doc strip shows. */
  label: string;
}

export interface AuxState {
  /** Docs opened in this pane, most recently opened last. */
  docs: AuxDoc[];
  /** A facet name or a doc id; `null` means nothing was picked (the panel is
   *  then only worth showing when live subagents are running — see
   *  `auxVisible`). */
  active: string | null;
  /** The reader closed the panel while it held nothing but the agent cards.
   *  Kept so it stays closed as those cards churn, and reset once the last
   *  live subagent finishes (`syncLiveAgents`). */
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

/** Click a facet button: show it, or — clicking the one already showing —
 *  close the panel again. A toggle, because the button row is the only control
 *  the facet has. */
export function toggleFacet(state: AuxState, facet: AuxFacet): AuxState {
  if (state.active === facet) return closeAux(state);
  return { ...state, active: facet, agentsDismissed: false };
}

/** Open a doc: reveal it if already open (no second copy — same rule the tab
 *  strip used), otherwise append and focus it. */
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
 *  the panel doesn't blink shut mid-read; with no docs left it closes. */
export function closeDoc(state: AuxState, id: string): AuxState {
  const idx = state.docs.findIndex((d) => d.id === id);
  if (idx < 0) return state;
  const docs = state.docs.filter((d) => d.id !== id);
  if (state.active !== id) return { ...state, docs };
  const fallback = docs[idx] ?? docs[idx - 1] ?? null;
  return { ...state, docs, active: fallback ? fallback.id : null };
}

/** The panel's own close button. Also marks the agent cards dismissed, so
 *  closing an empty-but-for-cards panel actually closes it. */
export function closeAux(state: AuxState): AuxState {
  return { ...state, active: null, agentsDismissed: true };
}

/**
 * Is the panel on screen?
 *
 * Two ways in: the reader picked something, or a subagent is running and the
 * cards have not been dismissed. The second is what makes "一个页看完整个任务的
 * 所有 agent 状态" true without asking for a click.
 */
export function auxVisible(state: AuxState, liveAgentCount: number): boolean {
  if (state.active != null) return true;
  return liveAgentCount > 0 && !state.agentsDismissed;
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
 * Drop an active facet that no longer has a button.
 *
 * 后台任务 empties as soon as the session takes another turn, and switching
 * sessions can strand the panel on a facet the new one doesn't offer — the old
 * tab row had the same guard.
 */
export function pruneFacet(state: AuxState, available: (facet: AuxFacet) => boolean): AuxState {
  if (state.active == null || !isAuxFacet(state.active)) return state;
  return available(state.active) ? state : { ...state, active: null };
}
