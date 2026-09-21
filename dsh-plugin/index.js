// Fleet's dsh adapter plugin.
//
// dsh has no hook layer. Fleet's per-turn context therefore rides a cordis
// plugin: a prepended `agent/pre-step` listener that appends one
// `plugin`-sourced message to the entering batch, so it reaches the same
// request as the prompt.
//
// Why not prepend the text to the prompt (what Fleet did before): a prompt is a
// `source.kind === 'user'` message, and dsh's session-title provider frames the
// first such message for its title model under a hard 4096-byte input budget
// that rejects rather than truncates. Fleet's TASKS.md block alone runs 4.7-5.3
// KB, so prepending it meant the LLM title never ran and the session was named
// `<system-reminder> The workspace \`TASKS.m`. A `plugin`-sourced message is not
// an eligible title message, so this channel is invisible to that budget.
// Verified live: same server, same model, only the first prompt's size differed.
//
// Content comes from `fleet dsh-context`, not from logic here, so the injected
// text has one renderer shared with the Claude hook and the codex path.
//
// When a section may enter is decided here, and the plan section is
// turn-scoped: it re-enters only on a step that starts with a user prompt (see
// `startsTurn`). The Claude hook that renders the same text fires on
// UserPromptSubmit and nothing else, so a Claude session sees its TASKS.md once
// per prompt. Re-checking it on every step instead made every checkbox anyone
// ticked in the workspace — twenty active plans, several sessions — append a
// fresh 4.5-6.6 KB copy mid-turn: measured 21 copies (~95 KB) inside one turn
// of one session. Appends never break the provider's prefix cache (verified on
// 57 such steps), so the cost was context, not cache; the fix is still to stop
// paying it.

import { execFile } from 'node:child_process'

/** Cordis plugin name used by loader diagnostics. */
export const name = 'fleet-context'

/** The agent registry owns pre-step processing. */
export const inject = ['agents']

/** Default ceiling for one `fleet dsh-context` call, in milliseconds. */
const DEFAULT_TIMEOUT_MS = 5000

/**
 * Sections whose changes wait for the next user prompt instead of entering on
 * the step they are noticed. Everything else (the static guidance, the session
 * id) still enters as soon as it differs — those only change when Fleet itself
 * is updated, so there is nothing to throttle.
 *
 * `fleet-ctx`, the context-pressure reminder, is deliberately absent: the CLI
 * emits it at most once per tier per session, so it cannot repeat the way the
 * plan block did, and a session that crosses a tier mid-turn is precisely the
 * one that should not have to wait for its next prompt to be told.
 */
export const TURN_SCOPED_SECTIONS = new Set(['fleet-prd'])

/**
 * Whether this step opens a turn, i.e. carries a user prompt into the model.
 *
 * dsh numbers steps from 1 within each turn, so `step === 1` is the turn's
 * first request. The batch is checked as well: a message the user typed while
 * the agent was running is spliced into a later step's inbox with
 * `source.kind === 'user'`, and that intervention deserves a fresh plan
 * reading just as a new prompt does. Either signal alone suffices, which also
 * keeps this correct on a dsh build that stops passing `step`.
 *
 * @param {{step?: unknown, messages?: unknown}} position - the pre-step payload
 * @returns {boolean}
 */
export function startsTurn(position) {
  if (position?.step === 1) return true
  const messages = position?.messages
  if (!Array.isArray(messages)) return false
  return messages.some((m) => m?.source?.kind === 'user')
}

/**
 * Deep-freeze in place, matching how dsh publishes its own messages.
 * @param {unknown} value
 * @returns {unknown} the same value, frozen
 */
function deepFreeze(value) {
  if (value === null || typeof value !== 'object') return value
  Object.freeze(value)
  for (const key of Object.keys(value)) deepFreeze(value[key])
  return value
}

/**
 * Run `fleet dsh-context` and return its sections.
 *
 * A non-zero exit, a timeout, unparsable stdout, or a malformed payload all
 * resolve to an empty list: a context source that cannot answer must not stall
 * or fail the turn it is decorating.
 *
 * `userTitle` and `locale` are forwarded from this entry's config rather than
 * left to the CLI's defaults, which would render English guidance addressing the
 * user as "Boss".
 *
 * `pressure` is this session's context occupancy (see {@link readContextPressure}).
 * It is measured here and passed in because only this process holds the session
 * log; the CLI owns the tier policy and the wording, as it does for Claude and
 * Codex. Omitted when unreadable, which simply yields no reminder.
 *
 * @param {{fleetBin: string, timeoutMs: number, userTitle?: string, locale?: string, fleetVersion?: string}} config
 * @param {string} cwd - the session's working directory
 * @param {string} sessionId
 * @param {{used: number, window: number, model: string}} [pressure]
 * @returns {Promise<{sections: Array<{name: string, text: string}>, sandboxMode: string | undefined, oneShot: boolean}>}
 */
