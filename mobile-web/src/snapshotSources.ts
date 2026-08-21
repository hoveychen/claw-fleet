// 每份 pending_snapshot 是哪个 agent 回的 —— 手机端自留的诊断台账。
//
// relay 把客户端的每个请求广播给频道里**所有** agent（Channel.agents 是个 map），
// 手机采信最先到的那份回复。所以桌面端之外只要冒出第二个 agent，它就能替桌面端
// 作答；如果它看不到本机的 ~/.fleet（例如跑在被重定向的 FLEET_HOME 下），回的就是
// 一份空列表，而快照在客户端是权威的整表覆盖 —— 卡片因此被抹掉。
// decisionReconcile.ts 负责当场拦下这种空快照；这里负责把「谁来过」记下来，
// 好让下次复现时在「更多」页一眼看到李鬼的 host/pid/home，不用再翻服务端日志。
import type { AgentFingerprint } from "./types";

/** 台账最多留几条来源。正常只有 1 条（桌面端），多出来的就是要查的。 */
export const MAX_SNAPSHOT_SOURCES = 8;

export interface SnapshotSource {
  /** agentKeyOf() 的结果；`undefined` 表示对方没带指纹（老桌面端）。 */
  key?: string;
  agent?: AgentFingerprint;
  firstAt: number;
  lastAt: number;
  /** 这个来源一共回了多少份快照。 */
  snapshots: number;
  /** 其中有多少份空快照因为来源可疑被丢掉了。 */
  ignored: number;
  /** 最近一次是否被当作主 agent（卡片的真实来源）。 */
  trusted: boolean;
}

export interface SnapshotSourceEvent {
  key?: string;
  agent?: AgentFingerprint;
  at: number;
  trusted: boolean;
  ignored: boolean;
}

/** 纯函数：把一次快照到达并入台账，返回新数组（不改入参）。 */
export function recordSnapshotSource(
  sources: SnapshotSource[],
  ev: SnapshotSourceEvent,
): SnapshotSource[] {
  const next = sources.map((s) =>
    s.key === ev.key
      ? {
          ...s,
          agent: ev.agent ?? s.agent,
          lastAt: ev.at,
          snapshots: s.snapshots + 1,
          ignored: s.ignored + (ev.ignored ? 1 : 0),
          trusted: ev.trusted,
        }
      : s,
  );
  if (!next.some((s) => s.key === ev.key)) {
    next.push({
      key: ev.key,
      agent: ev.agent,
      firstAt: ev.at,
      lastAt: ev.at,
      snapshots: 1,
      ignored: ev.ignored ? 1 : 0,
      trusted: ev.trusted,
    });
  }
  // 超出上限时丢最久没动静的那条 —— 当前活跃的来源永远留得住。
  if (next.length > MAX_SNAPSHOT_SOURCES) {
    next.sort((a, b) => b.lastAt - a.lastAt);
    return next.slice(0, MAX_SNAPSHOT_SOURCES);
  }
  return next;
}
