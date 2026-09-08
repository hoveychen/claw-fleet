import { describe, expect, it } from "vitest";
import { renderToStaticMarkup } from "react-dom/server";
import { AppHeader } from "./AppHeader";

const noop = () => {};

describe("AppHeader", () => {
  it("字符串标题走标准包装，节点标题原样透传", () => {
    // 会话详情的标题行自带 subagent 徽标和一个展开详情面板的 tap 目标，
    // 它必须能整块塞进来而不被再包一层标题样式 —— 否则 flex 权重全乱。
    const asString = renderToStaticMarkup(<AppHeader onBack={noop} title="终端" />);
    expect(asString).toContain("终端");
    expect(asString).toMatch(/class="[^"]*title[^"]*"/);

    const asNode = renderToStaticMarkup(
      <AppHeader onBack={noop} title={<span id="custom">会话</span>} />,
    );
    expect(asNode).toContain('id="custom"');
    expect(asNode).not.toMatch(/class="[^"]*title[^"]*"/);
  });

  it("没传 sub / actions 就不渲染那两个容器", () => {
    // 空的 flex 子项会连带 .header 的 gap 一起占位，把标题往左挤。
    const bare = renderToStaticMarkup(<AppHeader onBack={noop} title="仓库" />);
    expect(bare).not.toMatch(/class="[^"]*sub[^"]*"/);
    expect(bare).not.toMatch(/class="[^"]*actions[^"]*"/);

    const full = renderToStaticMarkup(
      <AppHeader onBack={noop} title="仓库" sub="/tmp/x" actions={<button>⟳</button>} />,
    );
    expect(full).toMatch(/class="[^"]*sub[^"]*"/);
    expect(full).toMatch(/class="[^"]*actions[^"]*"/);
  });

  it("seamless 才打 data-seamless，缺省不打（底线由 CSS 按属性开关）", () => {
    expect(renderToStaticMarkup(<AppHeader onBack={noop} title="会话" seamless />)).toContain(
      'data-seamless="true"',
    );
    expect(renderToStaticMarkup(<AppHeader onBack={noop} title="会话" />)).not.toContain(
      "data-seamless",
    );
  });

  it("titleAfter 与字符串标题同行，且标题才是那个会收缩的", () => {
    // 知识库的条目数紧贴标题；标题长了应该是标题被截断，而不是把计数挤走。
    const html = renderToStaticMarkup(
      <AppHeader onBack={noop} title="知识库" titleAfter={<span id="n">42</span>} />,
    );
    expect(html).toMatch(/class="[^"]*titleRow[^"]*"/);
    // 计数是 titleRow 的直接子节点，不在 .title 内部（否则会跟着一起被 ellipsis 吃掉）。
    expect(html).toMatch(/class="[^"]*title[^"]*">知识库<\/div><span id="n">/);
  });

  it("返回键始终带 aria-label —— 它没有可见文字了", () => {
    const html = renderToStaticMarkup(<AppHeader onBack={noop} title="用量" />);
    expect(html).toMatch(/<button[^>]*aria-label="[^"]+"/);
  });
});
