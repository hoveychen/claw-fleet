import { useCallback, useEffect, useRef, useState } from "react";

/**
 * Wraps an async action for a button: `pending` is true while it runs and
 * re-entrant calls are dropped, so a double click cannot fire it twice.
 * Errors propagate to the caller after `pending` is cleared.
 */
export function usePending<A extends unknown[], R>(
  fn: (...args: A) => Promise<R>,
): [boolean, (...args: A) => Promise<R | undefined>] {
  const [pending, setPending] = useState(false);
  const inFlight = useRef(false);
  const mounted = useRef(true);
  const fnRef = useRef(fn);
  fnRef.current = fn;

  useEffect(() => {
    mounted.current = true;
    return () => {
      mounted.current = false;
    };
  }, []);

  const run = useCallback(async (...args: A) => {
    if (inFlight.current) return undefined;
    inFlight.current = true;
    setPending(true);
    try {
      return await fnRef.current(...args);
    } finally {
      inFlight.current = false;
      if (mounted.current) setPending(false);
    }
  }, []);

  return [pending, run];
}
