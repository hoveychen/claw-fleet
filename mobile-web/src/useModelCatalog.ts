// Fleet's own model directory (`claw-fleet-core/models.toml`), used by Composer's
// model / effort dropdowns. The desktop equivalent is
// `claw-fleet-desktop/app/useModelCatalog.ts`: both query the same core function,
// one via Tauri command, one via relay.
//
// This replaces two hardcoded lists that once lived here in Composer.tsx. Those
// lists fell out of sync: they claimed Codex effort ladder was
// `minimal/low/medium/high`, but testing shows no Codex model accepts `minimal`,
// and all of them accept `xhigh`/`max`.
import { useEffect, useState } from "react";
import type { FleetTransport } from "./transport";
import type { PickerHarness } from "./generated/types";

/** Returns empty array when unavailable (relay not connected, request in flight,
 *  or desktop version too old to recognize this method). Callers treat empty as
 *  "not loaded yet" and show only their "default" entry — same graceful fallback
 *  as useCodexProfiles. */
export function useModelCatalog(client: FleetTransport | null): PickerHarness[] {
  const [catalog, setCatalog] = useState<PickerHarness[]>([]);
  useEffect(() => {
    if (!client) return;
    let alive = true;
    client
      .request<PickerHarness[]>("model_catalog")
      .then((r) => {
        if (alive) setCatalog(r ?? []);
      })
      .catch(() => {
        if (alive) setCatalog([]);
      });
    return () => {
      alive = false;
    };
  }, [client]);
  return catalog;
}

/** Available models for a harness → dropdown entries `[value, label]`, prefixed
 *  with the "default" entry. When the catalog hasn't arrived, only "default"
 *  shows — an honest fallback: session runs on whatever the CLI configured. */
export function modelChoicesFor(
  catalog: PickerHarness[],
  harness: string,
  defaultLabel: string,
): Array<[string, string]> {
  const models = catalog.find((h) => h.name === harness)?.models ?? [];
  return [["", defaultLabel], ...models.map((m): [string, string] => [m.id, m.label])];
}

/** Why a model is too new for the installed CLI, or `null` when it is not (or
 *  either version is unknown — core only sets `needsCliUpgrade` when both are
 *  known). `command` is the channel-appropriate upgrade, when there is one. */
export function cliFloorFor(
  catalog: PickerHarness[],
  harness: string,
  model: string,
): { min: string; current: string; command: string | null } | null {
  const h = catalog.find((x) => x.name === harness);
  const m = h?.models.find((x) => x.id === model);
  if (!h?.cliVersion || !m?.needsCliUpgrade || !m.minCliVersion) return null;
  return { min: m.minCliVersion, current: h.cliVersion, command: h.upgradeCommand ?? null };
}

/** Effort ladder for a model; when no model is selected, the union of all efforts
 *  in that harness.
 *
 *  Per-model rather than per-harness because ladders truly differ within a harness:
 *  `gpt-5.5` stops at `xhigh`, but its siblings can reach `max` and `ultra`. Old
 *  code hard-coded this as "special case gpt-6-astra", so everything else was
 *  wrong. */
export function effortChoicesFor(
  catalog: PickerHarness[],
  harness: string,
  model: string,
  defaultLabel: string,
): Array<[string, string]> {
  const h = catalog.find((x) => x.name === harness);
  const picked = h?.models.find((m) => m.id === model);
  let levels: string[];
  if (picked) {
    levels = picked.efforts;
  } else {
    levels = [];
    for (const m of h?.models ?? []) {
      for (const e of m.efforts) if (!levels.includes(e)) levels.push(e);
    }
  }
  return [["", defaultLabel], ...levels.map((e): [string, string] => [e, e])];
}
