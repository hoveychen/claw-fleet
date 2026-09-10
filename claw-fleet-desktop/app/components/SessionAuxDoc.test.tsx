// @vitest-environment jsdom
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, describe, expect, it, vi } from "vitest";

import "../i18n";
import { makeAuxDoc, type AuxDoc } from "../detailAux";
import type { AuxCardTail } from "./auxDocMenu";

// The four readers are the real 仓库 / 知识库 / 产出 panes; they reach for Tauri
// the moment they mount. What is under test here is the *wiring* — which doc
// kind picks which reader, and that each one is handed the card's own ref and
// its card-management tail — so stand them in.
vi.mock("./FileTabPane", () => ({
  FileTabPane: ({ doc }: { doc: AuxDoc }) => <div data-testid="file">{doc.ref}</div>,
}));
vi.mock("./WikiTabPane", () => ({
  WikiTabPane: ({ doc }: { doc: AuxDoc }) => <div data-testid="wiki">{doc.ref}</div>,
}));
vi.mock("./WebTabPane", () => ({
  WebTabPane: ({ doc }: { doc: AuxDoc }) => <div data-testid="web">{doc.ref}</div>,
}));
vi.mock("./ArtifactTabPane", () => ({
  ArtifactTabPane: ({ doc, tail }: { doc: AuxDoc; tail: AuxCardTail }) => (
    <div data-testid="artifact" data-others={tail.otherCount}>
      {doc.ref}
    </div>
  ),
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

const tail: AuxCardTail = {
  isExpanded: true,
  onToggle: () => {},
  onClose: () => {},
  onCloseOthers: () => {},
  onCloseAll: () => {},
  otherCount: 3,
};

function doc(kind: Parameters<typeof makeAuxDoc>[0], ref: string, label?: string) {
  return (
    <SessionAuxDoc
      doc={makeAuxDoc(kind, ref, label)}
      tail={tail}
      workspacePath="/repo"
      onOpenWiki={() => {}}
    />
  );
}

describe("SessionAuxDoc", () => {
  it("gives a file the repo reader", () => {
    const el = render(doc("file", "/repo/src/main.rs"));
    expect(el.querySelector('[data-testid="file"]')?.textContent).toBe("/repo/src/main.rs");
  });

  it("gives a wiki slug the wiki reader, and lets it open the next one here", () => {
    const el = render(doc("wiki", "arch/overview"));
    expect(el.querySelector('[data-testid="wiki"]')?.textContent).toBe("arch/overview");
  });

  it("gives a deliverable the 产出 reader, addressed by store id", () => {
    const el = render(doc("artifact", "20260909-080326", "9/8 对外更新日志"));
    expect(el.querySelector('[data-testid="artifact"]')?.textContent).toBe("20260909-080326");
  });

  it("gives a url the web reader", () => {
    const el = render(doc("web", "https://example.com/a"));
    expect(el.querySelector('[data-testid="web"]')?.textContent).toBe("https://example.com/a");
  });

  // The tail is the rail's, not the reader's: a pane builds its menu from it,
  // so a pane that never received it would render a card whose 关闭其他 / 全部关闭
  // silently did nothing.
  it("hands the reader the card-management tail", () => {
    const el = render(doc("artifact", "20260909-080326"));
    expect(el.querySelector('[data-testid="artifact"]')?.getAttribute("data-others")).toBe("3");
  });
});
