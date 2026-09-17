// dsh model catalog — source of model/effort dropdown data on mobile.
//
// dsh is the only agent whose model list is not curated by Fleet: it exposes
// the providers configured on the host via `llm.models` — on this machine:
// 2 DeepSeek models plus 276 openrouter models across 43 vendors. Mobile cannot
// reach that config (it lives in the host's ~/.dsh/settings.yaml), so we fetch
// it via relay's `dsh_models` method.
//
// The desktop equivalent is `dshModelMenu` in claw-fleet-desktop/app/modelChoices.ts —
// that uses a two-level popover, but we only have native <select>, so we use
// <optgroup> to express the same grouping rules (vendor division and order are
// consistent across both, so the menu won't reorder just because it's on a different platform).
import { useEffect, useState } from "react";
import type { FleetTransport } from "./transport";
import type { DshModelCatalog } from "./generated/types";

/** Featured openrouter vendors pinned to the top of the menu. Order is
 *  hand-picked by the user, not derived: the live data has no sortable signal
 *  like popularity or recency (openrouter descriptions are all null).
 *  Kept in sync with desktop's DSH_FEATURED_VENDORS. */
export const DSH_FEATURED_VENDORS: string[] = [
  "anthropic",
  "deepseek",
  "openai",
  "google",
  "moonshotai",
];

/** Groups with model count <= this threshold flatten inline without subgroups —
 *  nesting just two DeepSeek models would add one extra tap without saving space.
 *  Larger groups split by vendor. */
export const DSH_INLINE_GROUP_CAP = 20;

/** A group of dropdown items. Empty `label` means no optgroup wrapping — items flatten inline. */
export interface DshModelOptGroup {
  label: string;
  models: Array<[string, string]>;
}

/** `anthropic/claude-opus-5` → `anthropic`; returns "" for unprefixed IDs. */
function vendorOf(modelId: string): string {
  const i = modelId.indexOf("/");
  return i > 0 ? modelId.slice(0, i) : "";
}

/** Convert catalog into <select> grouped option entries.
 *
 *  Coverage is exhaustive: each model in the catalog appears in exactly one group.
 *  Omitting a model means it's unreachable in the UI even though dsh offers the spec.
 *
 *  Returns empty array instead of erroring when catalog is missing or empty —
 *  the dropdown then only has its own "default" item. This is honest: the session
 *  will run on whichever model ~/.dsh/settings.yaml selected. All fields read
 *  defensively; host's Fleet version may be older than any field here. */
export function dshModelGroups(
  catalog: DshModelCatalog | null | undefined,
): DshModelOptGroup[] {
  const out: DshModelOptGroup[] = [];
  for (const group of catalog?.groups ?? []) {
    const models = group.models ?? [];
    if (!models.length) continue;
    const entry = (m: (typeof models)[number]): [string, string] => [m.spec, m.name || m.id];
    if (models.length <= DSH_INLINE_GROUP_CAP) {
      out.push({ label: group.name || group.id, models: models.map(entry) });
      continue;
    }
    // Bucket by vendor first, then emit featured vendors in user-defined order —
    // this way the menu stays stable as the catalog grows.
    const byVendor = new Map<string, Array<[string, string]>>();
    for (const m of models) {
      const vendor = DSH_FEATURED_VENDORS.includes(vendorOf(m.id)) ? vendorOf(m.id) : "";
      const bucket = byVendor.get(vendor) ?? [];
      bucket.push(entry(m));
      byVendor.set(vendor, bucket);
    }
    const groupName = group.name || group.id;
    for (const vendor of DSH_FEATURED_VENDORS) {
      const bucket = byVendor.get(vendor);
      if (bucket?.length) out.push({ label: `${groupName} · ${vendor}`, models: bucket });
    }
    const rest = byVendor.get("");
    if (rest?.length) out.push({ label: groupName, models: rest });
  }
  return out;
}

/** Which model's effort ladder to display: the explicitly selected one if set;
 *  otherwise the one dsh will actually use — the catalog's `defaultSpec` (set via
 *  agent-default-model on the host). Previously only recognized explicit selection,
 *  so on machines that always used the default, the effort dropdown only had one
 *  option and the ladder seemed to not exist. Returns "" if neither is available;
 *  downstream treats this as "no ladder available". */
export function dshLadderSpec(
  catalog: DshModelCatalog | null | undefined,
  model: string,
): string {
  return model || catalog?.defaultSpec || "";
}

/** This model's own effort ladder and dsh's default value for it.
 *  Each model has a different ladder; using Claude's fixed tiers would emit
 *  effort values dsh doesn't recognize. */
export function dshEffortsFor(
  catalog: DshModelCatalog | null | undefined,
  spec: string,
): { efforts: Array<[string, string]>; defaultEffort: string } {
  if (!spec) return { efforts: [], defaultEffort: "" };
  for (const group of catalog?.groups ?? []) {
    for (const m of group.models ?? []) {
      if (m.spec !== spec) continue;
      return {
        efforts: (m.efforts ?? []).map((e) => [e.id, e.name || e.id]),
        defaultEffort: m.defaultEffort ?? "",
      };
    }
  }
  return { efforts: [], defaultEffort: "" };
}

/** Host's dsh model catalog. Returns null if unreachable (relay disconnected,
 *  request in flight, host has no dsh installed, or desktop version too old to
 *  recognize this method) — callers treat null as "only the default item". */
export function useDshModels(client: FleetTransport | null): DshModelCatalog | null {
  const [catalog, setCatalog] = useState<DshModelCatalog | null>(null);
  useEffect(() => {
    if (!client) return;
    let alive = true;
    client
      .request<DshModelCatalog>("dsh_models")
      .then((r) => {
        if (alive) setCatalog(r ?? null);
      })
      .catch(() => {
        if (alive) setCatalog(null);
      });
    return () => {
      alive = false;
    };
  }, [client]);
  return catalog;
}
