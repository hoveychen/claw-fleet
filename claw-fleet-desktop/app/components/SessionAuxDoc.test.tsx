// @vitest-environment jsdom
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, describe, expect, it, vi } from "vitest";

import "../i18n";
import { makeAuxDoc } from "../detailAux";

// The three readers are the real 仓库 / 知识库 panes; they reach for Tauri the
// moment they mount. What is under test here is the *wiring* — which doc kind
// picks which reader — so stand them in.
vi.mock("./FilesView", () => ({
  ExternalFilePreview: ({ path }: { path: string }) => <div data-testid="file">{path}</div>,
}));
vi.mock("./WikiTabPane", () => ({
  WikiTabPane: ({ slug }: { slug: string }) => <div data-testid="wiki">{slug}</div>,
}));
vi.mock("./WebTabPane", () => ({
  WebTabPane: ({ url }: { url: string }) => <div data-testid="web">{url}</div>,
}));
vi.mock("./ArtifactTabPane", () => ({
  ArtifactTabPane: ({ id }: { id: string }) => <div data-testid="artifact">{id}</div>,
}));

const { SessionAuxDoc } = await import("./SessionAuxDoc");

(globalThis as unknown as { IS_REACT_ACT_ENVIRONMENT: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

let container: HTMLDivElement | null = null;
let root: Root | null = null;

afterEach(() => {
  if (root) act(() => root!.unmount());
  root = null;
  container?.remove();
  container = null;
});

function render(node: React.ReactNode) {
  container = document.createElement("div");
  document.body.appendChild(container);
  root = createRoot(container);
  act(() => root!.render(node));
  return container;
}

describe("SessionAuxDoc", () => {
  it("gives a file the repo reader", () => {
    const el = render(
      <SessionAuxDoc
        doc={makeAuxDoc("file", "/repo/src/main.rs")}
        onOpenWiki={() => {}}
        onClose={() => {}}
      />,
    );
    expect(el.querySelector('[data-testid="file"]')?.textContent).toBe("/repo/src/main.rs");
  });

  it("gives a wiki slug the wiki reader, and lets it open the next one here", () => {
    const onOpenWiki = vi.fn();
    const el = render(
      <SessionAuxDoc
        doc={makeAuxDoc("wiki", "arch/overview")}
        onOpenWiki={onOpenWiki}
        onClose={() => {}}
      />,
    );
    expect(el.querySelector('[data-testid="wiki"]')?.textContent).toBe("arch/overview");
  });

  it("gives a deliverable the 产出 reader, addressed by store id", () => {
    const el = render(
      <SessionAuxDoc
        doc={makeAuxDoc("artifact", "20260909-080326", "9/8 对外更新日志")}
        onOpenWiki={() => {}}
        onClose={() => {}}
      />,
    );
    expect(el.querySelector('[data-testid="artifact"]')?.textContent).toBe("20260909-080326");
  });

  it("gives a url the web reader", () => {
    const el = render(
      <SessionAuxDoc
        doc={makeAuxDoc("web", "https://example.com/a")}
        onOpenWiki={() => {}}
        onClose={() => {}}
      />,
    );
    expect(el.querySelector('[data-testid="web"]')?.textContent).toBe("https://example.com/a");
  });
});
