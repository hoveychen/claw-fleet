// @vitest-environment jsdom
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, describe, expect, it, vi } from "vitest";

import type { DocHistoryEntry } from "../docHistory";
import type { ExplainRecord } from "../explainApi";
import type { SessionInfo } from "../types";
import { SessionLibraryPanel } from "./SessionLibraryPanel";

(globalThis as unknown as { IS_REACT_ACT_ENVIRONMENT: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

let container: HTMLDivElement | null = null;
let root: Root | null = null;

afterEach(() => {
  if (root) act(() => root!.unmount());
  root = null;
  container?.remove();
  container = null;
});

const doc = (ref: string, label: string): DocHistoryEntry => ({
  kind: "file",
  ref,
  label,
  ts: 1,
});

const explain = (id: string, question: string): ExplainRecord =>
  ({
    id,
    sessionId: "s1",
    question,
    quote: "quoted text",
    createdMs: 1,
    status: "done",
    text: "answer",
  }) as unknown as ExplainRecord;

const agent = (id: string, title: string): SessionInfo =>
  ({
    id,
    aiTitle: title,
    status: "idle",
    isSubagent: true,
    lastActivityMs: 1,
  }) as unknown as SessionInfo;

function render(props: Partial<Parameters<typeof SessionLibraryPanel>[0]> = {}) {
  container = document.createElement("div");
  document.body.appendChild(container);
  root = createRoot(container);
  act(() =>
    root!.render(
      <SessionLibraryPanel
        explains={[]}
        hiddenExplains={new Set()}
        docs={[]}
        subagents={[]}
        onOpenExplain={() => {}}
        onOpenDoc={() => {}}
        onForgetDoc={() => {}}
        onForgetAllDocs={() => {}}
        onOpenAgent={() => {}}
        {...props}
      />,
    ),
  );
  return container!;
}

function click(el: Element | null | undefined) {
  act(() => {
    el?.dispatchEvent(new MouseEvent("click", { bubbles: true }));
  });
}

describe("SessionLibraryPanel", () => {
  it("says the session has nothing yet rather than rendering a blank pane", () => {
    const el = render();
    expect(el.textContent).toContain("边栏里出现过的东西都会留在这里");
  });

  it("lists all three groups with their counts", () => {
    const el = render({
      explains: [explain("e1", "这段什么意思")],
      docs: [doc("/repo/src/main.rs", "main.rs"), doc("/repo/src/lib.rs", "lib.rs")],
      subagents: [agent("agent-1", "扫描依赖")],
    });
    expect(el.textContent).toContain("这段什么意思");
    expect(el.textContent).toContain("main.rs");
    expect(el.textContent).toContain("lib.rs");
    expect(el.textContent).toContain("扫描依赖");
  });

  it("marks a question the rail has dismissed, so the row explains why it is only here", () => {
    const el = render({
      explains: [explain("e1", "问题甲"), explain("e2", "问题乙")],
      hiddenExplains: new Set(["e2"]),
    });
    const rows = [...el.querySelectorAll("button")].filter((b) => b.textContent?.includes("问题"));
    expect(rows.find((r) => r.textContent?.includes("问题甲"))?.textContent).not.toContain("已收起");
    expect(rows.find((r) => r.textContent?.includes("问题乙"))?.textContent).toContain("已收起");
  });

  it("hands a doc row back to the rail with the kind and ref it was stored under", () => {
    const onOpenDoc = vi.fn();
    const el = render({ docs: [doc("/repo/src/main.rs", "main.rs")], onOpenDoc });
    click([...el.querySelectorAll("button")].find((b) => b.textContent?.includes("main.rs")));
    expect(onOpenDoc).toHaveBeenCalledWith("file", "/repo/src/main.rs", "main.rs");
  });

  it("forgets one doc without touching the rest", () => {
    const onForgetDoc = vi.fn();
    const el = render({ docs: [doc("/a.rs", "a.rs"), doc("/b.rs", "b.rs")], onForgetDoc });
    // The ✕ is the second button in each row wrapper.
    const wrap = [...el.querySelectorAll("[class*='library_row_wrap']")].find((w) =>
      w.textContent?.includes("a.rs"),
    );
    click(wrap?.querySelectorAll("button")[1]);
    expect(onForgetDoc).toHaveBeenCalledWith("file", "/a.rs");
  });

  it("offers a subagent that already finished", () => {
    const onOpenAgent = vi.fn();
    const finished = agent("agent-9", "已结束的活");
    const el = render({ subagents: [finished], onOpenAgent });
    click([...el.querySelectorAll("button")].find((b) => b.textContent?.includes("已结束的活")));
    expect(onOpenAgent).toHaveBeenCalledWith(finished);
  });

  it("only shows the clear-all action for docs, the one group with a delete", () => {
    const withDocs = render({ docs: [doc("/a.rs", "a.rs")] });
    expect(withDocs.textContent).toContain("清空");
    act(() => root!.unmount());
    root = null;
    container?.remove();
    const withoutDocs = render({ explains: [explain("e1", "q")] });
    expect(withoutDocs.textContent).not.toContain("清空");
  });
});
