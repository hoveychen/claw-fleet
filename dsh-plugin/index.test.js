import assert from 'node:assert/strict'
import { chmodSync, mkdtempSync, rmSync, writeFileSync } from 'node:fs'
import { join } from 'node:path'
import { after, describe, test } from 'node:test'

import {
  TURN_SCOPED_SECTIONS,
  apply,
  ensureSandboxMode,
  fetchContext,
  fetchSections,
  latestInjectedText,
  name,
  startsTurn,
} from './index.js'

// Scratch dir for the stub `fleet` executables. `/tmp` rather than os.tmpdir()
// so the path stays inside this session's writable roots.
const scratch = mkdtempSync('/tmp/fleet-dsh-plugin-')
after(() => rmSync(scratch, { recursive: true, force: true }))

/**
 * Write an executable stub standing in for the `fleet` binary.
 * @param {string} label - file name, so each test gets its own stub
 * @param {string} body - shell body; `$@` receives the real argv
 */
function stubFleet(label, body) {
  const path = join(scratch, label)
  writeFileSync(path, `#!/bin/sh\n${body}\n`)
  chmodSync(path, 0o755)
  return path
}

/** Minimal `agent` double: only `session.header.cwd`, `session.id`, and the log. */
function fakeAgent({ cwd = '/ws', id = 'session-1', events = [] } = {}) {
  return { session: { header: { cwd }, id, events } }
}

/** A durable log entry standing in for one of this plugin's past injections. */
function injected(sectionName, text) {
  return {
    type: 'user/message',
    data: { source: { kind: 'plugin', plugin: name, sections: [{ name: sectionName, text }] } },
  }
}

/**
 * Drive `apply`'s listener the way cordis would.
 * @param {{fleetBin: string, timeoutMs?: number, userTitle?: string, locale?: string}} config
 * @param {any} agent
 * @param {{kind: string, messages?: Array<any>}} decision - what `next()` returns
 * @param {{turn?: number, step?: number, messages?: Array<any>}} [position] - the
 *   step's place in its turn and the batch dsh claimed for it; defaults to a
 *   turn's first step, which is where every section is allowed to enter
 */
async function runPreStep(
  config,
  agent,
  decision = { kind: 'enter', messages: [] },
  position = { turn: 1, step: 1 },
) {
  let listener
  const ctx = {
    on(event, fn, options) {
      assert.equal(event, 'agent/pre-step')
      assert.deepEqual(options, { prepend: true })
      listener = fn
    },
  }
  apply(ctx, config)
  assert.ok(listener, 'apply must register a pre-step listener')
  return listener(
    { agent, turn: 1, messages: [], ...position, signal: { aborted: false } },
    async () => decision,
  )
}

