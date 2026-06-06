// Workflow execution-based extractor (node sidecar).
//
// Runs a Claude Code Workflow script ONCE with a fully mocked framework
// (agent/parallel/pipeline/phase/log/workflow + args/budget) to recover the
// orchestration skeleton: one entry per lexical `agent(...)` call-site, with the
// resolved-prompt static head (for binding), kind (single/parallel/pipeline),
// phase, label, agentType, and pipeline grouping. Pairs with
// `workflow_sidecar.rs`, which feeds the JSON output into the same `build_dag`
// the static byte-scanner uses, then binds real journal agents on top.
//
// Why execution beats the regex scanner for fingerprints: every interpolation
// point becomes a SENTINEL, so the static prompt head is just `resolved.split(
// SENTINEL)[0]` — robust to builder functions, nested concats, `.map` indices,
// template literals, etc., with no per-pattern parsing.
//
// Usage: node workflow_harness.mjs <script-path> [argsJSON]
// stdout: {ok, capped, error, name, description, phases, calls[]}
//   call = {site, kind, pipelineId, phase, label, agentType, schema, fingerprint, promptLen}

import { readFileSync } from 'node:fs'

const CAP = 4000                 // max agent() calls — breaks data-driven loops
const SENTINEL = '␟'        // marks where a mock (interpolated) value landed; never in real prompts
class CapHit extends Error {}

// Property names the script ever reads — seeded into the magic Proxy's ownKeys
// so `{...agentResult}` / Object.keys(result) carry them through as magic
// instead of producing an empty object (the spread-loses-fields trap).
let KNOWN_KEYS = []
function scanKeys(src) {
  const set = new Set()
  for (const m of src.matchAll(/\.([A-Za-z_$][A-Za-z0-9_$]*)\b/g)) set.add(m[1])
  for (const m of src.matchAll(/(?:const|let|var)\s*\{([^}]*)\}\s*=/g)) {
    for (const part of m[1].split(',')) {
      const name = part.split(':')[0].trim().replace(/\.\.\./, '')
      if (/^[A-Za-z_$][A-Za-z0-9_$]*$/.test(name)) set.add(name)
    }
  }
  for (const k of ['then', 'catch', 'finally', 'length', 'name', 'prototype',
    'constructor', 'map', 'filter', 'flatMap', 'forEach', 'reduce', 'sort',
    'slice', 'concat', 'reverse', 'flat', 'join', 'push', 'pop', 'shift',
    'unshift', 'find', 'some', 'every', 'has', 'includes', 'get', 'set',
    'add', 'delete', 'toString', 'valueOf']) set.delete(k)
  return [...set]
}

