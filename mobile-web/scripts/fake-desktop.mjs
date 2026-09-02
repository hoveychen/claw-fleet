#!/usr/bin/env node
// 一个假的「桌面端」:在 relay 上冒充 Fleet 桌面端,好让移动端的活能在本机端到端
// 验证 —— 尤其是**多设备**那些只有两台机器同时在册才会暴露的行为(合并收件箱、
// 答复发给哪一台、跨设备同 id 不串卡、连接策略)。
//
// 它做的就是真桌面端在 relay 上做的那几件事:
//   1. 用 HKDF 从配对 secret 派生 channelToken(拿去 auth)与 encKey(封/解 msg);
//   2. 以 agent 身份接入某个 channel;
//   3. 答 pending_snapshot / today_usage / decision_answer / tail / tail_delta;
//   4. 推 sessions 与 decision_created。
//
// 协议细节的地面真相在 fleet-relay/src/frames.rs(信封)、
// claw-fleet-core/src/relay_crypto.rs(HKDF 参数)与 mobile-web/src/relay.ts
// (业务帧)。改协议时这三处与本文件要一起改。
//
// ── 用法 ────────────────────────────────────────────────────────────────────
//
//   # 1. 本机起一个 relay(仓库根目录)
//   RELAY_PORT=18099 RELAY_DATA_DIR=/tmp/relay-data cargo run -p fleet-relay
//
//   # 2. 起两个假桌面端,各用一个 64 位十六进制 secret
//   node mobile-web/scripts/fake-desktop.mjs \
//     --relay ws://127.0.0.1:18099 --secret $(printf 'a%.0s' {1..64}) --label mac --cards 2
//   node mobile-web/scripts/fake-desktop.mjs \
//     --relay ws://127.0.0.1:18099 --secret $(printf 'b%.0s' {1..64}) --label linux --cards 1
//
//   # 3. 起 mobile-web 的 dev server,并把两台设备写进设备簿(浏览器控制台里):
//   #    localStorage.setItem('fleet-devices', JSON.stringify({devices:[
//   #      {id:'d1',label:'公司 Mac',   secret:'aaaa…',relayBase:'http://127.0.0.1:18099',addedAt:1},
//   #      {id:'d2',label:'家里 Linux', secret:'bbbb…',relayBase:'http://127.0.0.1:18099',addedAt:2}],
//   #      activeId:'d1'}))
//   #    然后刷新页面。
//
// 两台故意可以用**相同的卡 id**(默认就是 g1、g2…),因为「跨设备同 id 不互相
// 覆盖」正是最容易写错、也最难靠单测覆盖的那一条。
//
// 只用 Node 内建能力:WebSocket 全局(Node 22+)与 node:crypto,不引任何依赖。

import crypto from "node:crypto";

// ── 参数 ─────────────────────────────────────────────────────────────────────

function parseArgs(argv) {
  const out = { relay: "ws://127.0.0.1:18099", label: "fake", cards: 1, sessions: 3 };
  for (let i = 0; i < argv.length; i += 2) {
    const key = argv[i]?.replace(/^--/, "");
    const value = argv[i + 1];
    if (!key || value === undefined) continue;
    if (key === "cards" || key === "sessions") out[key] = Number(value);
    else out[key] = value;
  }
  return out;
}

const args = parseArgs(process.argv.slice(2));
if (!args.secret) {
  console.error(
    "usage: fake-desktop.mjs --secret <pairing secret> [--relay ws://host:port] " +
      "[--label mac] [--cards 2] [--sessions 3]",
  );
  process.exit(2);
}

// ── 端到端加密(必须与 claw-fleet-core/src/relay_crypto.rs 逐字节一致)─────────

const SALT = Buffer.from("fleet-relay/hkdf/v1");
const INFO_CHANNEL = Buffer.from("fleet-relay/channel/v1");
const INFO_ENC = Buffer.from("fleet-relay/enc/v1");
const AAD = Buffer.from("fleet-relay/msg/v1");

const secretBytes = Buffer.from(args.secret);
/** relay 只见到这个 —— 它是 secret 的单向像,relay 永远学不到 secret 本身。 */
const channelToken = Buffer.from(
  crypto.hkdfSync("sha256", secretBytes, SALT, INFO_CHANNEL, 32),
).toString("hex");
const encKey = Buffer.from(crypto.hkdfSync("sha256", secretBytes, SALT, INFO_ENC, 32));

function seal(json) {
  const iv = crypto.randomBytes(12);
  const cipher = crypto.createCipheriv("aes-256-gcm", encKey, iv);
  cipher.setAAD(AAD);
  const ct = Buffer.concat([cipher.update(Buffer.from(json, "utf8")), cipher.final()]);
  return {
    enc: "box",
    iv: iv.toString("base64"),
    ct: Buffer.concat([ct, cipher.getAuthTag()]).toString("base64"),
  };
}

function open(sealed) {
  const raw = Buffer.from(sealed.ct, "base64");
  const decipher = crypto.createDecipheriv(
    "aes-256-gcm",
    encKey,
    Buffer.from(sealed.iv, "base64"),
  );
  decipher.setAAD(AAD);
  decipher.setAuthTag(raw.subarray(raw.length - 16));
  return Buffer.concat([
    decipher.update(raw.subarray(0, raw.length - 16)),
    decipher.final(),
  ]).toString("utf8");
}

