// dsh 的模型目录 —— 手机端的模型/effort 下拉数据源。
//
// dsh 是唯一一个模型清单不由 Fleet 策划的 agent:它把主机上配好的 provider 通过
// `llm.models` 发出来,这台机器上是 2 个 DeepSeek 加 276 个 openrouter 模型、
// 横跨 43 个 vendor。手机拿不到这份配置(它在主机的 ~/.dsh/settings.yaml 里),
// 所以走 relay 的 `dsh_models` 方法要。
//
// 桌面端的对应物是 claw-fleet-desktop/app/modelChoices.ts 的 dshModelMenu ——
// 那边是两级 popover,这边只有原生 <select>,所以改用 <optgroup> 表达同一套分组
// 规则(vendor 划分与顺序两端一致,菜单不会因为换个端就重排)。
import { useEffect, useState } from "react";
import type { FleetTransport } from "./transport";
import type { DshModelCatalog } from "./generated/types";

/** 顶到菜单前面的 openrouter vendor。老板钦定的顺序,不是推导出来的:线上数据
 *  里没有任何可排序的热度/时新信号(openrouter 每一行的 description 都是 null)。
 *  与桌面端 DSH_FEATURED_VENDORS 保持一致。 */
export const DSH_FEATURED_VENDORS: string[] = [
  "anthropic",
  "deepseek",
  "openai",
  "google",
  "moonshotai",
];

/** 模型数不超过这个值的 group 整组平铺 —— 把两个 DeepSeek 模型折进子分组只会
 *  多一次操作、什么也省不下。超过则按 vendor 拆分组。 */
export const DSH_INLINE_GROUP_CAP = 20;

/** 一组下拉条目。`label` 为空表示不加 optgroup 直接平铺。 */
export interface DshModelOptGroup {
  label: string;
  models: Array<[string, string]>;
}

/** `anthropic/claude-opus-5` → `anthropic`;不带前缀的返回 ""。 */
function vendorOf(modelId: string): string {
  const i = modelId.indexOf("/");
  return i > 0 ? modelId.slice(0, i) : "";
}

/** 把目录整成 <select> 的分组条目。
 *
 *  构造上是全覆盖的:目录里每个模型都恰好出现在一个分组里。漏掉一个就意味着它
 *  在 UI 上够不着,而 dsh 明明收这个 spec。
 *
 *  目录缺失/为空时返回空数组而不是报错 —— 下拉于是只剩自己的「默认」项,这是
 *  诚实的:会话会跑在 ~/.dsh/settings.yaml 选中的模型上。字段一律防御性读取,
 *  主机的 Fleet 版本可能早于其中任何一个。 */
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
    // 先按 vendor 装桶,再按老板钦定的顺序吐出 featured vendor —— 这样目录变大
    // 时菜单顺序不会跟着重排。
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

/** 选中模型自己的 effort 阶梯,以及 dsh 自己的默认值。每个模型的阶梯不同,选
 *  Claude 那套固定档位会发出 dsh 不认的值。 */
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

/** 主机上 dsh 的模型目录。拿不到就返回 null(relay 没连上、请求在途、主机没装
 *  dsh、或桌面端版本老到不认这个方法)—— 调用方把 null 当「只有默认项」。 */
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