describe('fetchSections', () => {
  test('returns the sections the CLI printed', async () => {
    const fleetBin = stubFleet(
      'ok',
      `echo '{"sections":[{"name":"fleet-prd","text":"PLAN BODY"}]}'`,
    )
    const sections = await fetchSections({ fleetBin, timeoutMs: 5000 }, '/ws', 'session-1')
    assert.deepEqual(sections, [{ name: 'fleet-prd', text: 'PLAN BODY' }])
  })

  test('passes the cwd and session id through to the CLI', async () => {
    const fleetBin = stubFleet('argv', `printf '{"sections":[{"name":"argv","text":"%s"}]}' "$*"`)
    const sections = await fetchSections({ fleetBin, timeoutMs: 5000 }, '/some/ws', 'session-xyz')
    assert.equal(sections[0].text, 'dsh-context --cwd /some/ws --session session-xyz')
  })

  test('forwards the configured user title and locale', async () => {
    // Without these the CLI defaults to `Boss` / `en`, which would render
    // English guidance addressing a user whose Fleet says otherwise.
    const fleetBin = stubFleet('argv2', `printf '{"sections":[{"name":"argv","text":"%s"}]}' "$*"`)
    const sections = await fetchSections(
      { fleetBin, timeoutMs: 5000, userTitle: '老板', locale: 'zh' },
      '/ws',
      's',
    )
    assert.equal(sections[0].text, 'dsh-context --cwd /ws --session s --title 老板 --locale zh')
  })

  test('omits the title and locale flags when unconfigured', async () => {
    const fleetBin = stubFleet('argv3', `printf '{"sections":[{"name":"argv","text":"%s"}]}' "$*"`)
    const sections = await fetchSections({ fleetBin, timeoutMs: 5000 }, '/ws', 's')
    assert.ok(!sections[0].text.includes('--title'))
    assert.ok(!sections[0].text.includes('--locale'))
  })

  test('an empty sections array stays empty', async () => {
    const fleetBin = stubFleet('empty', `echo '{"sections":[]}'`)
    assert.deepEqual(await fetchSections({ fleetBin, timeoutMs: 5000 }, '/ws', 's'), [])
  })

  test('drops sections whose text is blank or whose fields are wrong', async () => {
    const fleetBin = stubFleet(
      'junk',
      `echo '{"sections":[{"name":"a","text":"  "},{"name":"b"},{"text":"c"},null,{"name":"d","text":"keep"}]}'`,
    )
    assert.deepEqual(await fetchSections({ fleetBin, timeoutMs: 5000 }, '/ws', 's'), [
      { name: 'd', text: 'keep' },
    ])
  })

  test('a non-zero exit yields no sections instead of throwing', async () => {
    const fleetBin = stubFleet('fail', 'echo boom >&2; exit 3')
    assert.deepEqual(await fetchSections({ fleetBin, timeoutMs: 5000 }, '/ws', 's'), [])
  })

  test('unparsable stdout yields no sections', async () => {
    const fleetBin = stubFleet('garbage', 'echo not-json')
    assert.deepEqual(await fetchSections({ fleetBin, timeoutMs: 5000 }, '/ws', 's'), [])
  })

  test('a missing binary yields no sections', async () => {
    const sections = await fetchSections(
      { fleetBin: join(scratch, 'does-not-exist'), timeoutMs: 5000 },
      '/ws',
      's',
    )
    assert.deepEqual(sections, [])
  })

  test('a hanging CLI is cut off by the timeout', async () => {
    const fleetBin = stubFleet('hang', 'sleep 30')
    assert.deepEqual(await fetchSections({ fleetBin, timeoutMs: 300 }, '/ws', 's'), [])
  })
})

describe('latestInjectedText', () => {
  test('reads back the newest reading for that section', () => {
    const agent = fakeAgent({
      events: [
        { type: 'user/message', data: { source: { kind: 'user' } } },
        injected('fleet-prd', 'old'),
        injected('fleet-prd', 'new'),
      ],
    })
    assert.equal(latestInjectedText(agent, 'fleet-prd'), 'new')
  })

  test('sections are tracked independently of each other', () => {
    const agent = fakeAgent({
      events: [injected('fleet-guidance-prd', 'GUIDANCE'), injected('fleet-prd', 'PLANS')],
    })
    assert.equal(latestInjectedText(agent, 'fleet-guidance-prd'), 'GUIDANCE')
    assert.equal(latestInjectedText(agent, 'fleet-prd'), 'PLANS')
    assert.equal(latestInjectedText(agent, 'fleet-guidance-wiki'), undefined)
  })

  test("ignores another plugin's messages", () => {
    const agent = fakeAgent({
      events: [
        {
          type: 'user/message',
          data: {
            source: {
              kind: 'plugin',
              plugin: 'time-context',
              sections: [{ name: 'fleet-prd', text: 'x' }],
            },
          },
        },
      ],
    })
    assert.equal(latestInjectedText(agent, 'fleet-prd'), undefined)
  })

  test('an empty log has no reading', () => {
    assert.equal(latestInjectedText(fakeAgent(), 'fleet-prd'), undefined)
  })

  /// dsh 0.1.2 dropped `Session.events` for `snapshotEvents()`. Reading the
  /// gone property threw inside `agent/pre-step`, which ends the turn — every
  /// turn on a machine carrying this plugin died with
  /// `UNKNOWN: Cannot read properties of undefined (reading 'length')`
  /// (isolated live: adding this plugin's patch file to a working 0.1.2 home
  /// reproduced it, removing it fixed it).
  test('reads the log through snapshotEvents when there is no events array', () => {
    const events = [injected('fleet-prd', 'from-snapshot')]
    const agent = { session: { header: { cwd: '/ws' }, id: 's', snapshotEvents: () => events } }
    assert.equal(latestInjectedText(agent, 'fleet-prd'), 'from-snapshot')
  })

  test('a session exposing neither shape is silent rather than fatal', () => {
    const agent = { session: { header: { cwd: '/ws' }, id: 's' } }
    assert.equal(latestInjectedText(agent, 'fleet-prd'), undefined)
  })
})

