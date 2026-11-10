import { useEffect, useState } from "react";

/** Where an inlined preview document is: still building, built, or failed. */
export type SandboxDocState =
  | { status: "loading" }
  | { status: "ready"; doc: string }
  | { status: "error"; error: string };

const LOADING: SandboxDocState = { status: "loading" };

/**
 * Build a sandboxed frame's `srcDoc` with `build` (normally `fetchSandboxDoc`)
 * whenever `key` changes. `key === null` turns the hook off — the caller is
 * loading the frame by URL — and it reports `null`.
 *
 * The result is tagged with its key, so the render right after a switch reads
 * "loading" instead of the previous document.
 */
export function useSandboxDoc(
  key: string | null,
  build: () => Promise<string>,
): SandboxDocState | null {
  const [state, setState] = useState<{ key: string; value: SandboxDocState } | null>(null);
  useEffect(() => {
    if (key === null) return;
    let alive = true;
    build()
      .then((doc) => {
        if (alive) setState({ key, value: { status: "ready", doc } });
      })
      .catch((e) => {
        if (alive) {
          const error = e instanceof Error ? e.message : String(e);
          setState({ key, value: { status: "error", error } });
        }
      });
    return () => {
      alive = false;
    };
    // `build` is a fresh closure on every render; `key` names what it builds.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [key]);
  if (key === null) return null;
  return state?.key === key ? state.value : LOADING;
}
