/**
 * `shared-ts/syntheticError.ts` — Claude Code's failed-turn records, classified.
 *
 * Every fixture below is a REAL record, copied verbatim out of the transcripts
 * under `~/.claude/projects` (4984 files scanned on 2026-09-16), with only the
 * bookkeeping fields the classifier never reads trimmed away. That matters more
 * than usual here: the whole module is a reverse-engineering of a schema nobody
 * publishes, so a fixture I invented would only prove that my guess agrees with
 * my guess. The two `rate_limit` shapes in particular — one with `quotaLimits`,
 * one without — are the kind of asymmetry a hand-written fixture smooths over.
 */
import { describe, expect, it } from "vitest";

import {
  classifySyntheticError,
  quotaResetsInMs,
  type SyntheticErrorSource,
} from "../../shared-ts/syntheticError";

/** The auth failure from the user's screenshot, and the reason this module exists. */
const AUTH: SyntheticErrorSource = {
  type: "assistant",
  error: "authentication_failed",
  isApiErrorMessage: true,
  message: {
    model: "<synthetic>",
    content: [
      { type: "text", text: "Failed to authenticate: OAuth session expired and could not be refreshed" },
    ],
  },
};

const SERVER: SyntheticErrorSource = {
  type: "assistant",
  error: "server_error",
  isApiErrorMessage: true,
  message: {
    model: "<synthetic>",
    content: [
      { type: "text", text: "API Error: Can't reach the API server — check your internet or DNS (ENOTFOUND)" },
    ],
  },
};

/** A window limit: prose reset string AND the structured quota block. */
const RATE_WINDOW: SyntheticErrorSource = {
  type: "assistant",
  error: "rate_limit",
  isApiErrorMessage: true,
  quotaLimits: {
    status: "rejected",
    resetsAt: 1788691800,
    unifiedRateLimitFallbackAvailable: false,
    rateLimitType: "five_hour",
    overageStatus: "rejected",
    overageDisabledReason: "org_level_disabled",
    isUsingOverage: false,
  },
  message: {
    model: "<synthetic>",
    content: [{ type: "text", text: "You've hit your session limit · resets 3:50am (America/Los_Angeles)" }],
  },
};

/** A model limit: same `error` code, no `quotaLimits`, nothing to wait for. */
const RATE_MODEL: SyntheticErrorSource = {
  type: "assistant",
  error: "rate_limit",
  isApiErrorMessage: true,
  message: {
    model: "<synthetic>",
    content: [
      {
        type: "text",
        text: "You've reached your Fable limit. Switch to another model, or manage usage credits at claude.ai/settings/usage?from=cc_cli_limit_message, to continue.",
      },
    ],
  },
};

const COMPACTION_BLOCKED: SyntheticErrorSource = {
  type: "assistant",
  error: "invalid_request",
  isApiErrorMessage: true,
  message: {
    model: "<synthetic>",
    content: [
      {
        type: "text",
        text: "Prompt is too long · automatic compaction failed: You've hit your session limit · resets 3:50am (America/Los_Angeles)",
      },
    ],
  },
};

/** Synthetic, but not an error — 1482 of the 2804 synthetic records here. */
const FILLER: SyntheticErrorSource = {
  type: "assistant",
  error: undefined,
  isApiErrorMessage: false,
  message: { model: "<synthetic>", content: [{ type: "text", text: "No response requested." }] },
};

