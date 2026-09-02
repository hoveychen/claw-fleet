/**
 * Turn a backend failure into a sentence a person can act on.
 *
 * Two separate problems, both of which used to reach the user raw:
 *
 * 1. **The envelope.** A probe error arrives as
 *    `Error: HTTP 500: {"error":"…"}`. The part written for a human is inside;
 *    the status code and JSON around it bury it.
 * 2. **The language.** rca launch failures are produced in Rust, in English,
 *    for a reader who is assumed to know what `rcaPath` is. The ones a user can
 *    actually hit while just starting a session now carry a stable
 *    `rca:<code>` prefix (`claw_fleet_core::remote_workspace::codes`) so the UI
 *    can say the same thing in the user's language.
 *
 * Anything unrecognised is passed through unchanged rather than guessed at — a
 * wrong translation of an error is worse than an untranslated one.
 */

/** Mirrors `claw_fleet_core::remote_workspace::codes`. */
export const RCA_ERROR_CODES = {
  "rca:no-local-rca": "rca_error.no_local_rca",
  "rca:bad-rca-override": "rca_error.bad_rca_override",
  "rca:host-gone": "rca_error.host_gone",
  "rca:no-transport": "rca_error.no_transport",
} as const;

/** Strip the `Error: HTTP <n>: {"error":"…"}` envelope a probe error arrives in. */
export function unwrapBackendError(e: unknown): string {
  const raw = String(e);
  const brace = raw.indexOf("{");
  if (brace >= 0) {
    try {
      const parsed = JSON.parse(raw.slice(brace)) as { error?: unknown };
      if (typeof parsed.error === "string" && parsed.error) return parsed.error;
    } catch {
      // Not JSON after all — fall through to the raw string.
    }
  }
  return raw.replace(/^Error:\s*/, "");
}

/** The `rca:<code>` an error carries, if any, plus the prose after it. */
export function parseRcaError(
  e: unknown,
): { key: string; detail: string } | null {
  const msg = unwrapBackendError(e);
  for (const [code, key] of Object.entries(RCA_ERROR_CODES)) {
    const at = msg.indexOf(`${code}:`);
    if (at >= 0) {
      return { key, detail: msg.slice(at + code.length + 1).trim() };
    }
  }
  return null;
}

/**
 * The message to show. `t` is the i18n lookup; when the error carries no known
 * code the un-enveloped original is returned, which is still better than the
 * raw string and never invents meaning.
 */
export function rcaErrorMessage(
  e: unknown,
  t: (key: string, opts?: Record<string, unknown>) => string,
): string {
  const parsed = parseRcaError(e);
  return parsed ? t(parsed.key) : unwrapBackendError(e);
}
