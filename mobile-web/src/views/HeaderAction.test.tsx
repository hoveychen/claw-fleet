// Pin down two things screenshots can't catch: busy rotation is triggered by CSS based on an attribute,
// and busy and disabled are two independent axes.
//
// Of the four hand-written versions, only the usage page defines :disabled; none had busy — so after
// hitting refresh, the screen doesn't respond during those hundreds of milliseconds of network round-trip.
// After adding it, "busy" and "disabled" must stay visually distinct: a refresh in flight is busy (spinning),
// while an unconnected page is disabled (grayed out). If they shared the same appearance, users can't tell
// "wait a moment" from "can't click".

import { describe, expect, it } from "vitest";
import { renderToStaticMarkup } from "react-dom/server";
import { HeaderAction } from "./HeaderAction";

const noop = () => {};
const render = (props: Partial<Parameters<typeof HeaderAction>[0]> = {}) =>
  renderToStaticMarkup(
    <HeaderAction icon={<svg />} label="刷新" onClick={noop} {...props} />,
  );

describe("HeaderAction", () => {
  it("by default does not set data-busy — rotation is triggered by CSS based on this attribute", () => {
    expect(render()).not.toContain("data-busy");
  });

  it("busy sets data-busy and aria-busy, but does not gray out", () => {
    const html = render({ busy: true });
    expect(html).toContain('data-busy="true"');
    expect(html).toContain('aria-busy="true"');
    expect(html).not.toContain("disabled");
  });

  it("disabled grays out but does not spin — can't-click and wait-a-moment are different", () => {
    const html = render({ disabled: true });
    expect(html).toContain("disabled");
    expect(html).not.toContain("data-busy");
  });

  it("button has no visible text, so aria-label must be rendered in the DOM", () => {
    expect(render()).toContain('aria-label="刷新"');
  });

  it("is type=button — must not submit when header falls within another form", () => {
    expect(render()).toContain('type="button"');
  });
});
