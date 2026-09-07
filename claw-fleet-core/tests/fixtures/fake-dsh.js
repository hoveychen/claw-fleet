#!/usr/bin/env node
// A stand-in for `dsh web`, used to measure and reproduce Fleet-side queueing
// without spending model credits or depending on a real dsh install.
//
// It speaks the slice of dsh 0.1.2's face `dsh_client` / `dsh_events` use:
//
//   GET  /?token=<launch token>   → 303 + Set-Cookie   (the browser-auth gate)
//   POST /api/<service>/<method>  → one `server-response` per `client-request`
//   WS   /api/remote.mux          → the stream mux (`$events`, `session/follow`)
//
// All three sit behind the cookie: `/api` and the mux answer 401 without it,
// exactly as dsh does, so a Fleet build that skips the exchange fails here the
// same way it would live.
//
// Per-method latency is injected through the environment so a test can make the
// *scan* call slow while the *interactive* call stays fast — the asymmetry the
// starvation claim is about.
//
//   FAKE_DSH_LIST_DELAY_MS      delay before answering session/list   (default 3000)
//   FAKE_DSH_HISTORY_DELAY_MS   delay before answering session/page   (default 50)
//   FAKE_DSH_LOG                append one line per request to this file
//
// Fleet learns the port *and the launch token* by parsing one stdout line, so
// the URL line below must keep the exact
// `dsh web: http://127.0.0.1:<port>/?token=<token>` shape — `parse_launch_line`
// rejects a line missing either half, on the grounds that a port without a
// token means a dsh older than 0.1.2.

const http = require('http');
const fs = require('fs');
const crypto = require('crypto');

const LIST_DELAY = Number(process.env.FAKE_DSH_LIST_DELAY_MS ?? 3000);
const HISTORY_DELAY = Number(process.env.FAKE_DSH_HISTORY_DELAY_MS ?? 50);
const LOG = process.env.FAKE_DSH_LOG;

// Minted per process, like dsh's: the stdout line below is its only exit.
const LAUNCH_TOKEN = crypto.randomBytes(16).toString('hex');
const COOKIE_NAME = 'dsh-session';
const COOKIE_VALUE = crypto.randomBytes(16).toString('hex');

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
  if (method === 'session/list') return LIST_DELAY;
  if (method === 'session/page') return HISTORY_DELAY;
  return 0;
}

function authorized(req) {
  const cookie = req.headers.cookie ?? '';
  return cookie.split(';').some((c) => c.trim() === `${COOKIE_NAME}=${COOKIE_VALUE}`);
}

// One session, shaped like a real `session/list` item, so the desktop UI has
// something to select when this fixture stands in for a slow dsh.
const SESSION_ID = process.env.FAKE_DSH_SESSION_ID ?? 'session-fake-slow';
const SESSION_CWD = process.env.FAKE_DSH_SESSION_CWD ?? process.cwd();

// The log cut this fixture reports on a follow stream's opening snapshot, and
// the newest seq its pages carry. `session/page` refuses a `throughSeq` past the
// real cursor, so the two must agree.
const CURSOR = 8;

