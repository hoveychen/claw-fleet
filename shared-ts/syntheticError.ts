/** Claude Code's synthetic API-error turns, classified into something a UI can
 *  act on.
 *
 *  When a turn fails, Claude Code does not throw — it appends an assistant
 *  record whose `message.model` is the literal `"<synthetic>"`, tags it
 *  `isApiErrorMessage: true`, and stamps a machine-readable `error` enum on the
 *  record. Both clients get these records verbatim (`get_messages` passes
 *  `serde_json::Value` straight through; the relay snapshot carries the same
 *  lines), so the classification is a pure front-end concern — no core change
 *  is involved and none should be: the taxonomy is Claude Code's, not Fleet's.
 *
 *  Before this module both clients rendered these as a bare assistant bubble.
 *  "Failed to authenticate: OAuth session expired and could not be refreshed"
 *  arrived as one line of grey prose with no hint that the fix is 20 seconds of
 *  `claude auth login` — which is the whole reason the module exists: the
 *  record already knows what went wrong, so the UI can offer the way out.
 *
 *  ## Where the taxonomy comes from
 *
 *  Two sources, cross-checked on 2026-09-16:
 *
 *  1. Every `error` string the `claude` 2.1.263 binary can emit.
 *  2. A scan of all 4984 transcripts on the author's machine — 2804 synthetic
 *     records. Observed: `rate_limit` (1097), no-error filler (1482),
 *     `server_error` (130), `authentication_failed` (87), `invalid_request`
 *     (6). The other nine enum members exist in the binary but had not fired
 *     here, so they are mapped from the binary alone and must degrade
 *     gracefully rather than be assumed rare forever.
 *
 *  An unrecognised `error` value is deliberately NOT dropped: it maps to
 *  `unknown`, which still renders as a failure card (with no action) rather
 *  than falling back to an assistant bubble. Claude Code adds enum members
 *  without telling us, and a new one is exactly the case where a plain-prose
 *  regression would go unnoticed.
 */

/** The `error` enum Claude Code stamps on a failed turn. `null` on synthetic
 *  records that are not errors at all (the "No response requested." filler). */
export type ApiErrorCode =
  | "rate_limit"
  | "server_error"
  | "authentication_failed"
  | "invalid_request"
  | "billing_error"
  | "overloaded"
  | "safeguards"
  | "model_not_found"
  | "request_too_large"
  | "permission_error"
  | "oauth_org_not_allowed"
  | "not_found_error"
  | "tool_use_error"
  | "api_error";

/** What the user can do about it — the card renders one button per action.
 *
 *  Kept deliberately small: every member has to be implemented by BOTH clients
 *  (the desktop over Tauri, the phone/webui over the relay), and an action only
 *  one of them can honour would render as a dead button on the other. */
export type ErrorAction =
  /** Re-run the interrupted turn on this same session. */
  | "retry"
  /** Re-authenticate (`claude auth login`), then retry. */
  | "login"
  /** Pick a different model and continue — the quota that ran out is per-model. */
  | "switchModel"
  /** Compact the context, then continue. */
  | "compact"
  /** Open an external page (billing / usage / status). `url` says which. */
  | "openUrl";

/** How loudly the card presents itself. `wait` is its own tier because a rate
 *  limit is not a failure the user caused or can fix — it is a countdown, and
 *  Fleet usually resumes it unattended. Painting it the same red as a revoked
 *  token trained the eye to ignore both. */
export type ErrorSeverity = "fatal" | "transient" | "wait";

export interface SyntheticErrorInfo {
  /** The raw enum off the record, or `"unknown"` when Claude Code sent one we
   *  have never seen. Never `null` — non-error synthetics don't reach here. */
  code: ApiErrorCode | "unknown";
  severity: ErrorSeverity;
  /** i18n key suffix for the card's headline, e.g. `detail.api_error.auth`. */
  titleKey: string;
  /** Actions in the order they should be rendered; first is the primary. */
  actions: ErrorAction[];
  /** Target for an `openUrl` action, when one is present. */
  url?: string;
  /** Structured quota payload, when the record carried one. */
  quota?: QuotaLimits;
  /** The message text as Claude Code wrote it, kept verbatim for the fold. */
  text: string;
}

