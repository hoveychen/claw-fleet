// 桌面主机启动时开了哪些可选面。目前只有一个：终端（后端的 FLEET_TERMINAL，见
// claw-fleet-core/src/feature_flags.rs）。手机端自己推不出来，向 relay 要
// （mobile_relay.rs::serve_request 的 `host_features`）。
//
// **默认全关**：拿不到答案（relay 没连上、请求在途、桌面端老到不认这个方法）时
// 一律按关处理。反过来（先当开、被拒再收回）会让用户点进终端页、开 shell 时才
// 撞上一句 "terminal feature is disabled"——一个入口存在与否，不该由一次失败的
// 请求来告诉用户。桌面端的对应物是 store.ts 的 loadHostFeatures，同样 fail
// closed。
import { useEffect, useState } from "react";
import type { HostFeatures } from "./generated/types";
import type { FleetTransport } from "./transport";

const ALL_OFF: HostFeatures = { terminal: false };

/** 把一次应答收成一个可信的开关集。单独拎出来是因为这里的每一条都是「宁可关」:
 *  老桌面端会应答 null 或缺字段,relay 层的 JSON 也可能把 true 送成字符串
 *  "true" —— 真值判断会把一个字符串当成开,而 `=== true` 不会。 */
export function normalizeHostFeatures(raw: unknown): HostFeatures {
  const terminal = (raw as HostFeatures | null | undefined)?.terminal;
  return { terminal: terminal === true };
}

export function useHostFeatures(client: FleetTransport | null): HostFeatures {
  const [features, setFeatures] = useState<HostFeatures>(ALL_OFF);
  useEffect(() => {
    if (!client) return;
    let alive = true;
    client
      .request<HostFeatures>("host_features")
      .then((r) => {
        if (alive) setFeatures(normalizeHostFeatures(r));
      })
      .catch(() => {
        if (alive) setFeatures(ALL_OFF);
      });
    return () => {
      alive = false;
    };
  }, [client]);
  return features;
}
