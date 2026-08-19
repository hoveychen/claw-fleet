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

import { execFile } from 'node:child_process'

/** Cordis plugin name used by loader diagnostics. */
export const name = 'fleet-context'

/** The agent registry owns pre-step processing. */
export const inject = ['agents']

/** Default ceiling for one `fleet dsh-context` call, in milliseconds. */
const DEFAULT_TIMEOUT_MS = 5000

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
 * @param {{fleetBin: string, timeoutMs: number, userTitle?: string, locale?: string}} config
 * @param {string} cwd - the session's working directory
 * @param {string} sessionId
 * @returns {Promise<Array<{name: string, text: string}>>}
 */
export function fetchSections(config, cwd, sessionId) {
  const args = ['dsh-context', '--cwd', cwd, '--session', sessionId]
  if (config.userTitle) args.push('--title', config.userTitle)
  if (config.locale) args.push('--locale', config.locale)
  return new Promise((resolve) => {
    execFile(
      config.fleetBin,
      args,
      { timeout: config.timeoutMs, maxBuffer: 4 * 1024 * 1024 },
      (error, stdout) => {
        if (error) return resolve([])
        let parsed
        try {
          parsed = JSON.parse(stdout)
        } catch {
          return resolve([])
        }
        const sections = parsed?.sections
        if (!Array.isArray(sections)) return resolve([])
        resolve(
          sections.filter(
            (s) =>
              s !== null &&
              typeof s === 'object' &&
              typeof s.name === 'string' &&
              typeof s.text === 'string' &&
              s.text.trim().length > 0,
          ),
        )
      },
    )
  })
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
  const events = agent.session.events
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
 * @param {{fleetBin?: string, timeoutMs?: number}} [config]
 */
export function apply(ctx, config) {
  const resolved = {
    fleetBin: config?.fleetBin ?? 'fleet',
    timeoutMs: config?.timeoutMs ?? DEFAULT_TIMEOUT_MS,
    userTitle: config?.userTitle,
    locale: config?.locale,
  }

  ctx.on(
    'agent/pre-step',
    async ({ agent, signal }, next) => {
      const decision = await next()
      if (decision.kind === 'reject' || signal.aborted) return decision

      const cwd = agent.session.header.cwd
      if (typeof cwd !== 'string' || cwd.length === 0) return decision

      const sections = await fetchSections(resolved, cwd, agent.session.id)
      if (sections.length === 0 || signal.aborted) return decision

      // Re-injecting an unchanged section every step would spend the whole
      // context window on the same few kilobytes. dsh keeps the message in
      // derived history until compaction shadows it, so an identical latest
      // reading is still in front of the model and this step needs nothing.
      const fresh = sections.filter((s) => latestInjectedText(agent, s.name) !== s.text)
      if (fresh.length === 0) return decision

      return {
        kind: 'enter',
        messages: [...decision.messages, ...fresh.map(sectionMessage)],
      }
    },
    { prepend: true },
  )
}
