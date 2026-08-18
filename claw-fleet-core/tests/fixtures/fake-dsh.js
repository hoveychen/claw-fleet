#!/usr/bin/env node
// A stand-in for `dsh web`, used to measure and reproduce Fleet-side queueing
// without spending model credits or depending on a real dsh install.
//
// It speaks exactly the slice of the `/api` face `dsh_client` uses: a POST per
// method carrying a `client-request`, answered with the matching
// `server-response`. Per-method latency is injected through the environment so
// a test can make the *scan* call slow while the *interactive* call stays fast
// — the asymmetry the starvation claim is about.
//
//   FAKE_DSH_LIST_DELAY_MS      delay before answering session.list   (default 3000)
//   FAKE_DSH_HISTORY_DELAY_MS   delay before answering session.history (default 50)
//   FAKE_DSH_LOG                append one line per request to this file
//
// Fleet learns the port by parsing one stdout line, so the URL line below must
// keep the exact `dsh web: http://127.0.0.1:<port>` shape.

const http = require('http');
const fs = require('fs');

const LIST_DELAY = Number(process.env.FAKE_DSH_LIST_DELAY_MS ?? 3000);
const HISTORY_DELAY = Number(process.env.FAKE_DSH_HISTORY_DELAY_MS ?? 50);
const LOG = process.env.FAKE_DSH_LOG;

const t0 = Date.now();
function log(line) {
  if (!LOG) return;
  try {
    fs.appendFileSync(LOG, `${String(Date.now() - t0).padStart(7)}ms ${line}\n`);
  } catch {
    /* diagnostics only */
  }
}

function delayFor(method) {
  if (method === 'session.list') return LIST_DELAY;
  if (method === 'session.history') return HISTORY_DELAY;
  return 0;
}

// One session, shaped like a real `session.list` item, so the desktop UI has
// something to select when this fixture stands in for a slow dsh.
const SESSION_ID = process.env.FAKE_DSH_SESSION_ID ?? 'session-fake-slow';
const SESSION_CWD = process.env.FAKE_DSH_SESSION_CWD ?? process.cwd();

function valueFor(method) {
  switch (method) {
    case 'session.list':
      return {
        items: [
          {
            sessionId: SESSION_ID,
            cwd: SESSION_CWD,
            updatedAt: Date.now(),
            running: false,
            projections: {
              values: {
                title: '慢 dsh 探针会话',
                tokenUsage: {
                  uncachedInputTokens: 120,
                  cacheReadTokens: 6756,
                  cacheWriteTokens: 1789,
                  outputTokens: 103,
                },
              },
            },
          },
        ],
      };
    case 'session.history':
      // Durable events wrapped the way dsh pages them, in the shapes
      // `dsh_messages::normalize` was written against.
      return {
        events: [
          {
            event: {
              type: 'user/message',
              seq: 7,
              time: Date.now() - 60000,
              data: {
                content: [{ type: 'text', text: '这条消息来自 fake dsh,用来验证对话区能立刻拿到数据' }],
                source: { kind: 'user', rpcId: 'fake-1' },
                role: 'user',
                id: 'fake-user-1',
              },
            },
          },
          {
            event: {
              type: 'assistant/message',
              seq: 8,
              time: Date.now() - 59000,
              data: {
                turn: 1,
                step: 1,
                message: {
                  role: 'assistant',
                  content: [{ type: 'text', text: '收到 —— 对话区没有卡在加载中。' }],
                },
                usage: { inputTokens: 3, outputTokens: 103, cacheReadTokens: 6756, cacheWriteTokens: 1789 },
              },
            },
          },
        ],
        hasMore: false,
      };
    default:
      return {};
  }
}

const server = http.createServer((req, res) => {
  let body = '';
  req.on('data', (chunk) => {
    body += chunk;
  });
  req.on('end', () => {
    let envelope = {};
    try {
      envelope = JSON.parse(body || '{}');
    } catch {
      /* answered as a bad envelope below */
    }
    const method = envelope.method ?? req.url.replace(/^\/api\//, '');
    const rpcId = envelope.rpcId ?? '';
    log(`enter ${method}`);
    setTimeout(() => {
      log(`leave ${method}`);
      res.writeHead(200, { 'content-type': 'application/json' });
      res.end(
        JSON.stringify({
          type: 'server-response',
          rpcId,
          result: { ok: true, value: valueFor(method) },
        }),
      );
    }, delayFor(method));
  });
});

server.listen(0, '127.0.0.1', () => {
  process.stdout.write(`dsh web: http://127.0.0.1:${server.address().port}\n`);
});
