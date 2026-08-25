// 纯聊天 workspace 的绝对路径。它在**桌面主机**的 home 下，手机端推导不出来，
// 所以向 relay 要（mobile_relay.rs::serve_request 的 `chat_workspace`）。
//
// 两处都要用：新会话弹层把它钉在目录选项首位（它没有「最近会话」可被发现），
// 任务页拿它把聊天会话从项目任务里筛出去。桌面端有一个同名的对应物
// (claw-fleet-desktop/app/hooks/useChatWorkspace.ts)。
import { useEffect, useState } from "react";
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