describe("classifySyntheticError", () => {
  it("routes an expired OAuth session to login, not to a bare retry", () => {
    const info = classifySyntheticError(AUTH);
    expect(info?.code).toBe("authentication_failed");
    expect(info?.severity).toBe("fatal");
    // login first: retrying with dead credentials fails the same way.
    expect(info?.actions).toEqual(["login", "retry"]);
    expect(info?.text).toContain("OAuth session expired");
  });

  it("treats a transport failure as transient and retryable", () => {
    const info = classifySyntheticError(SERVER);
    expect(info?.severity).toBe("transient");
    expect(info?.actions[0]).toBe("retry");
    expect(info?.url).toBe("https://status.claude.com");
  });

  it("splits the two rate_limit shapes Claude Code gives the same code", () => {
    const win = classifySyntheticError(RATE_WINDOW);
    const model = classifySyntheticError(RATE_MODEL);
    expect(win?.code).toBe("rate_limit");
    expect(model?.code).toBe("rate_limit");

    // A window limit reopens on its own; a model limit never does, so only the
    // window shape carries the countdown payload and only the model shape
    // leads with the switch.
    expect(win?.quota?.resetsAt).toBe(1788691800);
    expect(win?.titleKey).toBe("detail.api_error.rate_limit");
    // Even the window shape leads with the switch: until the reset passes, a
    // retry is the one action guaranteed to fail.
    expect(win?.actions).toEqual(["switchModel", "retry"]);
    expect(model?.quota).toBeUndefined();
    expect(model?.actions[0]).toBe("switchModel");
    expect(model?.titleKey).toBe("detail.api_error.model_quota");
  });

  it("does not offer compaction when it was compaction itself that got rate-limited", () => {
    const info = classifySyntheticError(COMPACTION_BLOCKED);
    // Code says invalid_request, but the blocker is quota — compaction is a
    // model call too, so the compact button would be a dead end.
    expect(info?.actions).not.toContain("compact");
    expect(info?.severity).toBe("wait");
    expect(info?.titleKey).toBe("detail.api_error.compaction_blocked");
  });

  it("still offers compaction for an ordinary over-long prompt", () => {
    const info = classifySyntheticError({
      ...COMPACTION_BLOCKED,
      message: {
        model: "<synthetic>",
        content: [{ type: "text", text: "Prompt is too long" }],
      },
    });
    expect(info?.actions).toEqual(["compact", "retry"]);
    expect(info?.severity).toBe("fatal");
  });

  it("refuses to promise a retry that re-trips the same classifier", () => {
    const info = classifySyntheticError({
      type: "assistant",
      error: "safeguards",
      isApiErrorMessage: true,
      message: {
        model: "<synthetic>",
        content: [{ type: "text", text: "API Error: Opus 5's safeguards flagged this message" }],
      },
    });
    expect(info?.actions).toEqual([]);
    expect(info?.severity).toBe("fatal");
  });

  it("cards an enum member it has never seen rather than falling back to prose", () => {
    // The regression this guards: Claude Code ships a new `error` value, the
    // switch has no arm for it, and the row silently reverts to a grey bubble.
    const info = classifySyntheticError({
      type: "assistant",
      error: "some_future_code",
      isApiErrorMessage: true,
      message: { model: "<synthetic>", content: [{ type: "text", text: "API Error: ???" }] },
    });
    expect(info).not.toBeNull();
    expect(info?.code).toBe("unknown");
    expect(info?.actions).toEqual(["retry"]);
  });

  it("ignores everything that is not a synthetic API error", () => {
    expect(classifySyntheticError(FILLER)).toBeNull();
    expect(classifySyntheticError(null)).toBeNull();
    // A real assistant turn, even one whose prose happens to look like an error.
    expect(
      classifySyntheticError({
        type: "assistant",
        message: { model: "claude-opus-5", content: [{ type: "text", text: "Failed to authenticate" }] },
      }),
    ).toBeNull();
    // A user turn tagged as an API error is not a failed model turn.
    expect(classifySyntheticError({ ...AUTH, type: "user" })).toBeNull();
  });

  it("reads content whether it arrives as blocks or as a bare string", () => {
    const info = classifySyntheticError({
      ...AUTH,
      message: { model: "<synthetic>", content: "Not logged in · Please run /login" },
    });
    expect(info?.text).toBe("Not logged in · Please run /login");
  });
});

describe("quotaResetsInMs", () => {
  const at = 1788691800;

  it("counts down from the structured resetsAt", () => {
    const now = (at - 90) * 1000;
    expect(quotaResetsInMs({ resetsAt: at }, now)).toBe(90_000);
  });

  it("clamps a reset that already passed instead of counting up", () => {
    expect(quotaResetsInMs({ resetsAt: at }, (at + 600) * 1000)).toBe(0);
  });

  it("returns null when the record carried no usable timestamp", () => {
    // The model-quota shape has no quotaLimits at all — the card must fall back
    // to its prose rather than render a countdown to the epoch.
    expect(quotaResetsInMs(undefined, Date.now())).toBeNull();
    expect(quotaResetsInMs({ status: "rejected" }, Date.now())).toBeNull();
    expect(quotaResetsInMs({ resetsAt: 0 }, Date.now())).toBeNull();
  });
});
