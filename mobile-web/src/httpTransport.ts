// 同源 HTTP 传输层：`fleet webui` 从同一个端口发出这张页面和它要的数据路由,
// 所以「后端」就在 `window.location.origin` 上,不需要中转、配对密钥或
// WebSocket。
//
// **本文件不得 import relay.ts,这是硬约束不是风格偏好。** `relay.ts` 在模块
// 加载时就会执行 `resolveRelayBase()` 去解析一个 relay 地址;浏览器构建只要
// 有一条 import 链碰到它,就会带着一个自己永远不会用的 relay 客户端上路,而
// 「webui 不依赖 relay」正是这套东西存在的前提。共用的部分都在 transport.ts
// (错误分层、超时预算、接口本身),那个文件是干净的。
// `httpTransport.relay-free.test.ts` 把这条约束钉成了测试。
//
// 两条通道,对应服务端两个既有出口:
//
//   - **请求** → `POST /mobile_rpc`(claw-fleet-core/src/routes.rs),桥到
//     `mobile_relay::serve_request` —— 与 relay 帧打进去的是同一张方法表,
//     所以两个传输层拿到的答案逐字节相同。
//   - **推送** → `GET /events` 的 SSE。hooks_server 那个 2 秒轮询循环本来就
//     同时喂 relay 帧和 SSE 事件,喂的是同一个 `req` 序列化出来的 JSON。
//
// 关于 SSE 还有一件不显眼但要命的事:它同时是**消费者存在**的信号。
// hooks_server 只在有 SSE 客户端(或手机在 relay 上)时才写
// `~/.fleet/consumer.heartbeat`,而 `fleet guard` / `fleet elicitation` /
// `fleet mcp` 在阻塞前会查这个心跳,查不到就直接放行到 Claude Code 自己的
// 终端提示。所以「只轮询不建流」的实现不只是收不到卡 —— 它会让每一个本该
// 弹到手机上的问题悄悄退回终端。必须是真的 EventSource。

import type { DecisionKind, SessionInfo } from "./types";
import { TransportError, type FleetTransport, type TransportHandlers } from "./transport";

/** SSE 事件名前缀 → 决策卡种类。
 *
 *  服务端两行紧挨着发:`sse.broadcast("guard-request", &json)` 与
 *  `publish_decision_created("guard", v, …)`(hooks_server/mod.rs)。所以这张
 *  表不是猜的映射,而是把那对孪生调用的命名约定写下来。 */
const DECISION_KINDS: DecisionKind[] = [
  "guard",
  "elicitation",
  "fleet-ask",
  "plan-approval",
  "permission-prompt",
  "a2ui-render",
];

/** 请求默认超时。与 relay 实现取同一个值:超时预算描述的是「一个人愿意等多久」,
 *  跟字节走哪条路无关。 */
const REQUEST_TIMEOUT_MS = 15_000;

export interface HttpTransportOptions {
  /** 数据路由的前缀。同源部署下留空即可 —— `/mobile_rpc` 是根路径,页面挂在
   *  `/m/` 下也能正确解析。测试用它指向替身。
   *
   *  设备簿里那种「HTTP 直连主机」给的是**绝对地址**(跨源),此时服务端必须
   *  回 CORS 头,否则浏览器直接拦(见 claw-fleet-core 的 hooks_server)。 */
  baseUrl?: string;
  /** 访问令牌。跨源直连一台带 token 门的主机时必需。
   *
   *  两条通道的带法不同,而这不是风格问题:
   *  - 请求走 `Authorization: Bearer <t>` 头;
   *  - SSE 走 `?token=<t>` 查询参数 —— **EventSource 不能设置请求头**,这是
   *    浏览器 API 的硬限制。服务端为此同时认这两种(hooks_server/mod.rs 的
   *    auth check 里明写了「后者是给 SSE 用的」)。
   *
   *  代价是 token 会出现在 SSE 那条 URL 里,也就可能落进服务端访问日志。这是
   *  EventSource 逼出来的取舍,不是我们选的 —— 换成 fetch+ReadableStream 手写
   *  SSE 才能避免,而那要重写整条流式通道与重连语义。 */
  token?: string | null;
  fetchImpl?: typeof fetch;
  eventSourceImpl?: typeof EventSource;
}

