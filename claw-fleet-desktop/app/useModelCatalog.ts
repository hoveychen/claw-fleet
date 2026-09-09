// Fleet's model catalog (`claw-fleet-core/models.toml`) for the desktop pickers.
//
// The mobile counterpart is `mobile-web/src/useModelCatalog.ts`; both call the
// same core function, one through the Tauri command and one through the relay.
import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import type { PickerHarness } from "./generated/types";

// Module-level cache. The catalog is compiled into the binary and parsed once
// per process, so it cannot change while the app runs — refetching it on every
// picker mount would be pure IPC noise. A single in-flight promise is shared so
// two pickers mounting together make one call.
let cached: PickerHarness[] | null = null;
let inFlight: Promise<PickerHarness[]> | null = null;

function load(): Promise<PickerHarness[]> {
  if (cached) return Promise.resolve(cached);
  if (!inFlight) {
    inFlight = invoke<PickerHarness[]>("model_catalog")
      .then((c) => {
        cached = c ?? [];
        return cached;
      })
      .catch(() => {
        // Leave `cached` null so a later mount retries. Unlike the dsh
        // catalogue there is no host dependency that could legitimately fail
        // here — this path means IPC itself is down — so retrying is right and
        // caching the failure would not be.
        inFlight = null;
        return [];
      });
  }
  return inFlight;
}

/** The catalog, `[]` until it arrives. Callers treat `[]` as "not loaded yet"
 *  and fall back to showing only their own "default" entry. */
export function useModelCatalog(): PickerHarness[] {
  const [catalog, setCatalog] = useState<PickerHarness[]>(cached ?? []);
  useEffect(() => {
    if (cached) return;
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