/** The structured quota block Claude Code attaches to `rate_limit` records.
 *
 *  Worth preferring over the prose: the prose renders the reset as a local
 *  wall-clock string ("4:10am (America/Los_Angeles)") that `rate_limit_parser`
 *  has to reverse-engineer back into an instant, while `resetsAt` is already a
 *  Unix timestamp sitting on the same record. `overageStatus` / `isUsingOverage`
 *  have no prose equivalent at all. */
export interface QuotaLimits {
  status?: string;
  /** Unix seconds. */
  resetsAt?: number;
  rateLimitType?: string;
  overageStatus?: string;
  overageDisabledReason?: string;
  isUsingOverage?: boolean;
  unifiedRateLimitFallbackAvailable?: boolean;
}

/** The minimum shape this module reads. Both clients' `RawMessage` satisfy it;
 *  typing it structurally keeps shared-ts from depending on either. */
export interface SyntheticErrorSource {
  type?: string;
  error?: unknown;
  isApiErrorMessage?: unknown;
  quotaLimits?: unknown;
  message?: {
    model?: string;
    content?: unknown;
  };
}

const KNOWN_CODES = new Set<string>([
  "rate_limit",
  "server_error",
  "authentication_failed",
  "invalid_request",
  "billing_error",
  "overloaded",
  "safeguards",
  "model_not_found",
  "request_too_large",
  "permission_error",
  "oauth_org_not_allowed",
  "not_found_error",
  "tool_use_error",
  "api_error",
]);

const USAGE_URL = "https://claude.ai/settings/usage";
const STATUS_URL = "https://status.claude.com";

/** Flatten the record's content to text, tolerating both the string and the
 *  block-array shape (Claude Code uses the array; the relay has been seen to
 *  hand back the string). */
function contentText(src: SyntheticErrorSource): string {
  const content = src.message?.content;
  if (typeof content === "string") return content.trim();
  if (!Array.isArray(content)) return "";
  const parts: string[] = [];
  for (const b of content) {
    if (b && typeof b === "object" && (b as { type?: string }).type === "text") {
      const t = (b as { text?: unknown }).text;
      if (typeof t === "string") parts.push(t);
    }
  }
  return parts.join("").trim();
}

function quotaOf(src: SyntheticErrorSource): QuotaLimits | undefined {
  const q = src.quotaLimits;
  if (!q || typeof q !== "object" || Array.isArray(q)) return undefined;
  return q as QuotaLimits;
}

/**
 * A rate limit comes in two shapes that want opposite responses, and Claude
 * Code gives them the same `error` code.
 *
 * - A *window* limit ("You've hit your session limit · resets 4:10am") is a
 *   countdown: the only move is to wait, and Fleet's auto-resume already does.
 * - A *model* limit ("You've reached your Fable limit. Switch to another model
 *   to continue.") has no useful reset to wait for — the sibling models are
 *   available right now. Offering "wait" there strands a session for hours in
 *   front of a fix that is one dropdown away.
 *
 * The tell is the verb: window limits open "You've hit your", model limits open
 * "You've reached your". Confirmed against all 1097 rate_limit records on the
 * author's machine — the two prefixes never crossed over.
 */
function isModelQuota(text: string): boolean {
  return /^You've reached your\b/.test(text);
}

/** Classify a transcript record, or return `null` when it is not a synthetic
 *  API error (an ordinary turn, or the "No response requested." filler, which
 *  is synthetic but carries `isApiErrorMessage: false`). */