/** `EventSource.readyState` 的终态。写字面量而不是取 `EventSource.CLOSED`,是因为
 *  这个类可以由 `eventSourceImpl` 注入(测试替身、非浏览器宿主),那时静态常量不
 *  一定存在;数值本身由规范钉死。 */
const CLOSED = 2;

/** 重开一条流的退避区间。与 relay.ts 取同一组数(1s 起、翻倍、封顶 15s):
 *  「一条链路多久值得再试一次」跟它是 WebSocket 还是 SSE 无关。 */
const RECONNECT_BASE_MS = 1_000;
const RECONNECT_MAX_MS = 15_000;

export class HttpTransport implements FleetTransport {
  private stream: EventSource | null = null;
  private connected = false;
  private closed = false;
  private reconnectDelay = RECONNECT_BASE_MS;
  private reconnectTimer: number | undefined;

  constructor(
    private readonly handlers: TransportHandlers,
    private readonly opts: HttpTransportOptions = {},
  ) {}

  private get base(): string {
    return (this.opts.baseUrl ?? "").replace(/\/$/, "");
  }

  /** SSE 那条 URL 上的 token 查询串。没有 token 时是空串。 */
  private tokenQuery(): string {
    const token = this.opts.token;
    return token ? `?token=${encodeURIComponent(token)}` : "";
  }

  /** 请求头。跨源直连带 token 的主机时加上 Bearer;同源部署下没有 token,
   *  头就只有 Content-Type,与从前逐字节相同。 */
  private headers(): Record<string, string> {
    const h: Record<string, string> = { "Content-Type": "application/json" };
    if (this.opts.token) h.Authorization = `Bearer ${this.opts.token}`;
    return h;
  }

  connect(): void {
    if (this.stream) return;
    this.closed = false;
    const ES = this.opts.eventSourceImpl ?? globalThis.EventSource;
    const stream = new ES(`${this.base}/events${this.tokenQuery()}`);
    this.stream = stream;

    stream.onopen = () => {
      if (this.closed) return;
      this.connected = true;
      // 握手成功 = 这条链路此刻是通的,下一次断开该从最短的退避重来。
      this.reconnectDelay = RECONNECT_BASE_MS;
      this.handlers.onStatus?.(true);
      // 同源部署里「主机在线」与「这张页面加载出来了」是同一件事:发出这张
      // 页面的进程就是回答 /mobile_rpc 的那一个。没有第三方中转可掉线,所以
      // 这个信号一旦为真就不会独立地变假 —— 它跟着连接本身走。
      this.handlers.onAgentOnline?.(true);
    };

    // 首屏 catch-up。**不能只等 SSE**:服务端那条 `sessions-updated` 只在
    // sessions 变化时广播(hooks_server/mod.rs 的 `if sessions_changed`),所以
    // 一个后接入的客户端,只要连上之后什么都没变,就永远收不到任何帧,任务页
    // 就永远停在「正在接收首屏快照」。relay 路径为此专门多了一个
    // `|| mobile_clients > prev_mobile_clients` 条件;SSE 没有等价物。
    //
    // 修在这边而不是让服务端为新客户端重推:SSE 是广播给所有连接的,为一个新
    // 客户端重推全量会打扰其他所有客户端。mount 时自己拉一次本来就是 HTTP
    // 客户端该做的事 —— 桌面 webui 的 liveProxy 一直这么干。
    void this.catchUpSessions();

    stream.onerror = () => {
      if (this.closed) return;
      if (this.connected) {
        this.connected = false;
        this.handlers.onStatus?.(false);
        this.handlers.onAgentOnline?.(false);
        this.handlers.onReconnect?.();
      }
      // EventSource 的内建重试**只覆盖网络层断开**:那时 readyState 停在
      // CONNECTING,浏览器自己会再握手,我们插一手只会开出第二条并行的流(服务端
      // 多算一个消费者、事件重复投递)。
      //
      // 但服务端回非 200(弱网时网关的 502/504)或 Content-Type 不对时,规范要求
      // UA "fail the connection" —— readyState 变 CLOSED 且**永不重试**。历史实现
      // 把重连整个托付给了那份契约,于是这条流死在那儿,而 connect() 开头的
      // `if (this.stream) return` 又让谁都重建不了它:只剩刷新页面。同一个洞还有
      // 第二个入口 —— 从没连上过的第一次握手(页面在断网时打开)走的也是这里,
      // 旧代码的 `!this.connected` 早退把它一并吃掉了。
      if (stream.readyState === CLOSED) this.scheduleReopen();
    };

    for (const kind of DECISION_KINDS) {
      stream.addEventListener(`${kind}-request`, (e) => {
        const req = parseJson((e as MessageEvent).data);
        if (req !== undefined) this.handlers.onDecisionCreated?.(kind, req);
      });
      stream.addEventListener(`${kind}-dismissed`, (e) => {
        // 服务端发的是 `serde_json::to_string(id)` —— 一个裸的 JSON 字符串,
        // 不是对象。按对象解会静默丢掉每一次「卡片已解决」,卡片就永远留在
        // 手机上。
        const id = parseJson((e as MessageEvent).data);
        if (typeof id === "string") this.handlers.onDecisionResolved?.(kind, id);
      });
    }

    stream.addEventListener("sessions-updated", (e) => {
      const sessions = parseJson((e as MessageEvent).data);
      if (!Array.isArray(sessions)) return;
      this.handlers.onSessions?.(sessions as SessionInfo[]);
      // 服务端这条 SSE 只发全量。relay 那边有 `sessions_delta`,这里没有 ——
      // 报 "delta" 会让 UI 显示一条并不存在的增量链路。
      this.handlers.onSessionsKind?.("full");
    });
  }

