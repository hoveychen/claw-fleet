import assert from 'node:assert/strict'
import { chmodSync, mkdtempSync, rmSync, writeFileSync } from 'node:fs'
import { join } from 'node:path'
import { after, describe, test } from 'node:test'

import { apply, fetchSections, latestInjectedText, name, renderSections } from './index.js'

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

/**
 * Drive `apply`'s listener the way cordis would.
 * @param {{fleetBin: string, timeoutMs?: number}} config
 * @param {any} agent
 * @param {{kind: string, messages?: Array<any>}} decision - what `next()` returns
 */
async function runPreStep(config, agent, decision = { kind: 'enter', messages: [] }) {
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
  return listener({ agent, turn: 1, step: 1, signal: { aborted: false } }, async () => decision)
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
    const fleetBin = stubFleet(
      'argv',
      `printf '{"sections":[{"name":"argv","text":"%s"}]}' "$*"`,
    )
    const sections = await fetchSections({ fleetBin, timeoutMs: 5000 }, '/some/ws', 'session-xyz')
    assert.equal(
      sections[0].text,
      'dsh-context --cwd /some/ws --session session-xyz',
    )
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

describe('renderSections', () => {
  test('joins section bodies with a blank line', () => {
    assert.equal(
      renderSections([
        { name: 'a', text: 'first' },
        { name: 'b', text: 'second' },
      ]),
      'first\n\nsecond',
    )
  })
})

describe('latestInjectedText', () => {
  test('reads back the newest reading this plugin wrote', () => {
    const agent = fakeAgent({
      events: [
        { type: 'user/message', data: { source: { kind: 'user' } } },
        {
          type: 'user/message',
          data: {
            source: { kind: 'plugin', plugin: name, sections: [{ name: 'x', text: 'old' }] },
          },
        },
        {
          type: 'user/message',
          data: {
            source: { kind: 'plugin', plugin: name, sections: [{ name: 'x', text: 'new' }] },
          },
        },
      ],
    })
    assert.equal(latestInjectedText(agent), 'new')
  })

  test('ignores another plugin\'s messages', () => {
    const agent = fakeAgent({
      events: [
        {
          type: 'user/message',
          data: {
            source: { kind: 'plugin', plugin: 'time-context', sections: [{ name: 't', text: 'x' }] },
          },
        },
      ],
    })
    assert.equal(latestInjectedText(agent), undefined)
  })

  test('an empty log has no reading', () => {
    assert.equal(latestInjectedText(fakeAgent()), undefined)
  })
})

describe('apply', () => {
  test('appends one plugin-sourced message carrying the sections', async () => {
    const fleetBin = stubFleet(
      'inject',
      `echo '{"sections":[{"name":"fleet-prd","text":"PLAN BODY"}]}'`,
    )
    const decision = await runPreStep({ fleetBin }, fakeAgent(), {
      kind: 'enter',
      messages: [{ id: 'prompt', role: 'user' }],
    })

    assert.equal(decision.kind, 'enter')
    assert.equal(decision.messages.length, 2)
    const injected = decision.messages[1]
    assert.equal(injected.role, 'user')
    assert.deepEqual(injected.content, [{ type: 'text', text: 'PLAN BODY' }])
    assert.equal(injected.source.kind, 'plugin')
    assert.equal(injected.source.plugin, name)
    assert.equal(injected.source.form, 'snapshot')
    assert.deepEqual(injected.source.sections, [{ name: 'fleet-prd', text: 'PLAN BODY' }])
    assert.ok(Object.isFrozen(injected), 'the message must be frozen')
    assert.ok(Object.isFrozen(injected.content[0]), 'freezing must be deep')
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

  test('does not re-inject an unchanged body already in the log', async () => {
    const fleetBin = stubFleet('same', `echo '{"sections":[{"name":"p","text":"BODY"}]}'`)
    const agent = fakeAgent({
      events: [
        {
          type: 'user/message',
          data: { source: { kind: 'plugin', plugin: name, sections: [{ name: 'p', text: 'BODY' }] } },
        },
      ],
    })
    const decision = await runPreStep({ fleetBin }, agent, { kind: 'enter', messages: [] })
    assert.deepEqual(decision.messages, [])
  })

  test('re-injects once the body changes', async () => {
    const fleetBin = stubFleet('changed', `echo '{"sections":[{"name":"p","text":"NEW BODY"}]}'`)
    const agent = fakeAgent({
      events: [
        {
          type: 'user/message',
          data: { source: { kind: 'plugin', plugin: name, sections: [{ name: 'p', text: 'OLD' }] } },
        },
      ],
    })
    const decision = await runPreStep({ fleetBin }, agent, { kind: 'enter', messages: [] })
    assert.equal(decision.messages.length, 1)
    assert.equal(decision.messages[0].content[0].text, 'NEW BODY')
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
