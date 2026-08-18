/**
 * How long a transcript fetch may run before the UI stops pretending it is
 * merely "loading" and says so.
 *
 * Sized above the slowest healthy fetch we have measured (a ~50 MB Claude
 * transcript opens in single-digit seconds; a dsh `session.history` answers in
 * tens of ms) so it never fires on a session that is simply large.
 */
export const TAIL_LOAD_DEADLINE_MS = 20_000;

/**
 * Watch `p` for taking too long, WITHOUT cancelling it.
 *
 * The fetch is a Tauri command: there is no abort, and the result is still
 * worth rendering whenever it lands. So the deadline only fires a callback —
 * the caller flips its own "stalled" state and keeps awaiting.
 */
export function withStallWatch<T>(
  p: Promise<T>,
  onStall: () => void,
  ms: number = TAIL_LOAD_DEADLINE_MS,
): Promise<T> {
  const timer = setTimeout(onStall, ms);
  const disarm = () => clearTimeout(timer);
  p.then(disarm, disarm);
  return p;
}
