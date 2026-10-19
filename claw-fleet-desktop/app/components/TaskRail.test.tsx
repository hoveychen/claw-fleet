// @vitest-environment jsdom
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, describe, expect, it, vi } from "vitest";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn(() => new Promise(() => {})) }));

import "../i18n";
import type { SessionInfo } from "../types";
import type { Artifact } from "./ArtifactsView";
import type { TaskRow } from "./homeRows";
import { collectReviewDocs, groupSubtasksByStep, stepIdOf, TaskRail, taskRailHasContent } from "./TaskRail";
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
    expect(el.querySelectorAll("section")).toHaveLength(0);
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
    const item = () => el.querySelector<HTMLButtonElement>("button[title='Report']")!;
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

describe("sub-tasks under steps", () => {
  const sub = (line: string, title: string) => {
    const tip = { id: `${line}-tip`, taskId: line, aiTitle: title } as unknown as SessionInfo;
    return { key: `t::${line}`, tip, hops: [tip], state: "running", pending: false };
  };
  const linkedRow = () =>
    row({
      hops: [
        {
          id: "main",
          taskSummary: {
            subtaskSteps: [
              { taskId: "s1", planId: "demo", step: "P2" },
              { taskId: "s3", planId: "other", step: "P1" },
            ],
          },
        } as unknown as SessionInfo,
      ],
      subtasks: [sub("s1", "Build slice"), sub("s2", "Side errand"), sub("s3", "Elsewhere")] as never,
    });
  const steps = [
    { text: "**P1** — design", done: true },
    { text: "**P2** — build", done: false },
  ];

  it("reads the step id off a plan item", () => {
    expect(stepIdOf("**P12** — x")).toBe("P12");
    expect(stepIdOf("no id")).toBeNull();
  });

  // Only links into the plan on screen count: s3 names a step of another plan.
  it("groups by step of the shown plan and leaves the rest loose", () => {
    const { byStep, loose } = groupSubtasksByStep(linkedRow(), "demo");
    expect(byStep.get("P2")?.map((s) => s.tip.taskId)).toEqual(["s1"]);
    expect(loose.map((s) => s.tip.taskId)).toEqual(["s2", "s3"]);
  });

  it("renders a linked sub-task under its step and the rest as other sub-tasks", () => {
    const el = render({ row: linkedRow(), steps, stepsPlanId: "demo" });
    const nested = el.querySelectorAll("[data-testid='step-subtasks']");
    expect(nested).toHaveLength(1);
    expect(nested[0].closest("li")?.textContent).toContain("build");
    expect(nested[0].textContent).toContain("Build slice");
    const sections = [...el.querySelectorAll("section")];
    // Steps lead once something hangs under them.
    expect(sections[0].textContent).toContain("Build slice");
    expect(sections[1].textContent).toContain("Side errand");
    expect(sections[1].textContent).not.toContain("Build slice");
  });

  it("keeps the plain layout when nothing links", () => {
    const el = render({ row: linkedRow(), steps, stepsPlanId: "unrelated" });
    expect(el.querySelectorAll("[data-testid='step-subtasks']")).toHaveLength(0);
    expect(el.querySelector("section")?.textContent).toContain("Build slice");
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
