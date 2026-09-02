import { readFileSync, readdirSync, statSync } from "node:fs";
import { join, relative, resolve } from "node:path";

import { describe, expect, it } from "vitest";

import enJson from "./locales/en.json";
import zhJson from "./locales/zh.json";

/**
 * 每个 `t("key")` 用到的 key 都必须在 en.json 与 zh.json 里都有。
 *
 * 这道门存在,是因为漏一个键的失败模式是**安静的**。i18next 找不到键时回落到
 * 第二个参数,而我们的 fallback 一律写成中文 —— 于是英文 UI 上那一处直接显示
 * 中文,构建、类型检查、既有测试全都是绿的。「移动端」面板里 relay 地址旁边那个
 * 按钮就这样带着中文「编辑」上线了很久,只有人真的把 UI 切成英文点到那一页才
 * 看得见。连一个 fallback 都没写的键更糟:两种语言都显示 key 本身。
 *
 * 反方向同样拦:zh.json 缺键时中文 UI 靠内联 fallback 侥幸看着对,但那份文案就
 * 不在 locale 文件里,翻译、审校、复用全都绕过它 —— 而下一个把 fallback 删掉的
 * 人不会知道自己删掉的是唯一一份中文。
 *
 * 扫的是**源码**而不是 locale 文件,所以它随代码变化保持为真:新加一处 t() 而
 * 忘了补键,这条测试当场报红并点名那个键。
 */

const APP_DIR = resolve(__dirname);

/** locale JSON 是嵌套的(`{schedule: {title: …}}`),而 t() 里写的是点号路径。 */
function flatten(obj: unknown, prefix = ""): Set<string> {
  const out = new Set<string>();
  if (typeof obj !== "object" || obj === null) return out;
  for (const [k, v] of Object.entries(obj as Record<string, unknown>)) {
    const key = prefix ? `${prefix}.${k}` : k;
    if (typeof v === "object" && v !== null) {
      for (const nested of flatten(v, key)) out.add(nested);
    } else {
      out.add(key);
    }
  }
  return out;
}

/**
 * `t("key")` / `t("key", "fallback")` 的调用点。
 *
 * `(?<![A-Za-z0-9_$.])` 是必需的而不是讲究:没有它,`createElement("td")` 里
 * 那个 `t(` 也会命中,于是 `td`、`tr`、`iframe` 这些 HTML 标签名会被当成缺失的
 * 翻译键报出来(第一版就是这样,报出 42 个假阳性)。
 */
const T_CALL = /(?<![A-Za-z0-9_$.])t\(\s*"([A-Za-z0-9_.]+)"/g;

/** 只收动态键会用到的前缀白名单 —— 目前没有,留空数组即为「一个都不许有」。 */
const DYNAMIC_KEY_PREFIXES: string[] = [];

function sourceFiles(dir: string): string[] {
  const out: string[] = [];
  for (const name of readdirSync(dir)) {
    if (name === "node_modules" || name === "locales" || name === "mock") continue;
    const full = join(dir, name);
    if (statSync(full).isDirectory()) {
      out.push(...sourceFiles(full));
      continue;
    }
    if (!/\.(ts|tsx)$/.test(name)) continue;
    if (/\.test\.tsx?$/.test(name)) continue;
    out.push(full);
  }
  return out;
}

describe("locale key coverage", () => {
  const en = flatten(enJson);
  const zh = flatten(zhJson);

  /** key → 用到它的文件(相对 app/),按 key 去重后仍保留第一个出处好定位。 */
  const used = new Map<string, string>();
  for (const file of sourceFiles(APP_DIR)) {
    const src = readFileSync(file, "utf8");
    for (const m of src.matchAll(T_CALL)) {
      const key = m[1];
      if (DYNAMIC_KEY_PREFIXES.some((p) => key.startsWith(p))) continue;
      if (!used.has(key)) used.set(key, relative(APP_DIR, file));
    }
  }

  it("扫到了可观数量的 t() 调用（正则没有整体失效）", () => {
    // 这条不是凑数:上面那个 lookbehind 或文件遍历一旦写坏,`used` 会静静地变成
    // 空集,而下面两条断言就全都「通过」了。
    expect(used.size).toBeGreaterThan(200);
  });

  it("每个 key 都在 en.json 里", () => {
    const missing = [...used.entries()]
      .filter(([k]) => !en.has(k))
      .map(([k, f]) => `${k}  (${f})`)
      .sort();
    expect(missing, `en.json 缺这些键 —— 英文 UI 会显示中文 fallback 或 key 本身`).toEqual([]);
  });

  it("每个 key 都在 zh.json 里", () => {
    const missing = [...used.entries()]
      .filter(([k]) => !zh.has(k))
      .map(([k, f]) => `${k}  (${f})`)
      .sort();
    expect(missing, `zh.json 缺这些键 —— 那份中文只活在内联 fallback 里`).toEqual([]);
  });
});