// ── 这台「桌面端」的家当 ─────────────────────────────────────────────────────

const label = args.label;
/** 手机端用它区分「谁回的快照」(decisionReconcile.ts 的可信 agent 判定)。 */
const agent = { host: label, home: `/home/${label}`, ver: "fake", pid: process.pid };

/** 形状对齐 mobile-web/src/types.ts 的 SessionInfo(relay 白名单投影)。
 *  少字段的后果不是显示不全,而是被任务页整条筛掉 —— `entrypoint` 与
 *  `fleetSpawned` 尤其关键(types.ts::isFleetOwnedTask)。 */
const sessions = Array.from({ length: args.sessions }, (_, i) => ({
  id: `${label}-s${i + 1}`,
  workspacePath: `/repos/${label}`,
  workspaceName: label,
  aiTitle: `${label} 的会话 ${i + 1}`,
  slug: `${label}-s${i + 1}`,
  status: "idle",
  isSubagent: false,
  lastMessagePreview: `来自 ${label} 的最后一条消息`,
  lastActivityMs: Date.now() - i * 60_000,
  createdAtMs: Date.now() - 3_600_000,
  jsonlPath: `/home/${label}/.claude/projects/x/${label}-s${i + 1}.jsonl`,
  agentSource: "claude",
  fleetSpawned: true,
  entrypoint: "claw-fleet-newsession",
  procAlive: i === 0,
  totalCostUsd: 0.1 * (i + 1),
}));

/** 形状对齐 generated/types.ts 的 GuardRequest。缺一个 `riskTags` 数组就会在
 *  渲染时抛异常,而 React 会把整棵树卸掉 —— 页面一片空白、控制台无错。 */
const cards = Array.from({ length: args.cards }, (_, i) => ({
  id: `g${i + 1}`, // 刻意与其他实例同号:跨设备撞 id 正是要验的
  sessionId: sessions[0]?.id ?? `${label}-s1`,
  workspaceName: label,
  aiTitle: `${label} 的任务`,
  toolName: "Bash",
  command: `echo hello-from-${label}-${i + 1}`,
  commandSummary: `echo hello-from-${label}-${i + 1}`,
  riskTags: [],
  timestamp: new Date().toISOString(),
}));

// ── 接入 ─────────────────────────────────────────────────────────────────────

const ws = new WebSocket(`${args.relay.replace(/\/$/, "")}/ws`);
const send = (frame) => ws.send(JSON.stringify(frame));
const sendPayload = (payload) => send({ type: "msg", payload: seal(JSON.stringify(payload)) });

ws.addEventListener("open", () => {
  // auth 帧带的是 channelToken,不是配对 secret。
  send({ type: "auth", role: "agent", secret: channelToken });
});

ws.addEventListener("message", (event) => {
  const frame = JSON.parse(String(event.data));

  if (frame.type === "authed") {
    console.log(`[${label}] authed · clients=${frame.clients}`);
    const push = () => {
      sendPayload({ event: "sessions", sessions });
      for (const request of cards) {
        sendPayload({ event: "decision_created", kind: "guard", request });
      }
    };
    push();
    // 真桌面端在客户端加入时推一次;这里图省事,每 5 秒重推 —— 手机后连上来也能
    // 拿到列表。答掉的卡已从 `cards` 移除,所以不会复活。
    setInterval(push, 5_000);
    return;
  }

  if (frame.type !== "msg") return;
  const payload = JSON.parse(open(frame.payload));

  if (payload.event === "client_hello") {
    console.log(
      `[${label}] hello from ${payload.clientId?.slice(0, 8)} pushSubscribed=${payload.pushSubscribed}`,
    );
    return;
  }
  if (payload.event === "req") {
    const { req_id, method, params = {} } = payload;
    console.log(`[${label}] req ${method} ${JSON.stringify(params).slice(0, 140)}`);
    let data = {};
    if (method === "decision_answer") {
      // 手机答复走的是 req/reply(relay.ts::answerViaReq),不是老的 fire-and-forget
      // `answer` 事件。收到就销卡并广播 resolved。
      console.log(`[${label}] ANSWER ${params.kind} ${params.id}`);
      const at = cards.findIndex((c) => c.id === params.id);
      if (at >= 0) cards.splice(at, 1);
      sendPayload({ event: "decision_resolved", kind: params.kind, id: params.id });
    } else if (method === "pending_snapshot") {
      data = { agent, guard: cards };
    } else if (method === "today_usage") {
      data = {
        date: new Date().toISOString().slice(0, 10),
        inputTokens: 1_000,
        outputTokens: 100,
        costUsd: 1.5,
        agentCostUsd: 1,
        fleetCostUsd: 0.5,
        sessionCount: sessions.length,
      };
    } else if (method === "tail" || method === "tail_delta") {
      // 详情页要的是 transcript 行。空列表足够验「请求发到了哪一台」。
      data = method === "tail" ? [] : { lines: [], offset: 0 };
    }
    sendPayload({ event: "reply", req_id, ok: true, data, handle_ms: 3 });
  }
});

ws.addEventListener("error", () => console.error(`[${label}] websocket error`));
ws.addEventListener("close", () => {
  console.log(`[${label}] closed`);
  process.exit(0);
});
