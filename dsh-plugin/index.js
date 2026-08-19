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
 * @param {{fleetBin: string, timeoutMs: number}} config
 * @param {string} cwd - the session's working directory
 * @param {string} sessionId
 * @returns {Promise<Array<{name: string, text: string}>>}
 */
export function fetchSections(config, cwd, sessionId) {
  return new Promise((resolve) => {
    execFile(
      config.fleetBin,
      ['dsh-context', '--cwd', cwd, '--session', sessionId],
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
 * Render sections into the one text body the injected message carries.
 * @param {Array<{name: string, text: string}>} sections
 * @returns {string}
 */
export function renderSections(sections) {
  return sections.map((s) => s.text).join('\n\n')
}

/**
 * Find this plugin's latest injected text in the durable log, including a
 * reading compaction has shadowed.
 *
 * Scanning the log rather than caching in memory is what makes the
 * inject-only-on-change rule survive resume and a server restart — the same
 * reason dsh-time-context scans events for its refresh interval.
 *
 * @param {{session: {events: Array<any>}}} agent
 * @returns {string | undefined}
 */
export function latestInjectedText(agent) {
  const events = agent.session.events
  for (let i = events.length - 1; i >= 0; i--) {
    const event = events[i]
    if (event.type !== 'user/message') continue
    const source = event.data.source
    if (source?.kind === 'plugin' && source.plugin === name) {
      return source.sections?.map((s) => s.text).join('\n\n')
    }
  }
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

      const text = renderSections(sections)

      // Re-injecting an unchanged body every step would spend the whole context
      // window on the same few kilobytes. dsh keeps the message in derived
      // history until compaction shadows it, so an identical latest reading is
      // still in front of the model and this step needs nothing.
      if (latestInjectedText(agent) === text) return decision

      const message = deepFreeze({
        id: crypto.randomUUID(),
        role: 'user',
        content: [{ type: 'text', text }],
        source: {
          kind: 'plugin',
          plugin: name,
          form: 'snapshot',
          sections: sections.map((s) => ({ name: s.name, text: s.text })),
        },
      })

      return { kind: 'enter', messages: [...decision.messages, message] }
    },
    { prepend: true },
  )
}
