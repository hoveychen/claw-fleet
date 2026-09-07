import { useEffect, useState, type Dispatch, type SetStateAction } from "react";

import { initialAux, type AuxState } from "./detailAux";

/**
 * The session detail's auxiliary state, scoped to one session.
 *
 * The scoping is the whole point of the hook. `SessionDetail` is mounted once
 * and re-pointed at another session rather than remounted (no `key` at any of
 * its three call sites), so nothing in it resets on a session switch unless it
 * says so — and the doc cards, which are "files *I* clicked open in *this*
 * conversation", read as a lie the moment they outlive the conversation that
 * named them: a `main.rs` card sitting beside a session that never mentioned
 * the file.
 *
 * Keyed on the session id, so switching hands the next session a clean rail and
 * a closed drawer.
 */
export function useSessionAux(
  sessionId: string | null | undefined,
): [AuxState, Dispatch<SetStateAction<AuxState>>] {
  const [aux, setAux] = useState<AuxState>(initialAux);
  useEffect(() => {
    setAux(initialAux);
  }, [sessionId]);
  return [aux, setAux];
}
