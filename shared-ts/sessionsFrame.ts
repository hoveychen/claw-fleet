// Applying the session-list push frames that `claw-fleet-core/src/session_delta.rs`
// produces. Shared by the desktop store and mobile-web's `fleet serve` transport,
// which receive the same frames over different channels (a Tauri event, SSE).
//
// A frame is either the whole list (`full`) or the rows that changed since the
// frame numbered `baseSeq` (`delta`). A delta only applies to exactly that
// state: a consumer holding any other seq — it missed a frame, or its list came
// from somewhere that carries no seq — must refetch a full frame instead. That
// is what `applySessionsFrame` returning `null` means.
//
// Unchanged rows keep their object identity, so memoised rows don't re-render.

export interface SessionsFullFrame<T> {
  kind: "full";
  seq: number;
  sessions: T[];
}

export interface SessionsDeltaFrame<T> {
  kind: "delta";
  seq: number;
  baseSeq: number;
  upsert: T[];
  remove: string[];
  /** The full id order; present only when it changed. */
  order?: string[];
}

export type SessionsFrame<T> = SessionsFullFrame<T> | SessionsDeltaFrame<T>;

/** The list a consumer holds, and the seq of the frame that produced it.
 *  `seq` is null when the list came from somewhere unnumbered. */
export interface SessionsState<T> {
  seq: number | null;
  sessions: T[];
}

/** Apply `frame` to `state`. Returns the new state, or `null` when the frame is
 *  a delta that does not follow `state` — the caller must resync from a full
 *  frame. A frame older than or equal to `state` is ignored (returns `state`). */
export function applySessionsFrame<T extends { id: string }>(
  state: SessionsState<T>,
  frame: SessionsFrame<T>,
): SessionsState<T> | null {
  if (frame.kind === "full") {
    if (state.seq !== null && frame.seq < state.seq) return state;
    return { seq: frame.seq, sessions: frame.sessions };
  }
  if (state.seq !== null && frame.seq <= state.seq) return state;
  if (state.seq !== frame.baseSeq) return null;

  const byId = new Map<string, T>();
  for (const s of state.sessions) byId.set(s.id, s);
  for (const id of frame.remove) byId.delete(id);
  const added: string[] = [];
  for (const s of frame.upsert) {
    if (!byId.has(s.id)) added.push(s.id);
    byId.set(s.id, s);
  }

  const order =
    frame.order ??
    [...state.sessions.map((s) => s.id).filter((id) => byId.has(id)), ...added];
  const sessions: T[] = [];
  for (const id of order) {
    const s = byId.get(id);
    if (s) sessions.push(s);
  }
  return { seq: frame.seq, sessions };
}
