// @vitest-environment jsdom
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

vi.mock("react-i18next", () => ({
  useTranslation: () => ({ t: (_k: string, d?: string) => d ?? "" }),
}));
// The fold's body renders markdown through TextBlock, which drags in the
// clipboard/tauri stack. This suite is about open/closed state, so a marker
// stands in for the body.
vi.mock("./TextBlock", () => ({
  TextBlock: () => <div data-testid="body" />,
}));

import { MetaFoldBlock } from "./MetaFoldBlock";

(globalThis as unknown as { IS_REACT_ACT_ENVIRONMENT: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

let container: HTMLDivElement | null = null;
let root: Root | null = null;

function render(forceOpen: boolean) {
  act(() => {
    root!.render(<MetaFoldBlock segments={["some injected system context"]} forceOpen={forceOpen} />);
  });
}

function isOpen(): boolean {
  return !!container!.querySelector('[data-testid="body"]');
}

function clickHeader() {
  act(() => {
    container!.querySelector("button")!.dispatchEvent(new MouseEvent("click", { bubbles: true }));
  });
}

beforeEach(() => {
  container = document.createElement("div");
  document.body.appendChild(container);
  root = createRoot(container);
});

afterEach(() => {
  act(() => root!.unmount());
  container!.remove();
  container = null;
  root = null;
});

describe("meta fold open state", () => {
  it("opens when the active search hit lands inside it", () => {
    render(false);
    expect(isOpen()).toBe(false);
    render(true);
    expect(isOpen()).toBe(true);
  });

  it("stays open after the reader steps off the hit", () => {
    render(true);
    expect(isOpen()).toBe(true);
    render(false);
    expect(isOpen()).toBe(true);
  });

  it("keeps a manual expand when the signal flips", () => {
    render(false);
    clickHeader();
    expect(isOpen()).toBe(true);
    render(false);
    expect(isOpen()).toBe(true);
  });

  it("keeps a manual collapse when the signal goes false", () => {
    render(true);
    clickHeader();
    expect(isOpen()).toBe(false);
    render(false);
    expect(isOpen()).toBe(false);
  });
});
