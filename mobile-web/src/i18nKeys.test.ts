import { readFileSync, readdirSync, statSync } from "node:fs";
import { join, relative, resolve } from "node:path";

import { describe, expect, it } from "vitest";

/**
 * 每个 `t("…")` 用到的中文原文都必须在 `i18n.ts` 的 DICT 里有英文条目。
 *
 * 这道门存在,是因为漏一条的失败模式是**安静的**:`t()` 查不到就原样返回那个
 * 中文 key,于是英文 UI 上那一处直接显示中文,而构建、类型检查、既有测试全是
 * 绿的。2026-09-08 产出页的压缩包浏览器就这样上线了 —— 整整 11 条文案(面包
 * 屑、搜索框、四条报错)在英文下全是中文,没有任何一道关卡拦住它。桌面端有
 * `app/localeKeys.test.ts` 抓同一类问题,移动端一直没有。
 *
 * 扫的是**源码**而不是词典,所以它随代码变化保持为真:新加一处 t() 而忘了补
 * 词条,这条测试当场报红并点名那个 key 和它所在的文件。
 */

const SRC_DIR = resolve(__dirname);
const I18N_FILE = join(SRC_DIR, "i18n.ts");

/**
 * `(?<![A-Za-z0-9_$.])` 是必需的而不是讲究:没有它,`format(".2f")`、
 * `element.at("x")` 这类以 t 结尾的标识符后面的括号也会命中,把参数当成翻译
 * key 报出来。桌面端那份守门测试踩过同样的坑(第一版报出 42 个假阳性)。
 */
const T_CALL = /(?<![A-Za-z0-9_$.])t\(\s*"((?:[^"\\]|\\.)*)"/g;

/**
 * 两种语言里长得一样、因此不进词典的字符串。
 *
 * 每一条都必须是**本来就是英文/专名**的东西 —— 词典里补一条
 * `"Fleet": "Fleet"` 只是噪音。中文文案永远不许进这张表:那正是这道门要拦的。
 */
const SAME_IN_BOTH_LANGS = new Set([
  "Fleet", // 产品名
  "Codex app-server", // 进程名,用量页按来源分组时原样显示
  "shell", // 终端页的默认命令名,两种语言都写 shell
]);

function sourceFiles(dir: string): string[] {
  const out: string[] = [];
  for (const name of readdirSync(dir)) {
    if (name === "node_modules") continue;
    const full = join(dir, name);
    if (statSync(full).isDirectory()) {
      out.push(...sourceFiles(full));
      continue;
    }
    if (!/\.tsx?$/.test(name)) continue;
    if (/\.test\.tsx?$/.test(name)) continue;
    out.push(full);
  }
  return out;
}

/** DICT 里登记了的 key。字面量既有 `"中文":` 也有裸标识符形式(`改名: "Rename"`),
 *  两种都要收 —— 只认带引号的那种会把裸键当成缺失报出来。 */
function dictKeys(): Set<string> {
  const src = readFileSync(I18N_FILE, "utf8");
  const start = src.indexOf("const DICT");
  if (start < 0) throw new Error("i18n.ts 里找不到 DICT —— 是不是改名了?");
  const body = src.slice(start);
  const out = new Set<string>();
  for (const m of body.matchAll(/^\s*"((?:[^"\\]|\\.)*)":/gm)) out.add(m[1]);
  for (const m of body.matchAll(/^\s*([^\s":,{}]+):\s*"/gm)) out.add(m[1]);
  return out;
}

describe("i18n key coverage", () => {
  /** key → 用到它的文件(相对 src/),按 key 去重后仍保留第一个出处好定位。 */
  const used = new Map<string, string>();
  for (const file of sourceFiles(SRC_DIR)) {
    const src = readFileSync(file, "utf8");
    for (const m of src.matchAll(T_CALL)) {
      if (!used.has(m[1])) used.set(m[1], relative(SRC_DIR, file));
    }
  }

  it("扫到了可观数量的 t() 调用（正则没有整体失效）", () => {
    // 这条不是凑数:上面那个 lookbehind 或文件遍历一旦写坏,`used` 会静静地变
    // 成空集,而下面那条断言就「通过」了。
    expect(used.size).toBeGreaterThan(300);
  });

  it("每个 t() 的 key 都在 DICT 里", () => {
    const keys = dictKeys();
    const missing = [...used.entries()]
      .filter(([k]) => !keys.has(k) && !SAME_IN_BOTH_LANGS.has(k))
      .map(([k, f]) => `${JSON.stringify(k)}  (${f})`)
      .sort();
    expect(
      missing,
      "DICT 缺这些条目 —— 英文 UI 上它们会原样显示中文",
    ).toEqual([]);
  });

  it("同形词允许清单不许收留中文", () => {
    // 这张表是逃生口,不是垃圾桶:往里塞一条中文就等于把这道门对那条文案关掉。
    const cjk = [...SAME_IN_BOTH_LANGS].filter((s) => /[一-鿿]/.test(s));
    expect(cjk, "中文文案要进 DICT，不是进允许清单").toEqual([]);
  });
});
