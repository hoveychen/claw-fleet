import { getFeatureState, resolveFeatureState, setFeatureState, type FeatureState } from "./storage";

/**
 * The feature switches `get_hooks_setup_plan` reports. Each field is read from
 * `~/.fleet/control-plane-prefs.json` — what the sessions Fleet starts actually
 * get — so it, not the tristate this panel keeps in localStorage, is the truth.
 */
export interface FeatureSwitches {
  hooksGloballyDisabled: boolean;
  guardInstalled: boolean;
  elicitationInstalled: boolean;
  planApprovalInstalled: boolean;
  interactionModeInstalled: boolean;
  prdDisciplineInstalled: boolean;
  wikiGuidanceInstalled: boolean;
  modelGuidanceInstalled: boolean;
  sessionTitleGuidanceInstalled: boolean;
}

/** The localStorage key each settings toggle is stored under, and the switch it drives. */
export const FEATURE_SWITCH_FIELDS = {
  "guard-enabled": "guardInstalled",
  "elicitation-enabled": "elicitationInstalled",
  "plan-approval-enabled": "planApprovalInstalled",
  "interaction-mode-enabled": "interactionModeInstalled",
  "prd-mode-enabled": "prdDisciplineInstalled",
  "wiki-guidance-enabled": "wikiGuidanceInstalled",
  "model-guidance-enabled": "modelGuidanceInstalled",
  "session-title-guidance-enabled": "sessionTitleGuidanceInstalled",
} as const satisfies Record<string, keyof FeatureSwitches>;

export type FeatureSwitchKey = keyof typeof FEATURE_SWITCH_FIELDS;

/**
 * Bring each toggle's stored tristate in line with the backend switch, and
 * return every toggle's state afterwards.
 *
 * A stored state that already resolves to the backend value is kept as is, so
 * "follow the default" survives. One that disagrees — the scope migration
 * recorded a feature as off while localStorage still says "default", or the
 * switch was flipped from another client — becomes an explicit "on"/"off".
 */
export function syncFeatureStates(
  switches: FeatureSwitches,
): Record<FeatureSwitchKey, FeatureState> {
  const out = {} as Record<FeatureSwitchKey, FeatureState>;
  for (const [key, field] of Object.entries(FEATURE_SWITCH_FIELDS) as [
    FeatureSwitchKey,
    keyof FeatureSwitches,
  ][]) {
    const stored = getFeatureState(key);
    const on = switches[field];
    if (resolveFeatureState(stored, key) === on) {
      out[key] = stored;
    } else {
      const next: FeatureState = on ? "on" : "off";
      setFeatureState(key, next);
      out[key] = next;
    }
  }
  return out;
}
