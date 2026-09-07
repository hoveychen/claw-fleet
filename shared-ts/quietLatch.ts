// 桌面端与移动端共用的一份「quiet-alive 迟滞」：让状态点不再亮绿/暗绿来回跳。
//
// 这个目录是仓库里唯一的跨前端包共享源码点，只放零依赖的纯函数（见 procShell.ts 的
// 说明）。这里共享的理由和那条一样：桌面 `rowBarColor` 与移动端 `statusTone` 是同一
// 语义的两份并行实现，闪烁这个毛病两边都有，判定逻辑各写一份必然漂移。
//
// 为什么需要迟滞：core 的 `determine_status` 只看 transcript 最后几条记录**加它们的
// 年龄**，每个活跃分支都有硬窗口（`stop_reason=tool_use` 保 60s Executing，尾随 user
// 消息保 120s Thinking，兜底 30s 后掉 Idle）。于是一个卡在**一条长工具调用**上的会话
// ——跑 build、等后台任务——每隔几分钟写一行 transcript：每写一次状态被顶回活跃
// （亮绿），窗口一过又衰减成 idle（暗绿），一分钟里跳好几次。
//
// 取舍（老板已拍板）：暗绿一旦点亮就粘住，代价是「真的卡住了」这个信号变迟钝一点。
// 恢复不靠等超时，而靠**写入变密集**——两次相邻写入间隔 ≤ DENSE_WRITE_MS 才判定
// 会话真的又在干活，立刻回亮绿。单独一次稀疏写入只更新基线，不解锁。

/** 判定「写入变密集」的间隔上限。取 30s——正好是 core 兜底掉 Idle 的窗口：间隔比它还
 *  短的两次写入，本来根本不会衰减成 quiet，所以只有真的连续产出才会命中。 */
export const DENSE_WRITE_MS = 30_000;

/** 超过这个时长没再被观察到的条目会被清掉，纯粹为了让 Map 不随扫到过的会话数无界增长。
 *  不是「暗绿超时回亮绿」——那正是要避免的抖动来源。 */
const ENTRY_TTL_MS = 3_600_000;

type Entry = {
  /** 最近一次被观察为 raw quiet 的时刻（仅用于 TTL 清理）。 */
  quietAt: number;
  /** 已观察到的最新 transcript 活动时间戳，用来量下一次写入的间隔。 */
  lastActivityMs: number;
  /** 最近一次被观察到的时刻（TTL 清理用）。 */
  seenAt: number;
};

export type QuietLatchState = Map<string, Entry>;

export function createQuietLatch(): QuietLatchState {
  return new Map();
}

export type QuietObservation = {
  /** 未经迟滞的原始判定：进程活着，但扫描算出的状态已经衰减成「结束」。 */
  rawQuiet: boolean;
  /** 该会话 transcript 的最后活动时间戳（`SessionInfo.lastActivityMs`）。 */
  lastActivityMs: number;
  now: number;
};

/** 带迟滞的 quiet-alive 判定。同一个会话每次渲染都可以调，幂等：
 *  重复观察同一个 `lastActivityMs` 不会被当成第二次写入。 */
export function stickyQuiet(
  state: QuietLatchState,
  id: string,
  { rawQuiet, lastActivityMs, now }: QuietObservation,
): boolean {
  const prev = state.get(id);
  if (rawQuiet) {
    state.set(id, {
      quietAt: now,
      // 进入 quiet 时的基线取「已知最新的活动时间」——扫描侧的 lastActivityMs 偶有
      // 回退（快照合并、增量补齐），基线只许前进，否则一次回退会伪造出一个密集间隔。
      lastActivityMs: Math.max(lastActivityMs, prev?.lastActivityMs ?? lastActivityMs),
      seenAt: now,
    });
    return true;
  }
  if (!prev) return false;
  prune(state, now);
  const gap = lastActivityMs - prev.lastActivityMs;
  if (gap > 0 && gap <= DENSE_WRITE_MS) {
    // 两次相邻写入挨得很近：会话真的又在产出了，解锁回亮绿。
    state.delete(id);
    return false;
  }
  if (gap > 0) {
    // 一次稀疏写入：不解锁，只把基线推到这次写入，好让**下一次**写入的间隔从这里量。
    state.set(id, { ...prev, lastActivityMs, seenAt: now });
  } else {
    state.set(id, { ...prev, seenAt: now });
  }
  return true;
}

function prune(state: QuietLatchState, now: number): void {
  if (state.size < 256) return;
  for (const [k, v] of state) {
    if (now - v.seenAt > ENTRY_TTL_MS) state.delete(k);
  }
}

/** 测试用：清空迟滞状态，好让各条用例之间互不串味。 */
export function resetQuietLatch(state: QuietLatchState): void {
  state.clear();
}
