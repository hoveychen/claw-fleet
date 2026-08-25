// 桌面主机上被监控的 agent 源（{name, enabled, available}）。手机端推导不出
// 「装没装 / 开没开」，所以向 relay 要（mobile_relay.rs::serve_request 的
// `sources_config`）。新会话弹层用它把工具选择器限制在真正被监控的源上——
// codex 源关掉时就不该在启动器里列 Codex。桌面端的对应物是
// claw-fleet-desktop/app/components/SettingsPanel 里的 get_sources_config。
import { useEffect, useState } from "react";
import type { FleetTransport } from "./transport";

export interface SourceInfo {
  name: string;
  enabled: boolean;
  available: boolean;
}

/** `null` 表示还没拿到——relay 未连上、请求在途，或桌面端版本老到不认这个方法。
 *  调用方必须把 null 当作「不知道」而不是「没有源」。 */
export function useSourcesConfig(client: FleetTransport | null): SourceInfo[] | null {
  const [sources, setSources] = useState<SourceInfo[] | null>(null);
  useEffect(() => {
    if (!client) return;
    let alive = true;
    client
      .request<SourceInfo[]>("sources_config")
      .then((r) => {
        if (alive) setSources(r);
      })
      .catch(() => {
        if (alive) setSources(null);
      });
    return () => {
      alive = false;
    };
  }, [client]);
  return sources;
}
