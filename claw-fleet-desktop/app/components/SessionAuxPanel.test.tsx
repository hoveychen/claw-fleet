// @vitest-environment jsdom
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, describe, expect, it, vi } from "vitest";

import "../i18n";
import { SessionAuxPanel } from "./SessionAuxPanel";

(globalThis as unknown as { IS_REACT_ACT_ENVIRONMENT: boolean }).IS_REACT_ACT_ENVIRONMENT = true;
Element.prototype.scrollIntoView = vi.fn();

let container: HTMLDivElement | null = null;
let root: Root | null = null;

afterEach(() => {
  if (root) act(() => root!.unmount());
  root = null;
  container?.remove();
  container = null;
});

function renderDrawer(onClose = vi.fn()) {
  container = document.createElement("div");
  document.body.appendChild(container);
  root = createRoot(container);
  act(() => root!.render(
    <SessionAuxPanel
      tabs={[{ id: "skills", label: "Skills" }]}
      activeId="skills"
      onPick={() => {}}
      onCloseTab={() => {}}
      onClose={onClose}
    >
      <div>skill content</div>
    </SessionAuxPanel>,
  ));
  return { container, onClose };
}

describe("SessionAuxPanel", () => {
  it("always renders as an overlay drawer without a layout width", () => {
    const { container } = renderDrawer();
    const aside = container.querySelector("aside");

    expect(aside).not.toBeNull();
    expect(aside?.style.width).toBe("");
    expect(aside?.previousElementSibling).not.toBeNull();
    expect(aside?.textContent).toContain("skill content");
  });

  it("closes when the scrim is clicked", () => {
    const { container, onClose } = renderDrawer();
    const scrim = container.querySelector("aside")?.previousElementSibling as HTMLElement;

    act(() => scrim.click());
    expect(onClose).toHaveBeenCalledOnce();
  });
});
