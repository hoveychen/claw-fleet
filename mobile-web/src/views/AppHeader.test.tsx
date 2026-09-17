import { describe, expect, it } from "vitest";
import { renderToStaticMarkup } from "react-dom/server";
import { AppHeader } from "./AppHeader";

const noop = () => {};

describe("AppHeader", () => {
  it("字符串标题走标准包装，节点标题原样透传", () => {
    // Session detail title row comes with subagent badge and tap target to expand detail panel,
    // it must fit in whole without wrapping in another layer of title styling — else flex weights break.
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
    // Empty flex children occupy space along with .header gap, pushing title left.
    const bare = renderToStaticMarkup(<AppHeader onBack={noop} title="仓库" />);
    expect(bare).not.toMatch(/class="[^"]*sub[^"]*"/);
    expect(bare).not.toMatch(/class="[^"]*actions[^"]*"/);

    const full = renderToStaticMarkup(
      <AppHeader onBack={noop} title="仓库" sub="/tmp/x" actions={<button>x</button>} />,
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
    // Wiki entry count sits tight against title; when title is long, title should truncate,
    // not get pushed away by count.
    const html = renderToStaticMarkup(
      <AppHeader onBack={noop} title="知识库" titleAfter={<span id="n">42</span>} />,
    );
    expect(html).toMatch(/class="[^"]*titleRow[^"]*"/);
    // Count is direct child of titleRow, not inside .title (else would be consumed by ellipsis too).
    expect(html).toMatch(/class="[^"]*title[^"]*">知识库<\/div><span id="n">/);
  });

  it("返回键始终带 aria-label —— 它没有可见文字了", () => {
    const html = renderToStaticMarkup(<AppHeader onBack={noop} title="用量" />);
    expect(html).toMatch(/<button[^>]*aria-label="[^"]+"/);
  });
});
