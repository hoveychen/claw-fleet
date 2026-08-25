// 移动端 UI 与「后端」之间的唯一接缝。
//
// 这个 app 的所有数据都经过一个对象:视图拿到它、调 `request()`、订阅它推来的
// 回调。在此之前那个对象只可能是 `RelayClient`,于是「经过 relay」被当成了
// 「有数据」的同义词。它们其实是两件事:relay 解决的是「手机不在桌面端同一
// 张网里」,而同源部署(`fleet webui` 把移动端 UI 和数据路由从同一个端口发出)
// 根本没有这个问题——那里既没有配对密钥,也没有 WebSocket,更没有 relay。
//
// 所以接缝在这里显形:`FleetTransport` 是视图真正依赖的那一小块面,
// `RelayClient` 只是它的第一个实现。第二个实现走同源 HTTP,并且**不 import
// 本文件之外的任何 relay 代码**——这一点是硬约束而非风格偏好:`relay.ts`
// 在模块加载时就会执行 `resolveRelayBase()` 去解析一个 relay 地址,浏览器构建
// 只要碰到它,就会带着一个自己永远不会用的 relay 客户端上路。
//
// 因此凡是与「怎么把字节送出去」无关的东西——错误分类、慢方法的超时预算——
// 都住在这里,而不是住在某一个实现里。`relay.ts` re-export 它们,老的 import
// 路径继续有效。

import type { DecisionKind, SessionInfo } from "./types";

/** 一次失败的 `request()`,带上「失败发生在哪一层」。
 *
 *  `remote: true` —— 主机收到了请求、做了判断、说不行(`ok:false` 回复)。
 *  消息是主机自己的文本。重试或等待都不会改变结果,直接展示给用户。
 *
 *  `remote: false` —— 请求压根没拿到裁决:超时、连接断了、回复帧丢了。
 *  主机很可能已经把活干了,所以调用方有权在宣告失败前用别的方式确认一次
 *  (见 `waitForSessionId`)。
 *
 *  这个区分对两个传输层同样成立,所以它属于接口而不属于任何一个实现:HTTP 的
 *  `ok:false` 与 relay 的 `reply{ok:false}` 是同一件事,fetch 抛错与 WebSocket
 *  掉线也是同一件事。 */
export class TransportError extends Error {
  constructor(
    message: string,
    readonly remote: boolean,
  ) {
    super(message);
    // 保持既有值:历史上这个类叫 RelayRequestError,而 `name` 会进日志和
    // 上报。改掉它只会让新旧记录对不上,换不来任何东西。
    this.name = "RelayRequestError";
  }
}

/** 主机明确拒绝了请求 —— 与「回复根本没到」相对。有降级兜底的调用方必须在
 *  这个为 false 时才启用兜底,否则会为一个主机早就给了答复的错误白等一整个
 *  宽限窗口。 */
export function isDesktopRejection(e: unknown): e is TransportError {
  return e instanceof TransportError && e.remote;
}

/** 资源类请求(决策卡预览图、知识库附件)的超时。
 *
 *  控制类请求小,默认的十几秒绰绰有余;资源请求要搬 MB 级数据过可能很慢的
 *  移动链路,用默认值会在弱网上假性中止——pending 条目被丢掉、迟到的回复被
 *  丢弃,而卡片上的 `<img>` 就永远吊在那里,连个错误都没有(见
 *  decisionAsset.test.ts 与对应的 e2e 复现)。 */
export const ASSET_REQUEST_TIMEOUT_MS = 60_000;
/** 上传的超时预算。同上,只是方向相反且通常更大。 */
export const UPLOAD_REQUEST_TIMEOUT_MS = 120_000;
/** `answerViaReq` 放弃前重发答复的次数。主机按决策 id 去重,所以丢了回复之后
 *  的重发是幂等的;这个数值限定的是「弱网要重试多久才让卡片退回可重试态」。 */
export const ANSWER_MAX_ATTEMPTS = 3;

/** 一次往返,拆成成因各不相同的几段。
 *
 *  光看 `totalMs` 说不出卡顿到底是这台手机的网络、主机的网络,还是主机自己的
 *  handler——三种修法毫不相干,所以这个拆分本身就是测量的全部意义。
 *
 *  两段都可选,因为任一来源都可能缺席:快链路上 relay 的 `msg_ack` 可能输给
 *  回复本身,而不带 `handle_ms` 的旧主机什么都不报。缺一段只会让 UI 退化成更
 *  粗的答案,不会让它编一个出来。 */
