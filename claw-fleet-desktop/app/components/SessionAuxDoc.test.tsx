// @vitest-environment jsdom
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, describe, expect, it, vi } from "vitest";

import "../i18n";
import { makeAuxDoc } from "../detailAux";

// The three readers are the real 仓库 / 知识库 panes; they reach for Tauri the
// moment they mount. What is under test here is the *wiring* — which kind picks
// which reader, and that the chips report back — so stand them in.
vi.mock("./FilesView", () => ({
  ExternalFilePreview: ({ path }: { path: string }) => <div data-testid="file">{path}</div>,
}));
vi.mock("./WikiTabPane", () => ({
  WikiTabPane: ({ slug }: { slug: string }) => <div data-testid="wiki">{slug}</div>,
}));
vi.mock("./WebTabPane", () => ({
  WebTabPane: ({ url }: { url: string }) => <div data-testid="web">{url}</div>,
}));

const { SessionAuxDoc, SessionAuxDocStrip } = await import("./SessionAuxDoc");

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

describe("SessionAuxDocStrip", () => {
  const docs = [makeAuxDoc("file", "/repo/a.rs"), makeAuxDoc("wiki", "arch/overview")];

  it("renders nothing when nothing is open", () => {
    const el = render(
      <SessionAuxDocStrip docs={[]} activeId={null} onPick={() => {}} onClose={() => {}} />,
    );
    expect(el.textContent).toBe("");
  });

  it("names each doc and reports the one clicked", () => {
    const onPick = vi.fn();
    const el = render(
      <SessionAuxDocStrip
        docs={docs}
        activeId={docs[0].id}
        onPick={onPick}
        onClose={() => {}}
      />,
    );
    const labels = Array.from(el.querySelectorAll("button")).map((b) => b.textContent);
    expect(labels).toContain("a.rs");
    expect(labels).toContain("overview");

    const second = Array.from(el.querySelectorAll("button")).find(
      (b) => b.textContent === "overview",
    )!;
    act(() => second.dispatchEvent(new MouseEvent("click", { bubbles: true })));
    expect(onPick).toHaveBeenCalledWith(docs[1].id);
  });

  it("reports a close separately from a pick", () => {
    const onPick = vi.fn();
    const onClose = vi.fn();
    const el = render(
      <SessionAuxDocStrip
        docs={docs}
        activeId={docs[0].id}
        onPick={onPick}
        onClose={onClose}
      />,
    );
    const closes = Array.from(el.querySelectorAll("button")).filter(
      (b) => b.textContent === "✕",
    );
    act(() => closes[0].dispatchEvent(new MouseEvent("click", { bubbles: true })));
    expect(onClose).toHaveBeenCalledWith(docs[0].id);
    expect(onPick).not.toHaveBeenCalled();
  });
});
