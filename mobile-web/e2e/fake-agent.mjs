// Fake desktop agent for mobile-web E2E against a local fleet-relay.
// Speaks the mobile_relay.rs protocol: auth as agent role, replies to `req`
// payloads, pushes slimmed sessions snapshots. v2.1 additions: `tail_delta`
// byte-offset incremental method + a log that grows every 3s so the browser
// can be observed appending lines instead of full-refreshing.
//
// Usage: node fake-agent.mjs  (RELAY_URL / SECRET via env, defaults below)

const RELAY_URL = process.env.RELAY_URL || "ws://127.0.0.1:18099/ws";
const SECRET = process.env.SECRET || "e2etestsecret123";

const now = Date.now();

// ── In-memory jsonl "files" keyed by path (tail / tail_delta read these) ────
const logs = new Map(); // path -> { lines: string[], bytes: number }

function pushLine(path, obj) {
  const entry = logs.get(path) ?? { lines: [], bytes: 0 };
  const line = JSON.stringify(obj);
  entry.lines.push(line);
  entry.bytes += Buffer.byteLength(line, "utf8") + 1; // +\n like a real file
  logs.set(path, entry);
}

function sliceFromOffset(path, offset) {
  const entry = logs.get(path) ?? { lines: [], bytes: 0 };
  if (offset == null || offset >= entry.bytes) {
    return { lines: [], newOffset: entry.bytes };
  }
  // Walk lines accumulating byte offsets; return the ones past `offset`.
  let pos = 0;
  const out = [];
  for (const line of entry.lines) {
    const len = Buffer.byteLength(line, "utf8") + 1;
    if (pos >= offset) out.push(JSON.parse(line));
    pos += len;
  }
  return { lines: out, newOffset: entry.bytes };
}

const P1 = "/fake/ws-alpha/sess-1.jsonl";
let uuidSeq = 0;
function msgLine(type, text, extra = {}) {
  return {
    type,
    uuid: `u${++uuidSeq}`,
    timestamp: new Date().toISOString(),
    message:
      type === "user"
        ? { role: "user", content: [{ type: "text", text }] }
        : { role: "assistant", content: [{ type: "text", text }] },
    ...extra,
  };
}
pushLine(P1, msgLine("user", "帮我修一下移动端的 bug"));
pushLine(P1, msgLine("assistant", "好的，我先看看 **TasksView** 的代码。"));
pushLine(P1, msgLine("assistant", "找到了，`markBucket` 的语义不对。"));

// ── Sessions snapshot (already-slim field set, mirrors SNAPSHOT_FIELDS) ─────
// markBucket semantics under test: unmarked => pending bucket.
// s1 unmarked working, s2 unmarked idle, s3 mark=pending, s4 mark=done
// => segment counts: 进行中 3, 已完成 1.
const sessions = [
  {
    id: "sess-1",
    workspacePath: "/fake/ws-alpha",
    workspaceName: "ws-alpha",
    aiTitle: "修移动端 markBucket bug",
    slug: "fix-markbucket",
    status: "executing",
    isSubagent: false,
    lastMessagePreview: "找到了，markBucket 的语义不对。",
    lastActivityMs: now - 30_000,
    createdAtMs: now - 3_600_000,
    jsonlPath: P1,
    model: "claude-fable-5",
    agentSource: "claude",
    pid: 11111,
    pidPrecise: true,
    entrypoint: "fleet-spawn",
    procAlive: true,
  },
  {
    id: "sess-2",
    workspacePath: "/fake/ws-alpha",
    workspaceName: "ws-alpha",
    aiTitle: "调研 IndexedDB 补水",
    slug: "idb-research",
    status: "idle",
    isSubagent: false,
    lastMessagePreview: "调研完成，等待下一步。",
    lastActivityMs: now - 600_000,
    createdAtMs: now - 7_200_000,
    jsonlPath: "/fake/ws-alpha/sess-2.jsonl",
    model: "claude-sonnet-5",
    agentSource: "claude",
    procAlive: false,
  },
  {
    id: "sess-3",
    workspacePath: "/fake/ws-beta",
    workspaceName: "ws-beta",
    aiTitle: "重构 relay 心跳",
    slug: "relay-heartbeat",
    status: "waitingInput",
    isSubagent: false,
    lastMessagePreview: "有两个方案，需要老板拍板。",
    lastActivityMs: now - 120_000,
    createdAtMs: now - 5_400_000,
    jsonlPath: "/fake/ws-beta/sess-3.jsonl",
    model: "claude-fable-5",
    agentSource: "claude",
    userMark: "pending",
    pid: 33333,
    pidPrecise: true,
    entrypoint: "fleet-spawn",
    procAlive: true,
  },
  {
    id: "sess-4",
    workspacePath: "/fake/ws-beta",
    workspaceName: "ws-beta",
    aiTitle: "发布 v2 到线上",
    slug: "deploy-v2",
    status: "idle",
    isSubagent: false,
    lastMessagePreview: "部署完成，healthz 正常。",
    lastActivityMs: now - 1_800_000,
    createdAtMs: now - 10_800_000,
    jsonlPath: "/fake/ws-beta/sess-4.jsonl",
    model: "claude-opus-4-8",
    agentSource: "claude",
    userMark: "done",
    procAlive: false,
  },
];

