// @vitest-environment jsdom
/**
 * The failed-turn card's buttons, wired.
 *
 * The card itself is tested next door; what is pinned here is the part that can
 * quietly lie to the user: which command each button actually fires, and which
 * buttons survive for a session that cannot be resumed at all. A "retry" that
 * resumes the wrong session, or one drawn on a subagent transcript where resume
 * is impossible, both look identical on screen to one that works.
 */
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const invoke = vi.fn();
// Indirected through a closure: `vi.mock` is hoisted above the `const`, so a
// bare `{ invoke }` factory would read the binding before it initialises.
vi.mock("@tauri-apps/api/core", () => ({
  invoke: (...a: unknown[]) => invoke(...(a as [])),
}));
// Stands in for i18next including its `{{var}}` interpolation: the backend's
// refusal reaches the screen through it, and a stub that returned the template
// would hide whether the message was ever carried at all.
vi.mock("react-i18next", () => ({
  useTranslation: () => ({
    t: (k: string, arg?: unknown) => {
      if (typeof arg === "string") return arg;
      const opts = (arg ?? {}) as Record<string, unknown> & { defaultValue?: string };
      return (opts.defaultValue ?? k).replace(/\{\{(\w+)\}\}/g, (m, n) =>
        n in opts ? String(opts[n]) : m,
      );
    },
  }),
}));
// The catalogue arrives over IPC; the picker only needs it to be non-empty.
vi.mock("../../useModelCatalog", () => ({
  useModelCatalog: () => [
    { name: "claude", models: [{ id: "claude-sonnet-5", label: "Sonnet 5", efforts: [] }] },
  ],
}));
// A real terminal wants a pty, a ResizeObserver and xterm; the card only needs
// to know it mounted one.
vi.mock("../ProcTerminal", () => ({
  ProcTerminal: () => <div data-testid="proc-terminal" />,
}));

import { classifySyntheticError } from "../../../../shared-ts/syntheticError";
import { ApiErrorActions, type ApiErrorContext } from "./ApiErrorActions";

(globalThis as unknown as { IS_REACT_ACT_ENVIRONMENT: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

const CTX: ApiErrorContext = {
  sessionId: "sess-1",
  workspacePath: "/repo",
  agentSource: "claude-code",
};

const info = (error: string, text: string) =>
  classifySyntheticError({
    type: "assistant",
    error,
    isApiErrorMessage: true,
    message: { model: "<synthetic>", content: [{ type: "text", text }] },
  })!;

const AUTH = info("authentication_failed", "Failed to authenticate: OAuth session expired");
const SERVER = info("server_error", "API Error: 529 Overloaded");
const TOO_LONG = info("invalid_request", "Prompt is too long");

let container: HTMLDivElement | null = null;
let root: Root | null = null;

beforeEach(() => {
  invoke.mockReset();
  invoke.mockResolvedValue(undefined);
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

function draw(props: Parameters<typeof ApiErrorActions>[0]) {
  act(() => root!.render(<ApiErrorActions {...props} />));
  return container!;
}

async function click(el: HTMLElement, sel: string) {
  await act(async () => {
    el.querySelector<HTMLButtonElement>(sel)!.click();
  });
}

const actionsOf = (el: HTMLElement) =>
  [...el.querySelectorAll("[data-action]")].map((b) => b.getAttribute("data-action"));

describe("ApiErrorActions", () => {
  it("retries by resuming THIS session, with no prompt", async () => {
    const el = draw({ info: SERVER, ctx: CTX });
    await click(el, '[data-action="retry"]');
    expect(invoke).toHaveBeenCalledWith("resume_rate_limited_session", {
      sessionId: "sess-1",
      workspacePath: "/repo",
      agentSource: "claude-code",
    });
    // No prompt: the agent picks up the turn the failure cut off. A prompt here
    // would silently replace the interrupted work with a new instruction.
    expect(invoke.mock.calls[0][1]).not.toHaveProperty("prompt");
  });

  it("sends /compact for an over-long context", async () => {
    const el = draw({ info: TOO_LONG, ctx: CTX });
    await click(el, '[data-action="compact"]');
    expect(invoke.mock.calls[0][1]).toMatchObject({ prompt: "/compact" });
  });

  it("opens a pty for the OAuth handshake rather than a fire-and-forget spawn", async () => {
    invoke.mockResolvedValue({ id: "proc-1", status: "running" });
    const el = draw({ info: AUTH, ctx: CTX });
    await click(el, '[data-action="login"]');
    expect(invoke).toHaveBeenCalledWith("run_workspace_proc", {
      workspacePath: "/repo",
      command: "claude auth login",
      cols: 80,
      rows: 24,
    });
    // `claude auth login` prints a URL and waits for a pasted code, so the shell
    // has to be on screen and typeable.
    expect(el.querySelector("[data-testid=proc-terminal]")).toBeTruthy();
  });

  it("resumes on the chosen model when the quota was the model's", async () => {
    const quota = info("rate_limit", "You've reached your Fable limit. Switch to another model.");
    const el = draw({ info: quota, ctx: CTX });
    await click(el, '[data-action="switchModel"]');
    expect(el.querySelector("[data-testid=api-error-model-picker]")).toBeTruthy();
    await click(el, '[data-model="claude-sonnet-5"]');
    expect(invoke).toHaveBeenCalledWith("resume_rate_limited_session", {
      sessionId: "sess-1",
      workspacePath: "/repo",
      agentSource: "claude-code",
      model: "claude-sonnet-5",
    });
  });

  it("surfaces a refused resume, which otherwise has no other surface", async () => {
    invoke.mockRejectedValue("Workspace directory not found: /repo");
    const el = draw({ info: SERVER, ctx: CTX });
    await click(el, '[data-action="retry"]');
    // The resume never started a process, so nothing downstream will ever say
    // why — the card is the last surface left.
    expect(el.querySelector("[data-testid=api-error-failure]")?.textContent).toContain(
      "Workspace directory not found",
    );
    expect(el.querySelector("[data-testid=api-error-done]")).toBeNull();
  });

  it("hides resume-backed buttons on a session that cannot be resumed", () => {
    // A subagent's `agent-*` transcript has nothing `claude --resume` can take.
    const el = draw({ info: AUTH, ctx: { ...CTX, isSubagent: true } });
    expect(actionsOf(el)).toEqual(["login"]);
    // …and the retry that WOULD have been drawn next to it is simply gone,
    // rather than present and failing on click.
  });

  it("drops every button when there is no session behind the transcript", () => {
    expect(actionsOf(draw({ info: AUTH, ctx: null }))).toEqual([]);
    // The classification is still worth showing.
    expect(container!.textContent).toContain("OAuth session expired");
  });

  it("does not resume for an IDE-attached session", () => {
    // Resuming behind VS Code would put two agents on one transcript.
    const el = draw({ info: SERVER, ctx: { ...CTX, ideName: "vscode" } });
    expect(actionsOf(el)).toEqual(["openUrl"]);
  });
});
