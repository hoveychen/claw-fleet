/**
 * What each button on the failed-turn card actually does, as a plain function.
 *
 * Split out of the card so it can be tested without a DOM — mobile-web's suite
 * renders through `renderToStaticMarkup` and has no jsdom, and "which relay
 * method did that button call, with which params" is precisely the part worth
 * pinning: a retry that resumes with a prompt silently replaces the interrupted
 * work, and a resume without an idempotency key can be delivered twice.
 */
import type { ErrorAction, SyntheticErrorInfo } from "../../../shared-ts/syntheticError";
import type { FleetTransport } from "../relay";

export interface ApiErrorSession {
  id: string;
  workspacePath: string;
  agentSource?: string | null;
}

/** What the card should say afterwards. `null` = nothing to report (the action
 *  opened a picker, or a tab). */
export type ActionOutcome =
  | { kind: "resumed" }
  | { kind: "loginStarted" }
  | { kind: "failed"; detail: string }
  | null;

export interface ActionEnv {
  info: SyntheticErrorInfo;
  session: ApiErrorSession;
  client: FleetTransport;
  /** Injected so the test does not need a browser. */
  openUrl?: (url: string) => void;
  /** Injected for the same reason; the real one is `crypto`-ish randomness. */
  newKey?: () => string;
}

function defaultKey(): string {
  return Math.random().toString(36).slice(2) + Date.now().toString(36);
}

/** Resume this session over the relay, optionally with a prompt or a different
 *  model. */
export async function resumeOverRelay(
  env: ActionEnv,
  extra: { prompt?: string; model?: string } = {},
): Promise<ActionOutcome> {
  const { session, client } = env;
  try {
    await client.request("resume_session", {
      sessionId: session.id,
      workspacePath: session.workspacePath,
      agentSource: session.agentSource ?? "",
      // Relay delivery is best-effort: if the receipt is lost the request can be
      // replayed (or reach a second agent on the same machine). The desktop uses
      // this key to recognise the replay instead of starting a second
      // `claude --resume` against one transcript.
      idempotencyKey: (env.newKey ?? defaultKey)(),
      ...extra,
    });
    return { kind: "resumed" };
  } catch (e) {
    return { kind: "failed", detail: String(e) };
  }
}

/**
 * Run one of the card's actions.
 *
 * `switchModel` is absent on purpose: it opens a picker rather than doing
 * anything, so it stays with the component's state.
 */
export async function runApiErrorAction(
  action: Exclude<ErrorAction, "switchModel">,
  env: ActionEnv,
): Promise<ActionOutcome> {
  switch (action) {
    case "retry":
      // No prompt — the agent picks up the turn the failure cut off. A prompt
      // here would quietly replace that work with a new instruction.
      return resumeOverRelay(env);
    case "compact":
      return resumeOverRelay(env, { prompt: "/compact" });
    case "openUrl":
      if (env.info.url) (env.openUrl ?? browserOpen)(env.info.url);
      return null;
    case "login":
      try {
        // The OAuth handshake prints a URL and waits for a pasted code, so it
        // needs a pty on the host — the same proc runner the "终端" (Terminal) page
        // drives. The card starts it; the typing happens over there.
        await env.client.request("proc_run", {
          workspacePath: env.session.workspacePath,
          command: "claude auth login",
          cols: 80,
          rows: 24,
        });
        return { kind: "loginStarted" };
      } catch (e) {
        return { kind: "failed", detail: String(e) };
      }
  }
}

function browserOpen(url: string): void {
  window.open(url, "_blank", "noopener");
}
