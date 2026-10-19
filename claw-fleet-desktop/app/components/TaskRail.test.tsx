// @vitest-environment jsdom
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, describe, expect, it, vi } from "vitest";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn(() => new Promise(() => {})) }));

import "../i18n";
import type { SessionInfo } from "../types";
import type { Artifact } from "./ArtifactsView";
import type { TaskRow } from "./homeRows";
import { collectReviewDocs, TaskRail, taskRailHasContent } from "./TaskRail";
import { IngestOpenProvider, type IngestOpenContext } from "./blocks/ingestOpenContext";
import { useUIStore } from "../store";

(globalThis as unknown as { IS_REACT_ACT_ENVIRONMENT: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

let container: HTMLDivElement | null = null;
let root: Root | null = null;

afterEach(() => {
  if (root) act(() => root!.unmount());
  root = null;
  container?.remove();
  container = null;
});

const main = { id: "main" } as unknown as SessionInfo;

function row(over: Partial<TaskRow> = {}): TaskRow {
  return { hops: [main], tip: main, subtasks: [], ...over } as unknown as TaskRow;
}

function render(props: Partial<Parameters<typeof TaskRail>[0]> = {}, ingest: IngestOpenContext | null = null) {
  container = document.createElement("div");
  document.body.appendChild(container);
  root = createRoot(container);
  act(() =>
    root!.render(
      <IngestOpenProvider value={ingest}>
      <TaskRail
        row={null}
        openSessionId="main"
        steps={null}
        artifacts={null}
        onOpenSession={() => {}}
        ambient={null}
        empty={false}
        developer={null}
        {...props}
      />
      </IngestOpenProvider>,
    ),
  );
  return container;
}

describe("taskRailHasContent", () => {
  // The bug this guards: the rail was mounted for every task and padded out
  // with "nothing filed yet", so a fresh task got a column of nothing.
  it("is false for a task with no sub-tasks, steps or material", () => {
    expect(taskRailHasContent(row(), "main", null, [])).toBe(false);
    expect(taskRailHasContent(row(), "main", [], null)).toBe(false);
  });

  it("is false with no task at all", () => {
    expect(taskRailHasContent(null, "main", null, null)).toBe(false);
  });

  it("is true once any task section has something in it", () => {
    expect(taskRailHasContent(row(), "main", [{ text: "P1", done: false } as never], [])).toBe(true);
    expect(taskRailHasContent(row(), "main", null, [{ id: "x" } as Artifact])).toBe(true);
    // Reading a sub-task: the rail carries the way back to the main line.
    expect(taskRailHasContent(row(), "sub", null, [])).toBe(true);
  });
});

describe("TaskRail", () => {
  it("omits the material section while nothing is filed", () => {
    const el = render({ row: row(), artifacts: [] });
    expect(el.querySelectorAll("section")).toHaveLength(1); // developer view only
  });

  it("says why it is empty when the switch holds it open", () => {
    const el = render({ empty: true });
    expect(el.querySelector("aside > p")?.textContent).toBeTruthy();
  });

  // A session with no task still gets the one rail, for its subagents, docs
  // and side questions.
  it("hosts the ambient sections on a session with no task", () => {
    const el = render({ ambient: <section data-testid="ambient">agents</section> });
    expect(el.querySelector("[data-testid='ambient']")).not.toBeNull();
  });

  // Material opens in the floating reader beside the conversation, like a
  // transcript's ingest card, instead of jumping to the artifacts page.
  it("opens material in the rail first, and the page only once it is open there", () => {
    const nav = vi.fn();
    useUIStore.setState({ requestArtifactNav: nav } as never);
    const art = { id: "a1", title: "Report", name: "r.md", mime: "text/markdown", note: "" } as Artifact;
    const open = vi.fn();
    let el = render({ row: row(), artifacts: [art] }, { open, expandedId: null });
    const item = () => el.querySelector<HTMLButtonElement>("button[title='r.md']")!;
    act(() => item().click());
    expect(open).toHaveBeenCalledWith("artifact", "a1", "Report");
    expect(nav).not.toHaveBeenCalled();

    act(() => root!.unmount());
    container!.remove();
    el = render({ row: row(), artifacts: [art] }, { open, expandedId: "artifact:a1" });
    expect(item().getAttribute("aria-current")).toBe("true");
    act(() => item().click());
    expect(nav).toHaveBeenCalledWith("a1");
  });

  // fleet__ask reviewDocs used to render as a tab strip inside the card; they
  // are listed with the material instead and open in the rail's reader.
  it("lists review docs as material, even on a session with no task", () => {
    const open = vi.fn();
    const docs = [
      { kind: "wiki" as const, ref: "human-zoo/menu-ux-research", title: "Menu research" },
      { kind: "file" as const, ref: "/repo/docs/plan.md" },
    ];
    expect(taskRailHasContent(null, "main", null, null, docs)).toBe(true);
    const el = render({ reviewDocs: docs }, { open, expandedId: null });
    expect(el.textContent).toContain("Menu research");
    expect(el.textContent).toContain("plan.md");
    act(() => el.querySelector<HTMLButtonElement>("button[title='/repo/docs/plan.md']")!.click());
    expect(open).toHaveBeenCalledWith("file", "/repo/docs/plan.md", "plan.md");
  });
});

describe("collectReviewDocs", () => {
  it("dedupes across cards, pending first, then newest answered", () => {
    const a = { kind: "wiki" as const, ref: "a" };
    const b = { kind: "file" as const, ref: "/b.md" };
    const c = { kind: "wiki" as const, ref: "c" };
    const got = collectReviewDocs([{ reviewDocs: [a] }], [{ reviewDocs: [b] }, { reviewDocs: [c, a] }, {}]);
    expect(got.map((d) => d.ref)).toEqual(["a", "c", "/b.md"]);
  });
});
