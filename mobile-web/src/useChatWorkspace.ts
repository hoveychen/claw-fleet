// 纯聊天 workspace 的绝对路径。它在**桌面主机**的 home 下，手机端推导不出来，
// 所以向 relay 要（mobile_relay.rs::serve_request 的 `chat_workspace`）。
//
// 两处都要用：新会话弹层把它钉在目录选项首位（它没有「最近会话」可被发现），
// 任务页拿它把聊天会话从项目任务里筛出去。桌面端有一个同名的对应物
// (claw-fleet-desktop/app/hooks/useChatWorkspace.ts)。
import { useEffect, useMemo, useState } from "react";
import type { FleetTransport } from "./transport";

/** `null` 表示还没拿到——relay 未连上、请求在途，或桌面端版本老到不认这个方法。
 *  调用方必须把 null 当作「不知道」而不是「没有聊天目录」。 */
export function useChatWorkspace(client: FleetTransport | null): string | null {
  const [path, setPath] = useState<string | null>(null);
  useEffect(() => {
    if (!client) return;
    let alive = true;
    client
      .request<{ path: string }>("chat_workspace")
      .then((r) => {
        if (alive) setPath(r.path);
      })
      .catch(() => {
        if (alive) setPath(null);
      });
    return () => {
      alive = false;
    };
  }, [client]);
  return path;
}

/**
 * 同上，但**每台设备各问一次**。任务页的列表是多设备合并的，聊天分区置顶要对每台
 * 机器都成立：远端主机的聊天目录是它自己 home 下的路径（`/root/.fleet/chat`），拿
 * 本机那一条去比永远比不中，那台机器的 Chat 分区就沉在项目中间。
 *
 * 返回 deviceId → 路径的映射；某台还没拿到（或桌面端老到不认这个方法）就没有这个
 * 键，调用方按「不知道」处理。已拿到的结果留着，不会因为一次快照重渲染而重问。
 */
export function useChatWorkspaces(
  deviceIds: readonly string[],
  clientFor: (deviceId: string) => FleetTransport | null,
): Record<string, string> {
  const [paths, setPaths] = useState<Record<string, string>>({});
  // 依赖用拼好的字符串：调用方每次快照都会给出一个新数组，但设备集合基本不变。
  const key = useMemo(() => [...deviceIds].sort().join(" "), [deviceIds]);
  useEffect(() => {
    if (!key) return;
    let alive = true;
    for (const id of key.split(" ")) {
      const transport = clientFor(id);
      if (!transport) continue;
      transport
        .request<{ path: string }>("chat_workspace")
        .then((r) => {
          if (alive && r?.path) setPaths((prev) => (prev[id] === r.path ? prev : { ...prev, [id]: r.path }));
        })
        .catch(() => {
          /* 老版本桌面端不认这个方法 —— 那台就没有置顶，不是错误 */
        });
    }
    return () => {
      alive = false;
    };
  }, [key, clientFor]);
  return paths;
}