export async function fetchContext(config, cwd, sessionId, pressure) {
  const result = await runFleet(config, cwd, sessionId, pressure)
  // Version skew: this plugin is installed from `~/.fleet/dsh-plugin`, but
  // `fleetBin` points at whichever Fleet build is on the machine, and the two
  // update independently. A build predating `--ctx-used` rejects the whole
  // invocation, which would take the plan and guidance sections down with the
  // reminder — the sections that were working before this feature existed.
  // Retrying without the pressure flags costs one process on such a machine
  // and keeps the older contract intact until Fleet catches up.
  if (result === undefined) {
    const retried = pressure ? await runFleet(config, cwd, sessionId, undefined) : undefined
    if (retried === undefined) {
      // Both attempts failed, so this machine's `fleetBin` cannot answer at all
      // — most likely a build too old to know `dsh-context`. Everything below
      // degrades to injecting nothing, which is the right behaviour and also a
      // perfectly silent one: before this line, a dsh session simply ran
      // without any Fleet context and nothing said so. `fleetVersion` is the
      // build that materialized this file; printing both names the mismatched
      // pair outright.
      warnOnce(
        `[fleet] no context injected: '${config.fleetBin}' could not answer ` +
          `'fleet dsh-context' (this plugin was installed by Fleet ` +
          `${config.fleetVersion ?? 'unknown'}). The two halves are out of sync.`,
      )
    }
    return retried ?? NOTHING
  }
  return result
}

/** Warnings already printed, so a long session says each thing exactly once. */
const warned = new Set()

/** Print `message` to stderr the first time it is seen in this process. */
function warnOnce(message) {
  if (warned.has(message)) return
  warned.add(message)
  console.error(message)
}

/** What a CLI that could not answer yields: inject nothing, fail nothing. */
const NOTHING = { sections: [], sandboxMode: undefined, oneShot: false }

/**
 * One `fleet dsh-context` invocation.
 *
 * @returns {Promise<{sections: Array<{name: string, text: string}>, sandboxMode: string | undefined, oneShot: boolean} | undefined>}
 *   `undefined` when the process itself failed — the signal {@link fetchContext}
 *   retries on. A process that ran but said nothing useful resolves to empty
 *   sections instead, because re-running it would say the same thing.
 */
function runFleet(config, cwd, sessionId, pressure) {
  const args = ['dsh-context', '--cwd', cwd, '--session', sessionId]
  if (config.userTitle) args.push('--title', config.userTitle)
  if (config.locale) args.push('--locale', config.locale)
  if (pressure) {
    args.push('--ctx-used', String(pressure.used), '--ctx-window', String(pressure.window))
    if (pressure.model) args.push('--ctx-model', pressure.model)
  }
  const nothing = NOTHING
  return new Promise((resolve) => {
    execFile(
      config.fleetBin,
      args,
      { timeout: config.timeoutMs, maxBuffer: 4 * 1024 * 1024 },
      (error, stdout) => {
        if (error) return resolve(undefined)
        let parsed
        try {
          parsed = JSON.parse(stdout)
        } catch {
          return resolve(nothing)
        }
        const sections = Array.isArray(parsed?.sections)
          ? parsed.sections.filter(
              (s) =>
                s !== null &&
                typeof s === 'object' &&
                typeof s.name === 'string' &&
                typeof s.text === 'string' &&
                s.text.trim().length > 0,
            )
          : []
        // A mode is only honoured when the CLI names one as a non-empty string.
        // Anything else — absent, null, a number — leaves the session on dsh's
        // own default, which is the safe direction: escalation must be an
        // explicit decision Fleet made, never a parsing accident.
        const mode = parsed?.sandboxMode
        // Same strictness for the one-step contract: only a literal `true`
        // caps the turn. A build that predates the field omits it, and a
        // session that is not a side-question fork must keep running.
        resolve({
          sections,
          sandboxMode: typeof mode === 'string' && mode.length > 0 ? mode : undefined,
          oneShot: parsed?.oneShot === true,
        })
      },
    )
  })
}

/**
 * The sections alone, for callers that do not care about the sandbox decision.
 *
 * @param {{fleetBin: string, timeoutMs: number, userTitle?: string, locale?: string, fleetVersion?: string}} config
 * @param {string} cwd - the session's working directory
 * @param {string} sessionId
 * @param {{used: number, window: number, model: string}} [pressure]
 * @returns {Promise<Array<{name: string, text: string}>>}
 */