export interface RttSample {
  /** 请求→回复,全程按这台手机自己的时钟计。 */
  totalMs: number;
  /** 手机↔中转的往返。同源 HTTP 没有中转这一段,恒为 null。 */
  phoneRelayMs: number | null;
  /** 主机报告自己在 handler 里花掉的时间(`handle_ms`),它用自己的时钟量,
   *  所以不涉及任何时钟同步。 */
  desktopHandleMs: number | null;
}

/** 传输层推给 UI 的事件。
 *
 *  每个实现都得把自己那套底层信号翻译成这组回调:relay 翻译 WebSocket 帧,
 *  HTTP 实现翻译 SSE 事件。有些信号在某个传输层下没有对应物(同源部署里
 *  「主机是否在线」和「页面是否加载出来」是同一件事),那就让它恒定或永不触发
 *  ——**不要伪造一个变化**,UI 会把它当真。 */
export interface TransportHandlers {
  /** 这台设备到数据源的连通性。 */
  onStatus?: (connected: boolean) => void;
  /** 主机侧的连通性。relay 下是「桌面端有没有连上中转」;同源下主机就是发出
   *  这张页面的那个进程,所以只要页面活着它就是在线。 */
  onAgentOnline?: (online: boolean) => void;
  onDecisionCreated?: (kind: DecisionKind, request: unknown) => void;
  onDecisionResolved?: (kind: DecisionKind, id: string) => void;
  onSessions?: (sessions: SessionInfo[]) => void;
  /** 刚落地的会话帧是哪一种 —— `full`(整份快照)还是 `delta`(增量增删)。
   *  让 UI 能显示主机的增量通道是否真的启用了。每次会话更新都触发。 */
  onSessionsKind?: (kind: "full" | "delta") => void;
  /** 一次请求→回复的往返采样。弱链路拥塞信号,也是区分「链路慢」与「主机慢」
   *  的唯一途径。 */
  onRttSample?: (sample: RttSample) => void;
  /** 每次连接掉线并安排重连时触发 —— 第二个弱链路信号(频繁重连 ⇒ 拥塞)。 */
  onReconnect?: () => void;
  onAuthError?: (message: string) => void;
}

/** 视图层真正依赖的那一小块面。
 *
 *  刻意保持得小:每多一个方法,第二个实现就多一份要么照做要么撒谎的义务。
 *  这里的每一项都有 UI 里实打实的调用点在撑着。 */
export interface FleetTransport {
  /** 开始连接 / 开始接收推送。幂等。 */
  connect(): void;
  /** 断开并停止一切后台活动。 */
  close(): void;
  /** 尽力而为的「我走了」,让主机不必等超时就把这台设备摘掉。 */
  sayGoodbye(): void;
  /** 数据面是否可用。UI 拿它当轮询和「链路是否活着」的闸门。 */
  readonly isAuthed: boolean;
  /** 「我连到哪」的人类可读形式,给「更多」页显示一行。
   *
   *  在接口上而不是让 UI 自己去问 relay:这一行的答案随传输层而变(relay 答
   *  中转主机名,同源答自己的 origin),而「更多」页不该为了显示一行字就 import
   *  一个具体实现 —— 那正是会把 relay 拖进同源构建的那类依赖。 */
  readonly endpointLabel: string;
  /** 向主机发一次数据请求(pending_snapshot / task_plans / …)。 */
  request<T>(
    method: string,
    params?: Record<string, unknown>,
    timeoutMs?: number,
    onAck?: () => void,
    ackIsDelivery?: boolean,
  ): Promise<T>;
  /** 发后不管的答复。布尔值只表示「送出去了」,不代表主机收到。 */
  answer(kind: DecisionKind, id: string, fields: Record<string, unknown>): boolean;
  /** 可靠答复路径:拿到真正的投递裁决,丢帧则重发(主机按决策 id 去重,重发
   *  幂等)。决策 UI 一律走这条,免得丢一帧就把卡片吊死。 */
  answerViaReq(
    kind: DecisionKind,
    id: string,
    fields: Record<string, unknown>,
    opts?: { attempts?: number; timeoutMs?: number },
  ): Promise<void>;
  /** 注册推送订阅。传输层没有推送通道时返回 false —— 调用方据此决定是否显示
   *  推送开关,所以这里必须诚实地说不,而不是假装成功。 */
  pushSubscribe(subscription: unknown): boolean;
  /** 注销先前注册的订阅。 */
  pushUnsubscribe(subscription: unknown): boolean;
}
