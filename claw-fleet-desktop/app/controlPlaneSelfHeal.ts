import { resolveFeature } from "./storage";

/**
 * The startup self-heal for Fleet's control plane: install every default-ON
 * feature the user has not switched off, whatever the disk currently says.
 *
 * This used to live inline in `SettingsPanel`'s mount effect, which made it
 * unreachable until 老板 actually opened 设置 — `App.tsx` renders the panel as
 * `settingsOpen && <SettingsPanel/>`. The other startup path (`set_locale` →
 * `reapply_all_guidance_if_installed` in `gui/notification.rs`) only *refreshes*
 * carriers that are already installed, by design. So a newly added default-ON
 * feature was installed by nobody: `fleet-session-title.md` never landed after
 * it shipped on 2026-09-06, and every Claude session showed up untitled while
 * the four older guidance files were being rewritten on every start.
 *
 * localStorage stays the source of truth for the user's choice (absent → on),
 * which is why this lives in the frontend rather than reusing
 * `control_plane::heal()` on the Rust side: heal reads
 * `~/.fleet/control-plane-prefs.json`, which only records a disablement made
 * through a `remove_*` call, so a feature switched off before that file existed
 * would be silently reinstalled.
 */

/** The subset of `get_hooks_setup_plan` this module needs. */
export interface ControlPlaneInstallState {
  guardInstalled: boolean;
  elicitationInstalled: boolean;
  planApprovalInstalled: boolean;
}

/** Mirrors the Claude concept toggles onto `~/.codex/AGENTS.md`. Not a toggle
 *  of its own — it reconciles against the Claude sentinels on disk. */
export const CODEX_RECONCILE_COMMAND = "reconcile_codex_guidance";

/**
 * The Tauri commands a start should fire, in order.
 *
 * Two shapes on purpose:
 *  - **hooks** (guard / elicitation / plan approval) are gated on `!installed`
 *    — they mutate `settings.json`, and there is nothing to refresh.
 *  - **guidance** (interaction / prd / wiki / model / session title) is applied
 *    unconditionally: apply is idempotent and doubles as the "refresh
 *    title+locale after an app upgrade" path.
 */
export function startupSelfHealCommands(
  installed: ControlPlaneInstallState,
  resolve: (key: string) => boolean = resolveFeature,
): string[] {
  const commands: string[] = [];

  if (resolve("guard-enabled") && !installed.guardInstalled) {
    commands.push("apply_guard_hook");
  }
  if (resolve("elicitation-enabled") && !installed.elicitationInstalled) {
    commands.push("apply_elicitation_hook");
  }
  if (resolve("interaction-mode-enabled")) {
    commands.push("apply_interaction_mode");
  }
  if (resolve("plan-approval-enabled") && !installed.planApprovalInstalled) {
    commands.push("apply_plan_approval_hook");
  }
  if (resolve("prd-mode-enabled")) {
    commands.push("apply_prd_mode");
  }
  if (resolve("wiki-guidance-enabled")) {
    commands.push("apply_wiki_guidance");
  }
  if (resolve("model-guidance-enabled")) {
    commands.push("apply_model_guidance");
  }
  if (resolve("session-title-guidance-enabled")) {
    commands.push("apply_session_title_guidance");
  }

  commands.push(CODEX_RECONCILE_COMMAND);
  return commands;
}

/**
 * Fire the plan. Each command is independent — one failure must not strand the
 * rest, which is why they are not chained. Returns what it fired, so callers
 * (and tests) can assert on it.
 */
export function runControlPlaneSelfHeal(
  invoke: (command: string) => Promise<unknown>,
  installed: ControlPlaneInstallState,
  resolve?: (key: string) => boolean,
): string[] {
  const commands = startupSelfHealCommands(installed, resolve);
  for (const command of commands) {
    Promise.resolve()
      .then(() => invoke(command))
      .catch((e: unknown) => console.error(`control-plane self-heal ${command}:`, e));
  }
  return commands;
}
