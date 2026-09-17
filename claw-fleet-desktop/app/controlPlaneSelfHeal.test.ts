import { readFileSync } from "node:fs";
import { join } from "node:path";

import { describe, expect, it, vi } from "vitest";

import {
  CODEX_RECONCILE_COMMAND,
  runControlPlaneSelfHeal,
  startupSelfHealCommands,
  type ControlPlaneInstallState,
} from "./controlPlaneSelfHeal";

const NOTHING_INSTALLED: ControlPlaneInstallState = {
  guardInstalled: false,
  elicitationInstalled: false,
  planApprovalInstalled: false,
};

const ALL_INSTALLED: ControlPlaneInstallState = {
  guardInstalled: true,
  elicitationInstalled: true,
  planApprovalInstalled: true,
};

/** Nothing stored → every feature follows its default-ON. */
const allDefault = () => true;

describe("startupSelfHealCommands", () => {
  it("new guidance with default-ON that isn't on disk yet gets installed — this is the slot where session-title guidance was missing", () => {
    expect(startupSelfHealCommands(NOTHING_INSTALLED, allDefault)).toContain(
      "apply_session_title_guidance",
    );
  });

  it("items the user turned off in settings won't be re-installed by startup self-heal", () => {
    const resolve = (key: string) => key !== "session-title-guidance-enabled";
    expect(startupSelfHealCommands(NOTHING_INSTALLED, resolve)).not.toContain(
      "apply_session_title_guidance",
    );
  });

  it("already-installed hooks don't get re-applied (they modify settings.json, which has nothing to refresh)", () => {
    const cmds = startupSelfHealCommands(ALL_INSTALLED, allDefault);
    expect(cmds).not.toContain("apply_guard_hook");
    expect(cmds).not.toContain("apply_elicitation_hook");
    expect(cmds).not.toContain("apply_plan_approval_hook");
  });

  it("guidance always applies unconditionally — apply is idempotent and also handles post-upgrade refresh of phrasing/language", () => {
    const cmds = startupSelfHealCommands(ALL_INSTALLED, allDefault);
    expect(cmds).toEqual([
      "apply_interaction_mode",
      "apply_prd_mode",
      "apply_wiki_guidance",
      "apply_model_guidance",
      "apply_session_title_guidance",
      CODEX_RECONCILE_COMMAND,
    ]);
  });

  it("codex reconcile always runs last — it reads the sentinel that the Claude side just wrote", () => {
    const cmds = startupSelfHealCommands(NOTHING_INSTALLED, allDefault);
    expect(cmds[cmds.length - 1]).toBe(CODEX_RECONCILE_COMMAND);
  });
});

describe("runControlPlaneSelfHeal", () => {
  const flush = async () => {
    for (let i = 0; i < 50; i++) await Promise.resolve();
  };

  it("one failure doesn't drag down the rest — each command has its own catch", async () => {
    const seen: string[] = [];
    const invoke = vi.fn((command: string) => {
      seen.push(command);
      return command === "apply_wiki_guidance"
        ? Promise.reject(new Error("boom"))
        : Promise.resolve(null);
    });
    const errors = vi.spyOn(console, "error").mockImplementation(() => {});

    const planned = runControlPlaneSelfHeal(invoke, NOTHING_INSTALLED, allDefault);
    await flush();

    expect(seen).toEqual(planned);
    expect(seen).toContain("apply_session_title_guidance");
    expect(errors).toHaveBeenCalledOnce();
    errors.mockRestore();
  });

  /**
   * Concurrent triggering is what happened on 2026-09-07 when CLAUDE.md got hammered
   * into a single block: six commands each read-modify-write the same file. The core
   * side already has a lock, so we serialize from the source here too.
   */
  it("executes serially — next command only fires after the previous one settles", async () => {
    const inflight: string[] = [];
    let maxConcurrent = 0;
    const resolvers: Array<() => void> = [];
    const invoke = vi.fn((command: string) => {
      inflight.push(command);
      maxConcurrent = Math.max(maxConcurrent, inflight.length);
      return new Promise<null>((resolve) => {
        resolvers.push(() => {
          inflight.splice(inflight.indexOf(command), 1);
          resolve(null);
        });
      });
    });

    const planned = runControlPlaneSelfHeal(invoke, NOTHING_INSTALLED, allDefault);
    // Only one in flight when none have settled yet.
    await flush();
    expect(invoke).toHaveBeenCalledOnce();

    while (resolvers.length) {
      resolvers.shift()!();
      await flush();
    }
    expect(invoke).toHaveBeenCalledTimes(planned.length);
    expect(maxConcurrent).toBe(1);
  });

  it("by the time codex reconcile finishes, earlier applies have truly persisted (not just sent)", async () => {
    const settled: string[] = [];
    const invoke = vi.fn(async (command: string) => {
      await Promise.resolve();
      settled.push(command);
      return null;
    });

    runControlPlaneSelfHeal(invoke, NOTHING_INSTALLED, allDefault);
    await flush();

    expect(settled[settled.length - 1]).toBe(CODEX_RECONCILE_COMMAND);
    expect(settled).toContain("apply_session_title_guidance");
  });
});

/**
 * The bug was never in the plan — it was that the only caller lived inside a
 * panel that mounts on demand. Guard the wiring itself: the app shell must run
 * the self-heal, and `SettingsPanel` must not keep a second hand-written copy
 * of the apply list that can drift from this module.
 */
describe("startup path guard", () => {
  const read = (rel: string) => readFileSync(join(__dirname, rel), "utf8");

  it("App shell runs self-heal on startup, doesn't wait for settings panel to mount", () => {
    const app = read("App.tsx");
    expect(app).toMatch(/runControlPlaneSelfHeal/);
  });

  it("SettingsPanel mount self-heal reuses the same list, doesn't copy it again", () => {
    // Individual toggle apply_* calls should stay (the path when user flips a switch).
    // What we guard is that the "default-on gets installed" list at mount-time has
    // no second hand-copied version.
    const panel = read("components/SettingsPanel.tsx");
    expect(panel).toMatch(/runControlPlaneSelfHeal/);
    expect(panel).not.toMatch(/auto-apply session title guidance/);
  });
});