function valueFor(method) {
  switch (method) {
    case 'session/list':
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
    case 'session/page':
      // Durable events paired with the transient host-computed view, the way
      // dsh pages them — `history_events` keeps the `event` half only.
      return {
        records: [
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
              seq: CURSOR,
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
      // `settings/describe` (Fleet's health probe), `session/modelCatalog`, … —
      // an empty value is a valid `ok` result.
      return {};
  }
}

const server = http.createServer((req, res) => {
  const url = new URL(req.url, 'http://127.0.0.1');

  // The launch-token exchange: the only way to get the cookie `/api` wants.
  if (req.method === 'GET' && url.pathname === '/') {
    if (url.searchParams.get('token') !== LAUNCH_TOKEN) {
      log('token exchange refused');
      res.writeHead(401).end();
      return;
    }
    log('token exchange');
    res.writeHead(303, {
      location: '/',
      'set-cookie': `${COOKIE_NAME}=${COOKIE_VALUE}; Path=/; HttpOnly; SameSite=Lax`,
    });
    res.end();
    return;
  }

  if (!authorized(req)) {
    log(`401 ${req.url}`);
    res.writeHead(401).end();
    return;
  }

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
    // The endpoint is the URL path; the envelope echoes it in `method`.
    const method = envelope.method ?? url.pathname.replace(/^\/api\//, '');
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

// ── The stream mux ───────────────────────────────────────────────────────────
// Hand-rolled rather than pulled from npm: this fixture must run from a bare
// checkout with nothing installed. Only what Fleet's follower actually does is
// implemented — one text frame in, one text frame out, no fragmentation, no
// extensions.

const WS_GUID = '258EAFA5-E914-47DA-95CA-C5AB0DC85B11';

function textFrame(payload) {
  const data = Buffer.from(payload, 'utf8');
  const head = data.length < 126
    ? Buffer.from([0x81, data.length])
    : Buffer.concat([Buffer.from([0x81, 126]), (() => {
      const b = Buffer.alloc(2);
      b.writeUInt16BE(data.length);
      return b;
    })()]);
  return Buffer.concat([head, data]);
}

/// Pull complete client frames out of a buffer, unmasking as we go.
function drainFrames(buf) {
  const out = [];
  let offset = 0;
  for (;;) {
    if (buf.length - offset < 2) break;
    const opcode = buf[offset] & 0x0f;
    const masked = (buf[offset + 1] & 0x80) !== 0;
    let len = buf[offset + 1] & 0x7f;
    let cursor = offset + 2;
    if (len === 126) {
      if (buf.length - cursor < 2) break;
      len = buf.readUInt16BE(cursor);
      cursor += 2;
    } else if (len === 127) {
      if (buf.length - cursor < 8) break;
      len = Number(buf.readBigUInt64BE(cursor));
      cursor += 8;
    }
    let mask = null;
    if (masked) {
      if (buf.length - cursor < 4) break;
      mask = buf.subarray(cursor, cursor + 4);
      cursor += 4;
    }
    if (buf.length - cursor < len) break;
    const payload = Buffer.from(buf.subarray(cursor, cursor + len));
    if (mask) {
      for (let i = 0; i < payload.length; i += 1) payload[i] ^= mask[i % 4];
    }
    cursor += len;
    offset = cursor;
    out.push({ opcode, payload });
  }
  return { frames: out, rest: buf.subarray(offset) };
}

server.on('upgrade', (req, socket) => {
  const url = new URL(req.url, 'http://127.0.0.1');
  // The mux sits behind the same cookie gate as `/api`: an unauthenticated
  // handshake is answered 401 and never becomes a socket.
  if (url.pathname !== '/api/remote.mux' || !authorized(req)) {
    log(`mux refused ${req.url}`);
    socket.end('HTTP/1.1 401 Unauthorized\r\n\r\n');
    return;
  }
  const accept = crypto
    .createHash('sha1')
    .update((req.headers['sec-websocket-key'] ?? '') + WS_GUID)
    .digest('base64');
  socket.write(
    'HTTP/1.1 101 Switching Protocols\r\n'
      + 'Upgrade: websocket\r\n'
      + 'Connection: Upgrade\r\n'
      + `Sec-WebSocket-Accept: ${accept}\r\n\r\n`,
  );
  log('mux connected');

  let pending = Buffer.alloc(0);
  socket.on('data', (chunk) => {
    pending = Buffer.concat([pending, chunk]);
    const { frames, rest } = drainFrames(pending);
    pending = rest;
    for (const frame of frames) {
      if (frame.opcode === 0x8) {
        socket.end();
        return;
      }
      if (frame.opcode === 0x9) {
        socket.write(Buffer.from([0x8a, 0x00])); // pong
        continue;
      }
      if (frame.opcode !== 0x1) continue;
      let open = {};
      try {
        open = JSON.parse(frame.payload.toString('utf8'));
      } catch {
        continue;
      }
      if (open.type !== 'open' || !open.streamId) continue;
      log(`mux open ${open.endpoint}`);
      const item = (value) => socket.write(
        textFrame(JSON.stringify({ type: 'item', streamId: open.streamId, value })),
      );
      if (open.endpoint === '$events') {
        // The opening item every waterfall answer is addressed with.
        item({ type: 'ready', clientId: 'fake-dsh-client' });
      } else if (open.endpoint === 'session/follow') {
        // The opening snapshot. Only its `cursor` is read, and it is the sole
        // legitimate source of `session/page`'s `throughSeq` — without this
        // frame an interactive history read has no cut to ask for.
        item({ type: 'snapshot', cursor: CURSOR, records: [] });
      }
    }
  });
  socket.on('error', () => socket.destroy());
});

server.listen(0, '127.0.0.1', () => {
  process.stdout.write(
    `dsh web: http://127.0.0.1:${server.address().port}/?token=${LAUNCH_TOKEN}\n`,
  );
});
