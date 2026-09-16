/**
 * The phone's failed-turn card.
 *
 * The classification is `shared-ts/syntheticError`, shared with the desktop and
 * tested there; what is phone-specific is the transport. So the buttons' actual
 * effects are tested through `apiErrorActions` (a plain function — this suite
 * has no DOM), and the card itself is checked as markup: which buttons it draws,
 * and that it draws none when there is nothing behind them.
 */
import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it, vi } from "vitest";

import { classifySyntheticError } from "../../../shared-ts/syntheticError";
import { ApiErrorCard } from "./ApiErrorCard";
import { runApiErrorAction, resumeOverRelay, type ActionEnv } from "./apiErrorActions";

const SESSION = { id: "s-1", workspacePath: "/repo", agentSource: "claude-code" };

const info = (error: string, text: string, extra?: Record<string, unknown>) =>
  classifySyntheticError({
    type: "assistant",
    error,
    isApiErrorMessage: true,
    ...extra,
    message: { model: "<synthetic>", content: [{ type: "text", text }] },
  })!;

const AUTH = info("authentication_failed", "Failed to authenticate: OAuth session expired");
const SERVER = info("server_error", "API Error: 529 Overloaded");

function env(request: ReturnType<typeof vi.fn>, i = SERVER): ActionEnv {
  return {
    info: i,
    session: SESSION,
    client: { request } as unknown as ActionEnv["client"],
    newKey: () => "key-1",
    openUrl: vi.fn(),
  };
}

describe("apiErrorActions", () => {
  it("retries by resuming this session, with a key and without a prompt", async () => {
    const request = vi.fn(async (_method: string, _params: Record<string, unknown>) => ({}));
    expect(await runApiErrorAction("retry", env(request))).toEqual({ kind: "resumed" });

    const [method, params] = request.mock.calls[0];
    expect(method).toBe("resume_session");
    expect(params).toMatchObject({
      sessionId: "s-1",
      workspacePath: "/repo",
      agentSource: "claude-code",
      // Relay delivery is best-effort; a lost receipt can replay the request, and
      // this key is how the desktop tells a replay from a second resume.
      idempotencyKey: "key-1",
    });
    // A prompt here would silently replace the interrupted work with a new
    // instruction instead of resuming it.
    expect(params).not.toHaveProperty("prompt");
  });

  it("sends /compact for an over-long context", async () => {
    const request = vi.fn(async (_method: string, _params: Record<string, unknown>) => ({}));
    await runApiErrorAction("compact", env(request, info("invalid_request", "Prompt is too long")));
    expect(request.mock.calls[0][1]).toMatchObject({ prompt: "/compact" });
  });

  it("resumes on the picked model when the quota belonged to the model", async () => {
    const request = vi.fn(async (_method: string, _params: Record<string, unknown>) => ({}));
    await resumeOverRelay(env(request), { model: "claude-sonnet-5" });
    expect(request.mock.calls[0][1]).toMatchObject({ model: "claude-sonnet-5" });
  });

  it("starts the OAuth handshake in a pty on the host", async () => {
    const request = vi.fn(async (_method: string, _params: Record<string, unknown>) => ({}));
    expect(await runApiErrorAction("login", env(request, AUTH))).toEqual({ kind: "loginStarted" });
    expect(request.mock.calls[0][0]).toBe("proc_run");
    expect(request.mock.calls[0][1]).toMatchObject({
      command: "claude auth login",
      workspacePath: "/repo",
    });
  });

  it("returns the failure instead of swallowing it", async () => {
    const request = vi.fn(async (_method: string, _params: Record<string, unknown>) => {
      throw new Error("workspace gone");
    });
    // The resume never started a process, so the card is the last surface that
    // can say why.
    expect(await runApiErrorAction("retry", env(request))).toMatchObject({
      kind: "failed",
      detail: expect.stringContaining("workspace gone"),
    });
  });
});

describe("ApiErrorCard (mobile)", () => {
  const client = { request: vi.fn(async () => ({})) } as unknown as Parameters<
    typeof ApiErrorCard
  >[0]["client"];

  it("keeps Claude Code's wording and offers login before retry", () => {
    const html = renderToStaticMarkup(
      <ApiErrorCard info={AUTH} session={SESSION} client={client} />,
    );
    expect(html).toContain("OAuth session expired");
    expect(html.indexOf('data-action="login"')).toBeLessThan(html.indexOf('data-action="retry"'));
  });

  it("draws no buttons when there is no transport or no session behind them", () => {
    const noClient = renderToStaticMarkup(
      <ApiErrorCard info={AUTH} session={SESSION} client={null} />,
    );
    const noSession = renderToStaticMarkup(
      <ApiErrorCard info={AUTH} session={null} client={client} />,
    );
    expect(noClient).not.toContain("data-action");
    expect(noSession).not.toContain("data-action");
    // The classification is still worth showing.
    expect(noClient).toContain("OAuth session expired");
  });

  it("dresses a rate limit as a wait with a countdown, not as a failure", () => {
    const limited = info(
      "rate_limit",
      "You've hit your session limit · resets 3:50am (America/Los_Angeles)",
      { quotaLimits: { resetsAt: Math.floor(Date.now() / 1000) + 125 } },
    );
    const html = renderToStaticMarkup(
      <ApiErrorCard info={limited} session={SESSION} client={client} />,
    );
    expect(html).toContain('data-severity="wait"');
    expect(html).toMatch(/2:0\d/);
  });

  it("offers nothing for safeguards, where no retry would be honest", () => {
    const html = renderToStaticMarkup(
      <ApiErrorCard
        info={info("safeguards", "API Error: safeguards flagged this message")}
        session={SESSION}
        client={client}
      />,
    );
    expect(html).not.toContain("data-action");
  });
});
