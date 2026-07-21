import type { Plugin } from "unified";
import type { Root, Element, Text, ElementContent } from "hast";
import { visit } from "unist-util-visit";

/**
 * 中文首行缩进。中文排版惯例是正文段落首行缩进 2 个字，英文段落不缩进。CSS
 * 无法按段落内容的语言来选择元素，所以在渲染时判断每个 `<p>` 的首个实义字符
 * 是否为 CJK：是则打上 `cjk-indent` 类，由 App.css 的 `text-indent: 2em` 完成
 * 缩进（`1em ≈ 1 个全角字宽`，`2em` 即 2 个中文字）。英文段落不打类、保持原样。
 *
 * 只作用于正文段落：列表项 / 引用块 / 表格单元格内的段落跳过——它们已有自己
 * 的缩进语境，再叠加首行缩进会显得错乱。必须排在 `rehype-sanitize` 之后运行，
 * 否则注入的 `className` 会被清洗掉。
 */

// 汉字（含扩展 A、兼容表意区）、CJK 标点、全角字符，外加中文常用的弯引号
// （‘–‟，覆盖 “ ” ‘ ’ 开头的中文引述段落）。判定的是段落首个实义
// 字符，所以以中文引号开头的段落也能正确缩进。
const CJK_LEADING =
  /[‘-‟　-〿㐀-䶿一-鿿豈-﫿＀-￯]/;

// markdown 生成的 hast 里，正文 <p> 的直接父就是这些容器（松散列表 li > p、
// 引用块 blockquote > p、表格 td/th > p），所以只需看直接父即可跳过它们内部
// 的段落——它们已有自己的缩进语境，再首行缩进会显得错乱。
const SKIP_PARENTS = new Set(["li", "blockquote", "td", "th"]);

/** 返回节点子树里第一个非空白字符；全空白则 null。 */
function firstMeaningfulChar(node: ElementContent): string | null {
  if (node.type === "text") {
    const m = (node as Text).value.match(/\S/);
    return m ? m[0] : null;
  }
  if (node.type === "element") {
    for (const child of (node as Element).children) {
      const c = firstMeaningfulChar(child);
      if (c) return c;
    }
  }
  return null;
}

export const rehypeCjkIndent: Plugin<[], Root> = () => (tree) => {
  visit(tree, "element", (node: Element, _index, parent) => {
    if (node.tagName !== "p") return;
    if (
      parent &&
      parent.type === "element" &&
      SKIP_PARENTS.has((parent as Element).tagName)
    )
      return;
    const ch = firstMeaningfulChar(node);
    if (!ch || !CJK_LEADING.test(ch)) return;
    node.properties ??= {};
    const cls = node.properties.className;
    const list = Array.isArray(cls)
      ? cls.map(String)
      : cls != null
        ? [String(cls)]
        : [];
    if (!list.includes("cjk-indent")) list.push("cjk-indent");
    node.properties.className = list;
  });
};