export function classifySyntheticError(
  src: SyntheticErrorSource | null | undefined,
): SyntheticErrorInfo | null {
  if (!src || src.type !== "assistant") return null;
  if (src.message?.model !== "<synthetic>") return null;
  if (src.isApiErrorMessage !== true) return null;

  const raw = typeof src.error === "string" ? src.error : "";
  const code = (KNOWN_CODES.has(raw) ? raw : "unknown") as ApiErrorCode | "unknown";
  const text = contentText(src);
  const quota = quotaOf(src);

  const base = { code, text, quota } as const;

  switch (code) {
    case "rate_limit":
      // Both shapes can offer a model switch; only the window shape is a wait.
      return isModelQuota(text)
        ? {
            ...base,
            severity: "wait",
            titleKey: "detail.api_error.model_quota",
            actions: ["switchModel", "openUrl"],
            url: USAGE_URL,
          }
        : {
            ...base,
            severity: "wait",
            titleKey: "detail.api_error.rate_limit",
            // Switch leads, retry follows. A window limit has a reset time, and
            // until it passes a retry is the one action guaranteed to fail —
            // it was the primary button until a screenshot showed it sitting
            // above its own "resets in 1:31:52" badge. The sibling models are
            // available now; the UI additionally holds the retry button shut
            // while the countdown runs.
            actions: ["switchModel", "retry"],
          };

    case "authentication_failed":
    case "oauth_org_not_allowed":
      return {
        ...base,
        severity: "fatal",
        titleKey: "detail.api_error.auth",
        actions: ["login", "retry"],
      };

    case "server_error":
    case "overloaded":
    case "api_error":
      return {
        ...base,
        severity: "transient",
        titleKey: "detail.api_error.server",
        actions: ["retry", "openUrl"],
        url: STATUS_URL,
      };

    case "invalid_request":
    case "request_too_large":
      // "Prompt is too long · automatic compaction failed: You've hit your
      // session limit"—two of the six such records on the author's machine
      // read like this, and they are NOT a context problem: compaction is
      // itself a model call, so a quota that blocks the turn blocks the escape
      // hatch too. Offering "Compress and Continue" there sends the user at a
      // button that cannot work until the window reopens.
      if (/automatic compaction failed:\s*You've (?:hit|reached) your/.test(text)) {
        return {
          ...base,
          severity: "wait",
          titleKey: "detail.api_error.compaction_blocked",
          actions: ["switchModel", "retry"],
        };
      }
      // The ordinary shape: a plain retry re-sends the same oversized context
      // and fails identically, so compact leads and retry is the second chance.
      return {
        ...base,
        severity: "fatal",
        titleKey: "detail.api_error.too_long",
        actions: ["compact", "retry"],
      };

    case "billing_error":
      return {
        ...base,
        severity: "fatal",
        titleKey: "detail.api_error.billing",
        actions: ["openUrl", "switchModel"],
        url: USAGE_URL,
      };

    case "model_not_found":
      return {
        ...base,
        severity: "fatal",
        titleKey: "detail.api_error.model_not_found",
        actions: ["switchModel"],
      };

    case "safeguards":
      // Deliberately actionless. A retry re-sends the same prompt into the same
      // classifier; a button promising otherwise would be a lie, and the model
      // switch the prose itself suggests is a decision about *what to ask*, not
      // one the card can make.
      return {
        ...base,
        severity: "fatal",
        titleKey: "detail.api_error.safeguards",
        actions: [],
      };

    case "permission_error":
    case "not_found_error":
    case "tool_use_error":
      return {
        ...base,
        severity: "fatal",
        titleKey: "detail.api_error.generic",
        actions: ["retry"],
      };

    default:
      // An enum member Claude Code added after this table was written. Still a
      // card — retry is the one move that is safe to offer without knowing why.
      return {
        ...base,
        severity: "transient",
        titleKey: "detail.api_error.generic",
        actions: ["retry"],
      };
  }
}

/** Milliseconds until the quota resets, or `null` when the record carried no
 *  usable `resetsAt`. Negative values clamp to 0 — a reset in the past means
 *  the window already reopened and the card should stop counting. */
export function quotaResetsInMs(quota: QuotaLimits | undefined, now: number): number | null {
  const at = quota?.resetsAt;
  if (typeof at !== "number" || !Number.isFinite(at) || at <= 0) return null;
  return Math.max(0, at * 1000 - now);
}