// The "magic value" returned by agent()/parallel()/pipeline() results. Survives
// arbitrary downstream use and invokes collection callbacks once so nested
// agent() calls inside .map(x => agent(...)) are discovered. Any string
// coercion yields SENTINEL so interpolation points are visible in prompts.
function makeMagic() {
  const t = function () {}
  return new Proxy(t, {
    apply() { return makeMagic() },
    construct() { return makeMagic() },
    has() { return true },
    ownKeys() { return Array.from(new Set([...Reflect.ownKeys(t), ...KNOWN_KEYS])) },
    getOwnPropertyDescriptor(_t, k) {
      if (typeof k === 'string' && KNOWN_KEYS.includes(k) && !Object.prototype.hasOwnProperty.call(t, k)) {
        return { value: makeMagic(), enumerable: true, configurable: true, writable: true }
      }
      return Reflect.getOwnPropertyDescriptor(t, k)
    },
    get(_t, p) {
      if (p === Symbol.toPrimitive) return (hint) => (hint === 'number' ? 0 : SENTINEL)
      if (p === Symbol.iterator) return function* () { yield makeMagic() }
      if (p === Symbol.asyncIterator) return async function* () { yield makeMagic() }
      if (p === 'then' || p === 'catch' || p === 'finally') return undefined
      if (p === 'length') return 1
      if (p === 'toString' || p === 'valueOf') return () => SENTINEL
      if (p === 'map') return (cb) => [cb(makeMagic(), 0, [makeMagic()])]
      if (p === 'flatMap') return (cb) => { const r = cb(makeMagic(), 0, [makeMagic()]); return Array.isArray(r) ? r : [r] }
      if (p === 'filter') return (cb) => { try { cb(makeMagic(), 0, [makeMagic()]) } catch {} return [makeMagic()] }
      if (p === 'forEach') return (cb) => { try { cb(makeMagic(), 0, [makeMagic()]) } catch {} }
      if (p === 'find') return (cb) => { try { cb(makeMagic(), 0, [makeMagic()]) } catch {} return makeMagic() }
      if (p === 'some') return (cb) => { try { cb(makeMagic(), 0, []) } catch {} return false }
      if (p === 'every') return (cb) => { try { cb(makeMagic(), 0, []) } catch {} return true }
      if (p === 'reduce') return (cb, init) => { try { return cb(init !== undefined ? init : makeMagic(), makeMagic(), 0, []) } catch { return makeMagic() } }
      if (p === 'sort' || p === 'slice' || p === 'concat' || p === 'reverse' || p === 'flat') return () => [makeMagic()]
      if (p === 'join') return () => SENTINEL
      if (p === 'push' || p === 'unshift') return () => 1
      if (p === 'pop' || p === 'shift') return () => makeMagic()
      if (p === 'has' || p === 'includes' || p === 'startsWith' || p === 'endsWith') return () => false
      if (p === 'get' || p === 'set' || p === 'add' || p === 'delete') return () => makeMagic()
      if (p === 'toLowerCase' || p === 'toUpperCase' || p === 'trim' || p === 'replace') return () => SENTINEL
      return makeMagic()
    },
  })
}

// ── Instrumentation state ──
const callsBySite = new Map()    // site "L:C" → call record (dedup by call-site)
let currentPhase = null
let total = 0
let pipelineCounter = 0
const frameStack = []            // {kind:'parallel'|'pipeline', pipelineId?}

// Parse the script-side call-site (line:col) out of a stack trace. The script
// runs as an AsyncFunction body, so its frames read
// `at eval (eval at main (…harness.mjs:N:N), <anonymous>:L:C)` — the `<anonymous>:L:C`
// is the script position (the eval-origin path is the harness, which is why we
// must NOT filter by filename). The first such frame is the nearest call-site.
function callSite() {
  const stack = (new Error().stack || '').split('\n').slice(1)
  for (const line of stack) {
    const m = line.match(/<anonymous>:(\d+):(\d+)/)
    if (m) return `${m[1]}:${m[2]}`
  }
  return null
}

// Longest common prefix of two strings — used to generalize a call-site's
// fingerprint/label across its executions. A literal-array fan-out runs the same
// call-site N times with `"head " + literal` → LCP strips the varying literal,
// recovering the general prefix the static scanner would extract. A magic
// (data-driven) fan-out collapses to one execution carrying SENTINELs, which the
// later split() trims. Either way the bound prefix matches all real agents.
function lcp(a, b) {
  if (a === null) return b
  const n = Math.min(a.length, b.length)
  let i = 0
  while (i < n && a[i] === b[i]) i++
  return a.slice(0, i)
}

function record(prompt, opts = {}) {
  if (total >= CAP) throw new CapHit()
  total++
  const site = callSite() || `seq:${total}`
  const resolved = typeof prompt === 'string' ? prompt : ''
  const label = typeof opts.label === 'string' ? opts.label : null

  const existing = callsBySite.get(site)
  if (existing) {
    // same lexical call-site executed again → widen prefixes to the common head
    existing._fp = lcp(existing._fp, resolved)
    if (label !== null) existing._label = lcp(existing._label, label)
    existing.promptLen = Math.max(existing.promptLen, resolved.length)
    return
  }

  const frame = frameStack[frameStack.length - 1]
  const kind = frame ? frame.kind : 'single'
  let pipelineId = null
  for (let i = frameStack.length - 1; i >= 0; i--) {
    if (frameStack[i].pipelineId !== undefined) { pipelineId = frameStack[i].pipelineId; break }
  }
  callsBySite.set(site, {
    site,
    kind,
    pipelineId,
    phase: opts.phase ?? currentPhase ?? null,
    agentType: opts.agentType ?? null,
    schema: !!opts.schema,
    promptLen: resolved.length,
    _fp: resolved,         // running LCP of resolved prompts (finalized at output)
    _label: label,         // running LCP of labels
    _resolved: resolved,   // first execution's full resolved prompt (for display)
  })
}