describe('apply', () => {
  test('appends one plugin-sourced message per section', async () => {
    const fleetBin = stubFleet(
      'inject',
      `echo '{"sections":[{"name":"fleet-guidance-prd","text":"RULES"},{"name":"fleet-prd","text":"PLAN BODY"}]}'`,
    )
    const decision = await runPreStep({ fleetBin }, fakeAgent(), {
      kind: 'enter',
      messages: [{ id: 'prompt', role: 'user' }],
    })

    assert.equal(decision.kind, 'enter')
    assert.equal(decision.messages.length, 3)
    const [, guidance, plans] = decision.messages

    assert.deepEqual(guidance.content, [{ type: 'text', text: 'RULES' }])
    assert.deepEqual(guidance.source.sections, [{ name: 'fleet-guidance-prd', text: 'RULES' }])
    assert.deepEqual(plans.source.sections, [{ name: 'fleet-prd', text: 'PLAN BODY' }])

    for (const message of [guidance, plans]) {
      assert.equal(message.role, 'user')
      assert.equal(message.source.kind, 'plugin')
      assert.equal(message.source.plugin, name)
      assert.equal(message.source.form, 'snapshot')
      assert.ok(Object.isFrozen(message), 'the message must be frozen')
      assert.ok(Object.isFrozen(message.content[0]), 'freezing must be deep')
    }
  })

  test('the injected message is NOT an eligible session-title message', async () => {
    // The whole reason this plugin exists: dsh's title provider only collects
    // `user/message` events whose `source.kind === 'user'`.
    const fleetBin = stubFleet('kind', `echo '{"sections":[{"name":"p","text":"BODY"}]}'`)
    const decision = await runPreStep({ fleetBin }, fakeAgent())
    assert.notEqual(decision.messages[0].source.kind, 'user')
  })

  test('injects nothing when the CLI reports no sections', async () => {
    const fleetBin = stubFleet('none', `echo '{"sections":[]}'`)
    const decision = await runPreStep({ fleetBin }, fakeAgent(), { kind: 'enter', messages: [] })
    assert.deepEqual(decision.messages, [])
  })

  test('does not re-inject a section whose body is unchanged', async () => {
    const fleetBin = stubFleet('same', `echo '{"sections":[{"name":"p","text":"BODY"}]}'`)
    const agent = fakeAgent({ events: [injected('p', 'BODY')] })
    const decision = await runPreStep({ fleetBin }, agent, { kind: 'enter', messages: [] })
    assert.deepEqual(decision.messages, [])
  })

  test('re-injects a section once its body changes', async () => {
    const fleetBin = stubFleet('changed', `echo '{"sections":[{"name":"p","text":"NEW BODY"}]}'`)
    const agent = fakeAgent({ events: [injected('p', 'OLD')] })
    const decision = await runPreStep({ fleetBin }, agent, { kind: 'enter', messages: [] })
    assert.equal(decision.messages.length, 1)
    assert.equal(decision.messages[0].content[0].text, 'NEW BODY')
  })

  test('a changed plan section does not drag the static guidance back in', async () => {
    // The reason de-dup is per section: guidance enters a session once, while
    // the plan body changes every time a checkbox moves.
    const fleetBin = stubFleet(
      'mixed',
      `echo '{"sections":[{"name":"fleet-guidance-prd","text":"RULES"},{"name":"fleet-prd","text":"PLANS v2"}]}'`,
    )
    const agent = fakeAgent({
      events: [injected('fleet-guidance-prd', 'RULES'), injected('fleet-prd', 'PLANS v1')],
    })
    const decision = await runPreStep({ fleetBin }, agent, { kind: 'enter', messages: [] })
    assert.equal(decision.messages.length, 1)
    assert.deepEqual(decision.messages[0].source.sections, [
      { name: 'fleet-prd', text: 'PLANS v2' },
    ])
  })

  test('a changed plan section waits for the next user prompt', async () => {
    // Parity with the Claude hook, which renders the same TASKS.md text only on
    // UserPromptSubmit. Mid-turn, every checkbox any session in the workspace
    // ticks would otherwise append another full copy — 21 copies in one turn
    // was measured. This step is mid-turn: step 3, no user message in the batch.
    const fleetBin = stubFleet('plan-midturn', `echo '{"sections":[{"name":"fleet-prd","text":"PLANS v2"}]}'`)
    const agent = fakeAgent({ events: [injected('fleet-prd', 'PLANS v1')] })
    const decision = await runPreStep(
      { fleetBin },
      agent,
      { kind: 'enter', messages: [{ id: 'tool-results', role: 'user', source: { kind: 'tool' } }] },
      { turn: 1, step: 3, messages: [{ source: { kind: 'tool' } }] },
    )
    assert.equal(decision.messages.length, 1, 'only the batch dsh claimed, nothing appended')
  })

  test('the deferred plan change enters on the next turn-opening step', async () => {
    const fleetBin = stubFleet('plan-nextturn', `echo '{"sections":[{"name":"fleet-prd","text":"PLANS v2"}]}'`)
    const agent = fakeAgent({ events: [injected('fleet-prd', 'PLANS v1')] })
    const decision = await runPreStep(
      { fleetBin },
      agent,
      { kind: 'enter', messages: [{ id: 'prompt', role: 'user', source: { kind: 'user' } }] },
      { turn: 2, step: 1, messages: [{ source: { kind: 'user' } }] },
    )
    assert.equal(decision.messages.length, 2)
    assert.deepEqual(decision.messages[1].source.sections, [{ name: 'fleet-prd', text: 'PLANS v2' }])
  })

  test('a user message spliced in mid-turn also lets the plan section in', async () => {
    // The user typing while the agent runs lands in a later step's inbox with
    // source.kind === 'user'; that intervention gets a fresh plan reading too.
    const fleetBin = stubFleet('plan-steer', `echo '{"sections":[{"name":"fleet-prd","text":"PLANS v2"}]}'`)
    const agent = fakeAgent({ events: [injected('fleet-prd', 'PLANS v1')] })
    const decision = await runPreStep(
      { fleetBin },
      agent,
      { kind: 'enter', messages: [{ id: 'steer', role: 'user', source: { kind: 'user' } }] },
      { turn: 1, step: 7, messages: [{ source: { kind: 'user' } }] },
    )
    assert.equal(decision.messages.length, 2)
    assert.equal(decision.messages[1].content[0].text, 'PLANS v2')
  })

  test('the first plan reading of a session is not delayed', async () => {
    // No prior reading in the log: a fresh session's first step is step 1 anyway,
    // and a session resumed mid-plan opens with a prompt. Both are turn starts.
    const fleetBin = stubFleet('plan-first', `echo '{"sections":[{"name":"fleet-prd","text":"PLANS"}]}'`)
    const decision = await runPreStep({ fleetBin }, fakeAgent(), { kind: 'enter', messages: [] })
    assert.equal(decision.messages.length, 1)
  })

  test('static guidance is not turn-scoped and still enters mid-turn when it changes', async () => {
    // Guidance text only changes when Fleet itself is updated; there is nothing
    // to throttle, and holding it back would leave a session on stale rules.
    const fleetBin = stubFleet(
      'guidance-midturn',
      `echo '{"sections":[{"name":"fleet-guidance-prd","text":"RULES v2"},{"name":"fleet-prd","text":"PLANS v2"}]}'`,
    )
    const agent = fakeAgent({
      events: [injected('fleet-guidance-prd', 'RULES v1'), injected('fleet-prd', 'PLANS v1')],
    })
    const decision = await runPreStep(
      { fleetBin },
      agent,
      { kind: 'enter', messages: [] },
      { turn: 1, step: 4, messages: [{ source: { kind: 'tool' } }] },
    )
    assert.equal(decision.messages.length, 1)
    assert.deepEqual(decision.messages[0].source.sections, [
      { name: 'fleet-guidance-prd', text: 'RULES v2' },
    ])
  })

  test('a rejected step is passed through untouched', async () => {
    const fleetBin = stubFleet('reject', `echo '{"sections":[{"name":"p","text":"BODY"}]}'`)
    const decision = await runPreStep({ fleetBin }, fakeAgent(), { kind: 'reject' })
    assert.deepEqual(decision, { kind: 'reject' })
  })

  test('a session with no cwd is passed through untouched', async () => {
    const fleetBin = stubFleet('nocwd', `echo '{"sections":[{"name":"p","text":"BODY"}]}'`)
    const decision = await runPreStep({ fleetBin }, fakeAgent({ cwd: '' }), {
      kind: 'enter',
      messages: [],
    })
    assert.deepEqual(decision.messages, [])
  })
})

