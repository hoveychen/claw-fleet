// Fleet 自己的模型目录（`claw-fleet-core/models.toml`），供 Composer 的模型 /
// 努力度下拉使用。桌面端的对应物是 `claw-fleet-desktop/app/useModelCatalog.ts`：
// 两边打的是同一个 core 函数，一个走 Tauri command，一个走 relay。
//
// 这替掉了本文件曾经在 Composer.tsx 里手抄的两份清单。那份抄写已经漂了：它声称
// Codex 的努力度梯子是 `minimal/low/medium/high`，而实测没有任何一个 Codex 模型
// 接受 `minimal`，且每个都接受 `xhigh`/`max`。
import { useEffect, useState } from "react";
import type { FleetTransport } from "./transport";
import type { PickerHarness } from "./generated/types";

/** 拿不到就返回空数组（relay 未连上、请求在途，或桌面端版本老到不认这个方法）。
 *  调用方把空数组当作「还没加载」，只显示自己的「默认」那一项——与
 *  useCodexProfiles 同样的降级取舍。 */
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

/** 某个 harness 的可选模型 → 下拉条目 `[value, label]`，开头带「默认」那一项。
 *  目录没到时只有「默认」，那正是诚实的降级：会话跑在 CLI 自己配置的模型上。 */
export function modelChoicesFor(
  catalog: PickerHarness[],
  harness: string,
  defaultLabel: string,
): Array<[string, string]> {
  const models = catalog.find((h) => h.name === harness)?.models ?? [];
  return [["", defaultLabel], ...models.map((m): [string, string] => [m.id, m.label])];
}

/** 某个模型自己的努力度梯子；没选模型时给该 harness 内的并集。
 *
 *  逐模型而不是逐 harness，因为同一个 harness 里梯子确实不同：`gpt-5.5` 到
 *  `xhigh` 为止，它的同代兄弟能到 `max` 和 `ultra`。旧代码把这件事写成了「给
 *  gpt-6-astra 开一个特例」，于是其余全错。 */
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