export async function fetchSections(config, cwd, sessionId, pressure) {
  return (await fetchContext(config, cwd, sessionId, pressure)).sections
}

/**
 * One session's durable event log, across the two shapes dsh has shipped.
 *
 * 0.1.1 exposed a plain `events` array; 0.1.2 replaced it with
 * `snapshotEvents()` (a frozen snapshot reused until the next append). Reading
 * the gone property was not a degraded read but a fatal one: this plugin runs
 * inside `agent/pre-step`, so the `TypeError` ended the turn — every turn on a
 * machine carrying this plugin died with `UNKNOWN: Cannot read properties of
 * undefined (reading 'length')`, dsh's own CLI included. Hence the last
 * fallback: a shape this does not recognise costs the inject-only-on-change
 * optimisation, never the turn.
 *
 * @param {any} session
 * @returns {Array<any>} the log in order, or an empty list
 */
function sessionEvents(session) {
  if (Array.isArray(session?.events)) return session.events
  if (typeof session?.snapshotEvents === 'function') {
    const snapshot = session.snapshotEvents()
    if (Array.isArray(snapshot)) return snapshot
  }
  return []
}

/**
 * Find the latest text this plugin injected for one section name, including a
 * reading compaction has shadowed.
 *
 * Scanning the log rather than caching in memory is what makes the
 * inject-only-on-change rule survive resume and a server restart — the same
 * reason dsh-time-context scans events for its refresh interval.
 *
 * Per-section rather than whole-message: the guidance sections are static and
 * should enter a session once, while the plan section changes as boxes get
 * ticked. Keyed on one body they would re-enter together every time a checkbox
 * moved.
 *
 * @param {{session: {events: Array<any>}}} agent
 * @param {string} sectionName
 * @returns {string | undefined}
 */
export function latestInjectedText(agent, sectionName) {
  const events = sessionEvents(agent.session)
  for (let i = events.length - 1; i >= 0; i--) {
    const event = events[i]
    if (event.type !== 'user/message') continue
    const source = event.data.source
    if (source?.kind !== 'plugin' || source.plugin !== name) continue
    const section = source.sections?.find((s) => s.name === sectionName)
    if (section !== undefined) return section.text
  }
}

/**
 * This session's live context occupancy, read from its own event log.
 *
 * dsh records the usage of every model call on the `assistant/message` it
 * produced: `inputTokens` is the uncached part of that request's prompt and
 * `cacheReadTokens` the cached part, so their sum is the whole prompt — i.e.
 * how full the window was on the last step. The window itself is on
 * `request/context`, which dsh writes once per request with the resolved
 * provider/model.
 *
 * Reading the log is what makes this free. The same numbers are available over
 * RPC (`session/list` → `projections.values.contextPressure`), but that is one
 * round trip against a server holding every session on the machine, paid on
 * every step of every session. The log is already in memory here.
 *
 * There is no compaction special case, unlike the Claude and Codex readers:
 * those read a *recorded* occupancy that a compaction invalidates, while this
 * reads the request dsh actually just made — the first `assistant/message`
 * after a compaction reports the smaller prompt on its own.
 *
 * Defensive like {@link sessionEvents}: this runs inside `agent/pre-step`,
 * where a `TypeError` ends the turn rather than degrading the reading.
 *
 * @param {any} agent
 * @returns {{used: number, window: number, model: string} | undefined}
 */
export function readContextPressure(agent) {
  const events = sessionEvents(agent?.session)
  let used
  let window
  let model = ''
  for (let i = events.length - 1; i >= 0; i--) {
    const event = events[i]
    const data = event?.data
    if (used === undefined && event?.type === 'assistant/message') {
      const usage = data?.usage
      const input = Number(usage?.inputTokens)
      const cached = Number(usage?.cacheReadTokens)
      const total = (Number.isFinite(input) ? input : 0) + (Number.isFinite(cached) ? cached : 0)
      if (total > 0) used = total
    }
    if (window === undefined && event?.type === 'request/context') {
      const w = Number(data?.contextWindow)
      if (Number.isFinite(w) && w > 0) {
        window = w
        if (typeof data?.model === 'string') model = data.model
      }
    }
    if (used !== undefined && window !== undefined) break
  }
  if (used === undefined || window === undefined) return undefined
  return { used, window, model }
}

