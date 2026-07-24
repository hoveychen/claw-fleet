import { beforeEach, describe, expect, it } from "vitest";
import {
  getItem,
  setItem,
  removeItem,
  resolveFeature,
  getFeatureState,
  setFeatureState,
  resolveFeatureState,
  featureDefault,
  migrateFeatureTristate,
  FEATURE_DEFAULTS,
  MODE_DEFAULTS,
  modeDefault,
  getModeSelection,
  setModeSelection,
  resolveMode,
} from "./storage";

// Keys these tests write; reset before each so the module-global cache doesn't
// leak state between cases (initStorage is never called, so the Tauri store is
// null and every read/write goes through the in-memory cache — pure JS).
const TOUCHED = [
  "guard-enabled",
  "tts-muted",
  "mascot-visible",
  "interaction-mode-enabled",
  "prd-mode-enabled",
  "plan-approval-enabled",
  "wiki-guidance-enabled",
  "model-guidance-enabled",
  "tts-mode",
  "feature-tristate-migrated",
  "notification-mode",
  "chime-sound",
];

function resetAll() {
  for (const k of TOUCHED) removeItem(k);
}

describe("tristate feature toggles", () => {
  beforeEach(resetAll);

  it("absent key follows FEATURE_DEFAULTS", () => {
    // guard defaults ON, tts-muted defaults OFF
    expect(getFeatureState("guard-enabled")).toBe("default");
    expect(resolveFeature("guard-enabled")).toBe(true);
    expect(resolveFeature("tts-muted")).toBe(false);
  });

  it("explicit on/off overrides the default and round-trips", () => {
    setFeatureState("guard-enabled", "off");
    expect(getFeatureState("guard-enabled")).toBe("off");
    expect(getItem("guard-enabled")).toBe("off");
    expect(resolveFeature("guard-enabled")).toBe(false);

    setFeatureState("tts-muted", "on");
    expect(resolveFeature("tts-muted")).toBe(true);
  });

  it("'default' selection clears the stored value (persistence of tristate)", () => {
    setFeatureState("guard-enabled", "off");
    expect(getItem("guard-enabled")).toBe("off");
    setFeatureState("guard-enabled", "default");
    expect(getItem("guard-enabled")).toBeNull();
    expect(getFeatureState("guard-enabled")).toBe("default");
  });

  it("legacy 'true'/'false' values are still understood", () => {
    setItem("guard-enabled", "false");
    expect(getFeatureState("guard-enabled")).toBe("off");
    expect(resolveFeature("guard-enabled")).toBe(false);
    setItem("tts-muted", "true");
    expect(resolveFeature("tts-muted")).toBe(true);
  });

  it("resolveFeatureState mirrors resolveFeature without a storage read", () => {
    expect(resolveFeatureState("default", "guard-enabled")).toBe(featureDefault("guard-enabled"));
    expect(resolveFeatureState("on", "tts-muted")).toBe(true);
    expect(resolveFeatureState("off", "guard-enabled")).toBe(false);
  });

  it("changing the central default propagates to default-state users (zero migration)", () => {
    setFeatureState("mascot-visible", "default"); // stays absent
    expect(resolveFeature("mascot-visible")).toBe(false);
    const orig = FEATURE_DEFAULTS["mascot-visible"];
    try {
      FEATURE_DEFAULTS["mascot-visible"] = true;
      // No per-user migration needed — the default-state user follows the table.
      expect(resolveFeature("mascot-visible")).toBe(true);
    } finally {
      FEATURE_DEFAULTS["mascot-visible"] = orig;
    }
  });
});

describe("migrateFeatureTristate (one-shot)", () => {
  beforeEach(resetAll);

  it("clears legacy binary values on the changed-default keys exactly once", () => {
    setItem("interaction-mode-enabled", "false");
    setItem("tts-mode", "off");

    migrateFeatureTristate();
    expect(getItem("interaction-mode-enabled")).toBeNull(); // reset to default
    expect(getItem("tts-mode")).toBeNull();
    expect(getItem("feature-tristate-migrated")).toBe("true");

    // A later deliberate choice must survive a second run (flag-guarded).
    setFeatureState("interaction-mode-enabled", "off");
    migrateFeatureTristate();
    expect(getItem("interaction-mode-enabled")).toBe("off");
  });
});

describe("tristate mode selectors", () => {
  beforeEach(resetAll);

  it("absent key follows MODE_DEFAULTS", () => {
    expect(getModeSelection("notification-mode")).toBe("default");
    expect(resolveMode("notification-mode")).toBe(MODE_DEFAULTS["notification-mode"]);
    expect(resolveMode("tts-mode")).toBe("chime_and_speech");
  });

  it("a concrete selection round-trips and resolves to itself", () => {
    setModeSelection("chime-sound", "ding_dong");
    expect(getModeSelection("chime-sound")).toBe("ding_dong");
    expect(resolveMode("chime-sound")).toBe("ding_dong");
  });

  it("'default' clears the key so the mode follows the central table", () => {
    setModeSelection("notification-mode", "all");
    expect(resolveMode("notification-mode")).toBe("all");
    setModeSelection("notification-mode", "default");
    expect(getItem("notification-mode")).toBeNull();
    expect(resolveMode("notification-mode")).toBe(modeDefault("notification-mode"));
  });

  it("changing a mode default propagates to default-state users (zero migration)", () => {
    setModeSelection("chime-sound", "default"); // absent
    const orig = MODE_DEFAULTS["chime-sound"];
    try {
      MODE_DEFAULTS["chime-sound"] = "chime_soft";
      expect(resolveMode("chime-sound")).toBe("chime_soft");
    } finally {
      MODE_DEFAULTS["chime-sound"] = orig;
    }
  });
});