describe('startsTurn', () => {
  test('step 1 opens a turn regardless of the batch', () => {
    assert.equal(startsTurn({ step: 1, messages: [] }), true)
    assert.equal(startsTurn({ step: 1 }), true)
  })

  test('a later step opens a turn only when a user message is in the batch', () => {
    assert.equal(startsTurn({ step: 2, messages: [{ source: { kind: 'tool' } }] }), false)
    assert.equal(startsTurn({ step: 2, messages: [{ source: { kind: 'user' } }] }), true)
    assert.equal(startsTurn({ step: 9, messages: [{ source: { kind: 'plugin' } }, { source: { kind: 'user' } }] }), true)
  })

  test("another agent's message is not a prompt", () => {
    // Subagent replies arrive as `agent-message`; dsh's own context as `plugin`
    // and `agent-instructions`. None of those is the user speaking.
    assert.equal(startsTurn({ step: 5, messages: [{ source: { kind: 'agent-message' } }] }), false)
    assert.equal(startsTurn({ step: 5, messages: [{ source: { kind: 'agent-instructions' } }] }), false)
  })

  test('an unrecognised payload is not a turn start rather than a throw', () => {
    // Runs inside agent/pre-step, where a throw ends the turn.
    assert.equal(startsTurn(undefined), false)
    assert.equal(startsTurn({}), false)
    assert.equal(startsTurn({ step: 3, messages: 'nope' }), false)
    assert.equal(startsTurn({ step: 3, messages: [null, 7, {}] }), false)
  })

  test('only the plan section is turn-scoped', () => {
    assert.deepEqual([...TURN_SCOPED_SECTIONS], ['fleet-prd'])
  })
})