/**
 * Switch this session's dsh sandbox mode, once.
 *
 * The switch IS its event: `setSandboxMode` in `@deepseek-ai/dsh-sandbox-policy`
 * is exactly `session.append('sandbox/mode', { mode })`, log-only, replayable and
 * scoped to the one session. Appending it here rather than importing that
 * package is deliberate — this plugin is loaded by absolute path, so it resolves
 * none of dsh's own packages.
 *
 * Why Fleet asks at all: every `fleet` command writes under `~/.fleet`, outside
 * any workspace, and dsh's sandbox has no allow-list, so a Fleet-driven session
 * on `workspace-write` pays an approval round-trip for each one. Which sessions
 * get this is NOT decided here — the CLI only names a mode for sessions Fleet
 * spawned; a session the user opened himself is never given one.
 *
 * Appending once per session is the point: a `sandbox/mode` already in the log
 * may be the user's own later choice, and re-appending ours every step would
 * silently overrule him.
 *
 * Defensive throughout, like {@link sessionEvents}: this runs inside
 * `agent/pre-step`, where a `TypeError` does not degrade the injection but ends
 * the turn.
 *
 * @param {any} agent
 * @param {string} mode
 * @returns {boolean} whether this call appended the event
 */
export function ensureSandboxMode(agent, mode) {
  const session = agent?.session
  if (typeof session?.append !== 'function') return false
  if (sessionEvents(session).some((e) => e?.type === 'sandbox/mode')) return false
  try {
    session.append('sandbox/mode', { mode })
    return true
  } catch {
    // A dsh build that does not know this event must cost the escalation, not
    // the turn.
    return false
  }
}

/**
 * Build the message carrying one section.
 * @param {{name: string, text: string}} section
 */
function sectionMessage(section) {
  return deepFreeze({
    id: crypto.randomUUID(),
    role: 'user',
    content: [{ type: 'text', text: section.text }],
    source: {
      kind: 'plugin',
      plugin: name,
      form: 'snapshot',
      sections: [{ name: section.name, text: section.text }],
    },
  })
}

/**
 * Register the pre-step listener for the lifetime of `ctx`.
 *
 * @param {any} ctx - plugin context; the listener is disposed with it
 * @param {{fleetBin?: string, timeoutMs?: number, fleetVersion?: string}} [config]
 */
export function apply(ctx, config) {
  const resolved = {
    fleetBin: config?.fleetBin ?? 'fleet',
    // The Fleet build that materialized this file. Diagnostic only — it is
    // never sent to `fleetBin`, because a flag an older build rejects takes the
    // whole invocation down with it.
    fleetVersion: config?.fleetVersion,
    timeoutMs: config?.timeoutMs ?? DEFAULT_TIMEOUT_MS,
    userTitle: config?.userTitle,
    locale: config?.locale,
  }

  ctx.on(
    'agent/pre-step',
    async ({ agent, signal, step, messages }, next) => {
      const decision = await next()
      if (decision.kind === 'reject' || signal.aborted) return decision

      const cwd = agent.session.header.cwd
      if (typeof cwd !== 'string' || cwd.length === 0) return decision

      const { sections, sandboxMode, oneShot } = await fetchContext(
        resolved,
        cwd,
        agent.session.id,
        readContextPressure(agent),
      )

      // Before the early return below: on a steady-state step every section is
      // unchanged and we return without injecting, so a switch gated behind that
      // would never happen on a resumed session.
      if (sandboxMode) ensureSandboxMode(agent, sandboxMode)

      // A side-question fork answers in exactly one step. Its prompt already
      // forbids tools; this is the hard stop for a model that calls one anyway:
      // the tool result would open step 2, and step 2 is rejected before any
      // request is made. Rejecting rather than trimming the tools list keeps
      // the child's request byte-identical to its parent's prefix, which is
      // the whole reason the question is asked on a fork. `step` is dsh's
      // 1-based counter within the turn; a build that stops passing it is not
      // second-guessed, so the soft contract still stands there.
      if (oneShot && typeof step === 'number' && step >= 2) return { kind: 'reject' }

      if (sections.length === 0 || signal.aborted) return decision

      // Re-injecting an unchanged section every step would spend the whole
      // context window on the same few kilobytes. dsh keeps the message in
      // derived history until compaction shadows it, so an identical latest
      // reading is still in front of the model and this step needs nothing.
      //
      // A changed turn-scoped section waits for the next prompt: the step that
      // notices it mid-turn returns without it, and the next `startsTurn` step
      // re-reads the CLI and finds it still differs from the log.
      const turnStart = startsTurn({ step, messages })
      const fresh = sections.filter(
        (s) =>
          latestInjectedText(agent, s.name) !== s.text &&
          (turnStart || !TURN_SCOPED_SECTIONS.has(s.name)),
      )
      if (fresh.length === 0) return decision

      return {
        kind: 'enter',
        messages: [...decision.messages, ...fresh.map(sectionMessage)],
      }
    },
    { prepend: true },
  )
}
