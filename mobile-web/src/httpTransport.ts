// 骨架：结构就位、行为为空。存在的意义是让 httpTransport.test.ts 能真正跑进
// 测试体、停在断言上——「模块找不到」只证明代码没写，证明不了测试抓得住行为。
// 下一步把每个方法填上，断言逐条转绿。

import type { DecisionKind } from "./types";
import type { FleetTransport, TransportHandlers } from "./transport";

export interface HttpTransportOptions {
  fetchImpl?: typeof fetch;
  eventSourceImpl?: typeof EventSource;
}

export class HttpTransport implements FleetTransport {
  constructor(
    private readonly handlers: TransportHandlers,
    private readonly opts: HttpTransportOptions = {},
  ) {}

  connect(): void {
    const ES = this.opts.eventSourceImpl ?? globalThis.EventSource;
    new ES("/events");
  }

  close(): void {}

  sayGoodbye(): void {}

  get isAuthed(): boolean {
    return false;
  }

  async request<T>(
    _method: string,
    _params?: Record<string, unknown>,
    _timeoutMs?: number,
    _onAck?: () => void,
    _ackIsDelivery?: boolean,
  ): Promise<T> {
    return undefined as T;
  }

  answer(_kind: DecisionKind, _id: string, _fields: Record<string, unknown>): boolean {
    return false;
  }

  async answerViaReq(
    _kind: DecisionKind,
    _id: string,
    _fields: Record<string, unknown>,
    _opts?: { attempts?: number; timeoutMs?: number },
  ): Promise<void> {}

  pushSubscribe(_subscription: unknown): boolean {
    return false;
  }

  pushUnsubscribe(_subscription: unknown): boolean {
    return false;
  }
}
