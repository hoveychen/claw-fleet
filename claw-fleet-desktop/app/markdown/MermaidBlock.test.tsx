// @vitest-environment jsdom
// A wiki doc's mermaid diagram used to flash a short skeleton block for a few
// frames (the prose below jumping up and back) whenever the wiki list reloaded:
// the reload rebuilt TextBlock's markdown `components`, the new `code` function
// remounted every MermaidBlock, and each one waited on mermaid again with the
// 180px skeleton on screen. Two guards: TextBlock keeps `code` stable across a
// wiki-context change, and a remounted block paints its cached SVG at once.
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";

const render = vi.fn(async (_id: string, _code: string) => ({
  svg: '<svg viewBox="0 0 120 60" class="diagram-svg"></svg>',
}));
vi.mock("mermaid", () => ({ default: { initialize: vi.fn(), render } }));
vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));
vi.mock("@tauri-apps/plugin-clipboard-manager", () => ({ writeText: vi.fn() }));
vi.mock("@tauri-apps/plugin-opener", () => ({ openUrl: vi.fn() }));

await import("../i18n");
const { TextBlock } = await import("../components/blocks/TextBlock");
const { MermaidBlock } = await import("./MermaidBlock");

(globalThis as unknown as { IS_REACT_ACT_ENVIRONMENT: boolean }).IS_REACT_ACT_ENVIRONMENT = true;
globalThis.ResizeObserver ??= class {
  observe() {}
  unobserve() {}
  disconnect() {}
} as unknown as typeof ResizeObserver;

let host: HTMLDivElement;
let root: Root;

beforeEach(() => {
  render.mockClear();
  host = document.createElement("div");
  document.body.appendChild(host);
  root = createRoot(host);
});

afterEach(() => {
  act(() => root.unmount());
  host.remove();
});

async function flush() {
  await act(async () => {
    for (let i = 0; i < 5; i++) await Promise.resolve();
  });
}

const wiki = () => ({ hasSlug: () => true, openSlug: () => {} });

describe("mermaid diagram stability", () => {
  it("keeps the diagram mounted when the wiki link context changes", async () => {
    const text = "intro\n\n```mermaid\nflowchart TB\n  StableA --> StableB\n```\n\noutro";
    act(() => root.render(<TextBlock text={text} wiki={wiki()} />));
    await flush();
    const svg = host.querySelector("svg.diagram-svg");
    expect(svg).not.toBeNull();

    // A wiki-list reload hands TextBlock a fresh context object.
    act(() => root.render(<TextBlock text={text} wiki={wiki()} />));
    expect(host.querySelector("svg.diagram-svg")).toBe(svg);
    expect(render).toHaveBeenCalledTimes(1);
  });

  it("paints the cached SVG synchronously when a rendered diagram remounts", async () => {
    const code = "flowchart TB\n  RemountA --> RemountB";
    act(() => root.render(<MermaidBlock key="a" code={code} />));
    await flush();
    expect(host.querySelector("svg.diagram-svg")).not.toBeNull();

    // New key = unmount + mount; no flush before asserting.
    act(() => root.render(<MermaidBlock key="b" code={code} />));
    expect(host.querySelector("svg.diagram-svg")).not.toBeNull();
    expect(render).toHaveBeenCalledTimes(1);
  });
});
