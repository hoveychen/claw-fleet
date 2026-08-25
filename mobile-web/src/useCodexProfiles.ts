// 桌面主机上的 codex profile-v2 文件（`<CODEX_HOME>/<name>.config.toml`）。
// 手机端枚举不了——文件在主机上，而且 `[model_providers.<id>]` 块只说「怎么
// 连」不说「有哪些模型」，所以 profile 是 codex 配置里唯一能命名一个第三方
// 模型的东西。新会话弹层用它把这些模型补进 codex 模型下拉；选中时发的值是
// `profile:<name>`，由 codex_launch.rs 的 push_model_args 转成 `codex exec -p`。
// 桌面端的对应物是 SessionOptionPills 里的 list_codex_profiles。
import { useEffect, useState } from "react";
import type { FleetTransport } from "./transport";

export interface CodexProfile {
  name: string;
  model: string | null;
  model_provider: string | null;
  reasoning_effort: string | null;
}

/** 拿不到就返回空数组（relay 未连上、请求在途，或桌面端版本老到不认这个
 *  方法）。这里刻意不用 null 区分「不知道」——调用方只是往内置模型清单后面
 *  追加，空数组的降级行为（只显示官方模型）正好是想要的。 */
export function useCodexProfiles(client: FleetTransport | null): CodexProfile[] {
  const [profiles, setProfiles] = useState<CodexProfile[]>([]);
  useEffect(() => {
    if (!client) return;
    let alive = true;
    client
      .request<CodexProfile[]>("codex_profiles")
      .then((r) => {
        if (alive) setProfiles(r ?? []);
      })
      .catch(() => {
        if (alive) setProfiles([]);
      });
    return () => {
      alive = false;
    };
  }, [client]);
  return profiles;
}

/** profile → 模型下拉条目 `[value, label]`。标签优先用 profile 自己的 model
 *  id（用户认得的是这个），没写 model 的 profile 退回用名字。 */
export function codexProfileChoices(
  profiles: CodexProfile[],
): Array<[string, string]> {
  return profiles.map((p) => {
    const model = p.model?.trim();
    const provider = p.model_provider?.trim();
    const label = model ? (provider ? `${model} (${provider})` : model) : p.name;
    return [`profile:${p.name}`, label];
  });
}