  /** 丢掉这条已死的流,退避之后重开一条。
   *
   *  抖动 0–30% 是为了让「网络恢复」那一刻的 N 个标签页不要挤在同一毫秒。 */
  private scheduleReopen(): void {
    if (this.closed || this.reconnectTimer !== undefined) return;
    const delay = this.reconnectDelay * (1 + Math.random() * 0.3);
    this.reconnectDelay = Math.min(this.reconnectDelay * 2, RECONNECT_MAX_MS);
    this.reconnectTimer = globalThis.setTimeout(() => {
      this.reconnectTimer = undefined;
      if (this.closed) return;
      // connect() 开头会因为 this.stream 非空早退,所以必须先把死流摘掉。
      this.stream?.close();
      this.stream = null;
      this.connect();
    }, delay) as unknown as number;
  }

  /** 拉一次 sessions 全量,补上 SSE 不会为新客户端重放的那一帧。
   *
   *  失败就安静算了:SSE 那条路仍然可能把数据送到,而这里抛出去只会把一次
   *  可恢复的首屏缺失变成整个连接失败。 */
  private async catchUpSessions(): Promise<void> {
    const fetchImpl = this.opts.fetchImpl ?? globalThis.fetch;
    try {
      const res = await fetchImpl(`${this.base}/sessions`);
      if (!res.ok) return;
      const sessions = await res.json();
      // 已经被 close() 或被一帧真正的 SSE 抢先都无所谓 —— 两边给的都是全量,
      // 后到的覆盖先到的，结果一致。
      if (this.closed || !Array.isArray(sessions)) return;
      this.handlers.onSessions?.(sessions as SessionInfo[]);
      this.handlers.onSessionsKind?.("full");
    } catch {
      // 见上:安静降级。
    }
  }

  close(): void {
    this.closed = true;
    if (this.reconnectTimer !== undefined) {
      globalThis.clearTimeout(this.reconnectTimer);
      this.reconnectTimer = undefined;
    }
    this.stream?.close();
    this.stream = null;
    if (this.connected) {
      this.connected = false;
      this.handlers.onStatus?.(false);
    }
  }

  /** 同源下没有「设备列表」这回事:主机不为这张页面维护会过期的登记,所以
   *  也没有什么需要提前摘掉。空实现是诚实的,不是偷懒。 */
  sayGoodbye(): void {}

