// 钉住两件在截图里抓不到的事：busy 的旋转是 CSS 按属性开的，而 busy 与
// disabled 是**两个**轴。
//
// 四份手搓版本里只有用量页写了 :disabled，没有一份有 busy —— 于是按下刷新之后
// 那几百毫秒的网络往返里屏幕上什么都不动。补上之后，「在忙」和「不可用」必须
// 保持可分：一次刷新在飞是 busy（转），一个还没连上桌面端的页是 disabled
// （置灰）。两者共用一种长相的话，人分不出「等一下」和「点不了」。

import { describe, expect, it } from "vitest";
import { renderToStaticMarkup } from "react-dom/server";
import { HeaderAction } from "./HeaderAction";

const noop = () => {};
const render = (props: Partial<Parameters<typeof HeaderAction>[0]> = {}) =>
  renderToStaticMarkup(
    <HeaderAction icon={<svg />} label="刷新" onClick={noop} {...props} />,
  );

describe("HeaderAction", () => {
  it("缺省不打 data-busy —— 旋转由 CSS 按这个属性开", () => {
    expect(render()).not.toContain("data-busy");
  });

  it("busy 打 data-busy 与 aria-busy，但不置灰", () => {
    const html = render({ busy: true });
    expect(html).toContain('data-busy="true"');
    expect(html).toContain('aria-busy="true"');
    expect(html).not.toContain("disabled");
  });

  it("disabled 置灰但不转 —— 「点不了」和「等一下」是两件事", () => {
    const html = render({ disabled: true });
    expect(html).toContain("disabled");
    expect(html).not.toContain("data-busy");
  });

  it("按钮上没有可见文字，所以 aria-label 必须落到 DOM 上", () => {
    expect(render()).toContain('aria-label="刷新"');
  });

  it("是 type=button —— header 落在别的 form 里时不能提交它", () => {
    expect(render()).toContain('type="button"');
  });
});