// One pending guard decision so the decisions tab has content.
const guardDecision = {
  id: "guard-1",
  sessionId: "sess-1",
  workspaceName: "ws-alpha",
  aiTitle: "修移动端 markBucket bug",
  command: "git push origin main",
  riskTags: ["git-push"],
  structuredCommand: {
    leaves: [
      {
        argv: ["git", "push", "origin", "main"],
        triggering: true,
        already_allowed: false,
      },
    ],
    connectors: [],
  },
};

// ── Wiki fixtures (wiki_list / wiki_file) ────────────────────────────────────
const wikiDocs = [
  {
    slug: "arch/overview",
    title: "架构总览",
    kind: "markdown",
    entry: "overview.md",
    workspacePath: "/fake/ws-alpha",
    workspaceName: "ws-alpha",
    createdMs: now - 86_400_000,
    updatedMs: now - 3_600_000,
    currentVersion: "20260712-100000",
    versions: [
      { id: "20260712-100000", publishedMs: now - 3_600_000, sizeBytes: 512, fileCount: 1, sourcePath: "/x/overview.md" },
      { id: "20260711-090000", publishedMs: now - 90_000_000, sizeBytes: 480, fileCount: 1, sourcePath: "/x/overview.md" },
    ],
  },
  {
    slug: "perf-report",
    title: "性能分析报告",
    kind: "htmlDir",
    entry: "index.html",
    workspacePath: "/fake/ws-beta",
    workspaceName: "ws-beta",
    createdMs: now - 172_800_000,
    updatedMs: now - 7_200_000,
    currentVersion: "20260712-080000",
    versions: [
      { id: "20260712-080000", publishedMs: now - 7_200_000, sizeBytes: 2048, fileCount: 2, sourcePath: "/x/report" },
    ],
  },
];

const wikiFiles = {
  "arch/overview:overview.md":
    { mime: "text/markdown; charset=utf-8", body: "# 架构总览\n\n这是 **移动端** 知识库渲染测试。\n\n- 列表\n- [[perf-report|跳到性能报告]]\n\n| 模块 | 状态 |\n|---|---|\n| relay | ✅ |\n" },
  "perf-report:index.html":
    { mime: "text/html; charset=utf-8", body: `<!doctype html><html><head><link rel="stylesheet" href="app.css"></head><body><h1>性能报告</h1><p class="ok">资源(css)已通过 relay 重写为 blob 加载。</p></body></html>` },
  "perf-report:app.css":
    { mime: "text/css; charset=utf-8", body: `body{font-family:-apple-system,sans-serif;padding:16px}.ok{color:#0a7d32;font-weight:600}` },
};