describe('ensureSandboxMode', () => {
  /** An agent double whose session records what was appended to its log. */
  function appendingAgent(events = []) {
    const appended = []
    return {
      appended,
      agent: {
        session: {
          header: { cwd: '/ws' },
          id: 'session-1',
          events,
          append(type, data) {
            appended.push({ type, data })
            events.push({ type, data })
          },
        },
      },
    }
  }

  test('appends the switch as one sandbox/mode event', () => {
    const { agent, appended } = appendingAgent()
    assert.equal(ensureSandboxMode(agent, 'danger-full-access'), true)
    assert.deepEqual(appended, [
      { type: 'sandbox/mode', data: { mode: 'danger-full-access' } },
    ])
  })

  test('never appends twice in one session', () => {
    // Re-appending on every step would bloat the log, and — worse — would
    // reinstate our mode after the user switched away from it.
    const { agent, appended } = appendingAgent()
    ensureSandboxMode(agent, 'danger-full-access')
    assert.equal(ensureSandboxMode(agent, 'danger-full-access'), false)
    assert.equal(appended.length, 1)
  })

  test("leaves the user's own later switch alone", () => {
    const { agent, appended } = appendingAgent([
      { type: 'sandbox/mode', data: { mode: 'read-only' } },
    ])
    assert.equal(ensureSandboxMode(agent, 'danger-full-access'), false)
    assert.equal(appended.length, 0)
  })

  test('a session with no append method costs the escalation, not the turn', () => {
    // The `sessionEvents` precedent: a shape we do not recognise must degrade,
    // because a throw inside agent/pre-step ends the turn.
    assert.equal(ensureSandboxMode({ session: { events: [] } }, 'danger-full-access'), false)
    assert.equal(ensureSandboxMode({}, 'danger-full-access'), false)
  })

  test('an append that throws is swallowed', () => {
    const agent = {
      session: {
        events: [],
        append() {
          throw new TypeError('unknown event type')
        },
      },
    }
    assert.equal(ensureSandboxMode(agent, 'danger-full-access'), false)
  })
})