// ── Injected framework API ──
async function agent(prompt, opts) {
  record(prompt, opts)
  return makeMagic()
}
async function parallel(thunks) {
  const arr = Array.isArray(thunks) ? thunks : [...thunks]
  frameStack.push({ kind: 'parallel' })
  const out = []
  for (const th of arr) {
    try { out.push(typeof th === 'function' ? await th() : await th) }
    catch (e) { if (e instanceof CapHit) throw e; out.push(null) }
  }
  frameStack.pop()
  return out
}
async function pipeline(items, ...stages) {
  const arr = Array.isArray(items) ? items : [...items]
  const pipelineId = pipelineCounter++
  frameStack.push({ kind: 'pipeline', pipelineId })
  const out = []
  for (const item of arr) {
    let v = item
    for (const st of stages) {
      try { v = await st(v, item, 0) }
      catch (e) { if (e instanceof CapHit) throw e; v = null; break }
    }
    out.push(v)
  }
  frameStack.pop()
  return out
}
function phase(title) { currentPhase = title }
function log() {}
async function workflow() { return makeMagic() }

// ── Load + transform + execute ──
const scriptPath = process.argv[2]
const argsVal = process.argv[3] !== undefined ? JSON.parse(process.argv[3]) : undefined

const raw = readFileSync(scriptPath, 'utf8')
// strip the leading `export ` so `export const meta` becomes a local const
const src = raw.replace(/^\s*export\s+(const|let|var|function)\b/m, '$1')
KNOWN_KEYS = scanKeys(src)

const budget = { total: null, spent: () => 0, remaining: () => Infinity }
const AsyncFunction = Object.getPrototypeOf(async function () {}).constructor

async function main() {
  let error = null
  let capped = false
  try {
    const fn = new AsyncFunction(
      'agent', 'parallel', 'pipeline', 'phase', 'log', 'workflow', 'args', 'budget',
      src
    )
    await fn(agent, parallel, pipeline, phase, log, workflow, argsVal, budget)
  } catch (e) {
    if (e instanceof CapHit) capped = true
    else error = (e && e.stack ? e.stack : String(e)).split('\n').slice(0, 3).join(' | ')
  }

  // meta (best effort): eval just the literal object
  let meta = {}
  try {
    const m = src.match(/const\s+meta\s*=\s*(\{[\s\S]*?\n\})/)
    if (m) meta = await new AsyncFunction('return (' + m[1] + ')')()
  } catch {}

  // finalize: cut running-LCP prefixes at the first interpolation sentinel;
  // turn the first execution's full resolved prompt into a readable template
  // (interpolation points → "…") for the UI to show on a node.
  const DISPLAY_MAX = 600
  const calls = [...callsBySite.values()].map((c) => {
    const { _fp, _label, _resolved, ...rest } = c
    let promptResolved = (_resolved ?? '').split(SENTINEL).join('…')
    if (promptResolved.length > DISPLAY_MAX) promptResolved = promptResolved.slice(0, DISPLAY_MAX) + '…'
    return {
      ...rest,
      fingerprint: (_fp ?? '').split(SENTINEL)[0],
      label: _label === null || _label === undefined ? null : _label.split(SENTINEL)[0],
      promptResolved: promptResolved || null,
    }
  })
  process.stdout.write(JSON.stringify({
    ok: error === null,
    capped,
    error,
    name: meta?.name ?? null,
    description: meta?.description ?? null,
    phases: Array.isArray(meta?.phases) ? meta.phases : [],
    calls,
  }))
}
main()