// ── Request handling ─────────────────────────────────────────────────────────
function serveRequest(method, params) {
  switch (method) {
    case "pending_snapshot":
      return {
        guard: [guardDecision],
        elicitation: [],
        fleetAsk: [],
        planApproval: [],
        permissionPrompt: [],
      };
    case "tail": {
      const entry = logs.get(params.path) ?? { lines: [] };
      const n = params.n ?? 120;
      return entry.lines.slice(-n).map((l) => JSON.parse(l));
    }
    case "tail_delta":
      return sliceFromOffset(params.path, params.offset ?? null);
    case "live_thinking":
      return null;
    case "task_plans":
      return [];
    case "session_decisions":
      return [];
    case "workflow_trees":
      return [];
    case "handoff_chain":
      return null;
    case "token_breakdown":
      return {
        totalsUsage: {
          inputTokens: 120_000,
          outputTokens: 45_000,
          cacheCreationTokens: 300_000,
          cacheReadTokens: 2_400_000,
        },
        totalsEstimatedCostUsd: 3.21,
        subagents: [],
      };
    case "guard_analyze":
      return { analysis: params.lang === "en" ? "**Low risk**: push to main." : "**低风险**：推送到 main。" };
    case "session_mark": {
      const s = sessions.find((x) => x.id === params.sessionId);
      if (s) {
        if (params.mark) s.userMark = params.mark;
        else delete s.userMark;
      }
      queueMicrotask(pushSessions);
      return {};
    }
    case "session_read": {
      const t = Date.now();
      for (const item of params.items ?? []) {
        const s = sessions.find((x) => x.id === item.sessionId);
        if (s) s.lastReadMs = t;
      }
      queueMicrotask(pushSessions);
      return {};
    }
    case "wiki_list":
      return wikiDocs;
    case "wiki_file": {
      const key = `${params.slug}:${params.relpath}`;
      const file = wikiFiles[key];
      if (!file) throw new Error(`fake-agent: no wiki file ${key}`);
      return { mime: file.mime, base64: Buffer.from(file.body, "utf8").toString("base64") };
    }
    case "wiki_search": {
      const q = String(params.query ?? "").trim().toLowerCase();
      if (q.length < 2) return [];
      const hits = [];
      for (const d of wikiDocs) {
        const meta = `${d.title} ${d.slug} ${d.workspaceName}`.toLowerCase();
        if (meta.includes(q)) {
          hits.push({ slug: d.slug, field: "meta", snippet: "" });
          continue;
        }
        const body = (wikiFiles[`${d.slug}:${d.entry}`]?.body ?? "").toLowerCase();
        const pos = body.indexOf(q);
        if (pos >= 0) {
          const raw = wikiFiles[`${d.slug}:${d.entry}`].body;
          const snippet = raw.slice(Math.max(0, pos - 20), pos + 40).replace(/\s+/g, " ").trim();
          hits.push({ slug: d.slug, field: "content", snippet: `…${snippet}…` });
        }
      }
      return hits;
    }
    case "wiki_export": {
      const d = wikiDocs.find((x) => x.slug === params.slug);
      if (!d) throw new Error(`fake-agent: no wiki doc ${params.slug}`);
      const base = d.slug.split("/").pop();
      if (d.kind === "markdown" || d.kind === "html") {
        const file = wikiFiles[`${d.slug}:${d.entry}`];
        const ext = d.kind === "markdown" ? "md" : "html";
        return {
          filename: `${base}.${ext}`,
          mime: file.mime,
          base64: Buffer.from(file.body, "utf8").toString("base64"),
        };
      }
      // htmlDir → a tiny stand-in zip payload (magic bytes suffice for the test).
      return {
        filename: `${base}.zip`,
        mime: "application/zip",
        base64: Buffer.from([0x50, 0x4b, 0x03, 0x04, 0, 0]).toString("base64"),
      };
    }
    default:
      throw new Error(`fake-agent: unhandled method ${method}`);
  }
}

// ── WS loop ──────────────────────────────────────────────────────────────────
let ws;
function send(frame) {
  if (ws?.readyState === WebSocket.OPEN) ws.send(JSON.stringify(frame));
}
function pushSessions() {
  send({ type: "msg", payload: { event: "sessions", sessions } });
}

function connect() {
  ws = new WebSocket(RELAY_URL);
  ws.onopen = () => {
    ws.send(JSON.stringify({ type: "auth", role: "agent", secret: SECRET }));
  };
  ws.onmessage = (ev) => {
    let frame;
    try {
      frame = JSON.parse(String(ev.data));
    } catch {
      return;
    }
    if (frame.type === "authed") {
      console.log(`[fake-agent] authed, clients=${frame.clients}`);
      pushSessions();
    } else if (frame.type === "presence") {
      console.log(`[fake-agent] presence clients=${frame.clients}`);
      if (frame.clients > 0) pushSessions(); // 新客户端强推
    } else if (frame.type === "msg") {
      const p = frame.payload ?? {};
      if (p.event === "req") {
        let reply;
        try {
          reply = { event: "reply", req_id: p.req_id, ok: true, data: serveRequest(p.method, p.params ?? {}) };
        } catch (e) {
          reply = { event: "reply", req_id: p.req_id, ok: false, error: String(e.message ?? e) };
        }
        console.log(`[fake-agent] req ${p.method} -> ${reply.ok ? "ok" : reply.error}`);
        send({ type: "msg", payload: reply });
      } else if (p.event === "answer") {
        console.log(`[fake-agent] answer kind=${p.kind} id=${p.id}`, JSON.stringify(p));
        send({ type: "msg", payload: { event: "decision_resolved", kind: p.kind, id: p.id } });
      }
    } else if (frame.type === "error") {
      console.error("[fake-agent] relay error:", frame.message);
    }
  };
  ws.onclose = () => {
    console.log("[fake-agent] disconnected, retrying in 1s");
    setTimeout(connect, 1000);
  };
  ws.onerror = () => ws.close();
}
connect();

// Grow sess-1's log every 3s so tail_delta has fresh lines to serve.
let growSeq = 0;
setInterval(() => {
  growSeq += 1;
  pushLine(P1, msgLine("assistant", `增量消息 #${growSeq}：正在继续修复……`));
}, 3000);

console.log("[fake-agent] started");