describe('fetchContext sandbox mode', () => {
  test('carries the mode the CLI named', async () => {
    const fleetBin = stubFleet(
      'mode',
      `echo '{"sections":[],"sandboxMode":"danger-full-access"}'`,
    )
    const ctx = await fetchContext({ fleetBin, timeoutMs: 5000 }, '/ws', 's')
    assert.equal(ctx.sandboxMode, 'danger-full-access')
  })

  test('an absent, null or non-string mode leaves the session on dsh defaults', async () => {
    // The CLI omits the field for a session Fleet did not spawn. Escalation has
    // to be an explicit decision, so anything that is not a real mode string
    // must read as "no decision".
    for (const [label, body] of [
      ['absent', `echo '{"sections":[]}'`],
      ['null', `echo '{"sections":[],"sandboxMode":null}'`],
      ['number', `echo '{"sections":[],"sandboxMode":7}'`],
      ['empty', `echo '{"sections":[],"sandboxMode":""}'`],
    ]) {
      const fleetBin = stubFleet(`mode-${label}`, body)
      const ctx = await fetchContext({ fleetBin, timeoutMs: 5000 }, '/ws', 's')
      assert.equal(ctx.sandboxMode, undefined, `${label} must not escalate`)
    }
  })

  test('the pre-step listener switches the mode even when no section changed', async () => {
    // The steady state: every section is already in the log, so the listener
    // returns without injecting. The switch must still have happened, or a
    // resumed session never gets it.
    const fleetBin = stubFleet(
      'steady',
      `echo '{"sections":[{"name":"fleet-prd","text":"SAME"}],"sandboxMode":"danger-full-access"}'`,
    )
    const events = [injected('fleet-prd', 'SAME')]
    const appended = []
    const agent = {
      session: {
        header: { cwd: '/ws' },
        id: 'session-1',
        events,
        append(type, data) {
          appended.push({ type, data })
          events.push({ type, data })
        },
      },
    }
    const decision = await runPreStep({ fleetBin, timeoutMs: 5000 }, agent)
    assert.equal(decision.messages.length, 0, 'nothing new to inject')
    assert.deepEqual(appended, [
      { type: 'sandbox/mode', data: { mode: 'danger-full-access' } },
    ])
  })
})
