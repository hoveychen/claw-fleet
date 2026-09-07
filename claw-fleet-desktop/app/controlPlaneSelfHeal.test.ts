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
  it("默认开且磁盘上还没装的新指引会被安装 —— 这正是会话标题指引漏装的那一格", () => {
    expect(startupSelfHealCommands(NOTHING_INSTALLED, allDefault)).toContain(
      "apply_session_title_guidance",
    );
  });

  it("老板在设置里关掉的项不会被启动自愈装回来", () => {
    const resolve = (key: string) => key !== "session-title-guidance-enabled";
    expect(startupSelfHealCommands(NOTHING_INSTALLED, resolve)).not.toContain(
      "apply_session_title_guidance",
    );
  });

  it("已装好的 hook 不再重复 apply（它们改的是 settings.json，没有可刷新的东西）", () => {
    const cmds = startupSelfHealCommands(ALL_INSTALLED, allDefault);
    expect(cmds).not.toContain("apply_guard_hook");
    expect(cmds).not.toContain("apply_elicitation_hook");
    expect(cmds).not.toContain("apply_plan_approval_hook");
  });

  it("指引类无条件 apply —— apply 幂等，同时兼任升级后刷新称呼/语言的那条路", () => {
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

  it("codex 镜像永远在最后跑 —— 它读的是 Claude 侧刚写完的 sentinel", () => {
    const cmds = startupSelfHealCommands(NOTHING_INSTALLED, allDefault);
    expect(cmds[cmds.length - 1]).toBe(CODEX_RECONCILE_COMMAND);
  });
});

describe("runControlPlaneSelfHeal", () => {
  it("一条失败不拖累其余 —— 每条命令各自 catch", async () => {
    const seen: string[] = [];
    const invoke = vi.fn((command: string) => {
      seen.push(command);
      return command === "apply_wiki_guidance"
        ? Promise.reject(new Error("boom"))
        : Promise.resolve(null);
    });
    const errors = vi.spyOn(console, "error").mockImplementation(() => {});

    const planned = runControlPlaneSelfHeal(invoke, NOTHING_INSTALLED, allDefault);
    await new Promise((r) => setTimeout(r, 0));

    expect(seen).toEqual(planned);
    expect(seen).toContain("apply_session_title_guidance");
    expect(errors).toHaveBeenCalledOnce();
    errors.mockRestore();
  });
});

/**
 * The bug was never in the plan — it was that the only caller lived inside a
 * panel that mounts on demand. Guard the wiring itself: the app shell must run
 * the self-heal, and `SettingsPanel` must not keep a second hand-written copy
 * of the apply list that can drift from this module.
 */
describe("启动路径守门", () => {
  const read = (rel: string) => readFileSync(join(__dirname, rel), "utf8");

  it("App shell 在启动时就跑自愈，不再等设置面板 mount", () => {
    const app = read("App.tsx");
    expect(app).toMatch(/runControlPlaneSelfHeal/);
  });

  it("SettingsPanel 的 mount 自愈复用同一份清单，而不是自己再抄一遍", () => {
    // 单个开关的 apply_* 调用当然要留（老板拨动开关那条路），守的是 mount 时
    // 那段「默认开就装」的清单不再有第二份手抄。
    const panel = read("components/SettingsPanel.tsx");
    expect(panel).toMatch(/runControlPlaneSelfHeal/);
    expect(panel).not.toMatch(/auto-apply session title guidance/);
  });
});