  get isAuthed(): boolean {
    return this.connected;
  }

  /** 显示「我连到哪」。同源时是发出这张页面的那个 origin;直连时是那台主机的
   *  地址(去掉 scheme 与末尾斜杠,与 relay 那边的口径一致)。 */
  get endpointLabel(): string {
    const base = this.base;
    if (!base) return globalThis.location?.host ?? "";
    try {
      const u = new URL(base);
      return u.protocol === "https:" ? u.host + u.pathname.replace(/\/$/, "") : base;
    } catch {
      return base;
    }
  }

  async request<T>(
    method: string,
    params?: Record<string, unknown>,
    timeoutMs?: number,
    onAck?: () => void,
    _ackIsDelivery?: boolean,
  ): Promise<T> {
    const fetchImpl = this.opts.fetchImpl ?? globalThis.fetch;
    // 没有中转在中间接管,所以「请求已被受理」与「请求已发出」是同一刻。
    // relay 那边 ack 来自中转的托管回执;这里立刻触发,让依赖它做 UI 反馈的
    // 调用点(提交后的即时确认)行为一致。
    onAck?.();

    const controller = new AbortController();
    const timer = setTimeout(() => controller.abort(), timeoutMs ?? REQUEST_TIMEOUT_MS);
    let res: Response;
    try {
      res = await fetchImpl(`${this.base}/mobile_rpc`, {
        method: "POST",
        headers: this.headers(),
        body: JSON.stringify({ method, params: params ?? {} }),
        signal: controller.signal,
      });
    } catch (e) {
      // 断网、超时中止、网关拒连 —— 都没拿到裁决。主机可能已经把活干了,
      // 所以 remote 为假,调用方有权另行确认。
      throw new TransportError(errText(e), false);
    } finally {
      clearTimeout(timer);
    }

    if (!res.ok) {
      // HTTP 层的失败(502、401、代理超时)同样不是主机的裁决。
      throw new TransportError(`HTTP ${res.status}`, false);
    }

    let body: unknown;
    try {
      body = await res.json();
    } catch (e) {
      throw new TransportError(errText(e), false);
    }
    const reply = body as { ok?: boolean; data?: unknown; error?: string };
    if (reply?.ok !== true) {
      // 主机收到了、判断了、说不行。重试改变不了结果 —— remote 为真,调用方
      // 据此跳过自己的宽限兜底,直接把消息给用户看。
      throw new TransportError(reply?.error ?? "request failed", true);
    }
    return reply.data as T;
  }

  answer(kind: DecisionKind, id: string, fields: Record<string, unknown>): boolean {
    // 发后不管的老路径。这里没有「帧可能丢」这个问题,所以直接走请求路径并
    // 丢掉结果 —— 布尔值的契约本来就只是「送出去了」。
    void this.request("decision_answer", { kind, id, ...fields }).catch(() => {});
    return true;
  }

  async answerViaReq(
    kind: DecisionKind,
    id: string,
    fields: Record<string, unknown>,
    opts?: { attempts?: number; timeoutMs?: number },
  ): Promise<void> {
    // 不重发。relay 那边的重试是为了对付「中转尽力投递、丢帧无人知晓」;
    // HTTP 的响应本身就是投递裁决,拿到 2xx 就是主机确认过了,拿不到就是真的
    // 没成 —— 再发一次只是把同一个失败重演一遍。
    await this.request("decision_answer", { kind, id, ...fields }, opts?.timeoutMs);
  }

  /** 这条传输层没有推送通道:Web Push 的 VAPID 订阅登记在 relay 上,而这里
   *  按设计不碰 relay。返回 false 让「更多」页据此隐掉推送开关 —— 假装成功
   *  会让用户开了开关却永远收不到通知,那比没有开关更糟。 */
  pushSubscribe(_subscription: unknown): boolean {
    return false;
  }

  pushUnsubscribe(_subscription: unknown): boolean {
    return false;
  }
}

function parseJson(raw: string): unknown {
  try {
    return JSON.parse(raw);
  } catch {
    return undefined;
  }
}

function errText(e: unknown): string {
  return e instanceof Error ? e.message : String(e);
}
