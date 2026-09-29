// Fleet's model catalog (`claw-fleet-core/models.toml`) for the desktop pickers.
//
// The mobile counterpart is `mobile-web/src/useModelCatalog.ts`; both call the
// same core function, one through the Tauri command and one through the relay.
import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import type { PickerHarness } from "./generated/types";

// Module-level cache, served immediately on mount and then revalidated. The
// model rows are compiled in and never change while the app runs, but the
// installed CLI version they are judged against does: after the user runs
// `claude update` the "needs a newer CLI" marks must go away without an app
// restart. Core caches its `--version` probe for a minute, so the refetch is
// cheap. A single in-flight promise is shared so two pickers mounting together
// make one call.
let cached: PickerHarness[] | null = null;
let inFlight: Promise<PickerHarness[]> | null = null;

function load(): Promise<PickerHarness[]> {
  if (!inFlight) {
    inFlight = invoke<PickerHarness[]>("model_catalog")
      .then((c) => {
        cached = c ?? [];
        inFlight = null;
        return cached;
      })
      .catch(() => {
        // Leave `cached` null so a later mount retries. Unlike the dsh
        // catalogue there is no host dependency that could legitimately fail
        // here — this path means IPC itself is down — so retrying is right and
        // caching the failure would not be.
        inFlight = null;
        return cached ?? [];
      });
  }
  return inFlight;
}

/** The catalog, `[]` until it arrives. Callers treat `[]` as "not loaded yet"
 *  and fall back to showing only their own "default" entry. */
export function useModelCatalog(): PickerHarness[] {
  const [catalog, setCatalog] = useState<PickerHarness[]>(cached ?? []);
  useEffect(() => {
    let live = true;
    load().then((c) => {
      if (live) setCatalog(c);
    });
    return () => {
      live = false;
    };
  }, []);
  return catalog;
}

/** Test seam: drop the cache so a test can serve a different catalog. */
export function __resetModelCatalogCache(): void {
  cached = null;
  inFlight = null;
}
