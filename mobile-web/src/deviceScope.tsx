// 「我现在看的是哪一台」—— 供 UI 侧读取的设备作用域。
//
// 为什么是 context 而不是 prop:需要它的东西不是一两处,而是散在各处的**本地
// 持久化**——新会话草稿、附件、上次用的 repo、继续会话的输入框、任务页的
// workspace 筛选。这些键此前是全局的,单设备时代那没问题;多设备之后它们全都
// 是「属于某一台机器的东西」:A 机的 workspace 路径在 B 机上根本不存在,而
// 会话 id 只在单机内唯一,所以 `resume:<id>` 这种键跨设备会直接撞车。
//
// 用 context 的第二个理由是它对下一阶段是对的:聚合收件箱之后,从合并列表点进
// 去的详情页属于**那一台**而不是当前作用域那一台,于是那处下钻只要用归属设备
// 的 id 再包一层 provider,里面所有草稿就自动落到对的命名空间。换成 prop 或
// 模块级全局都做不到这一点(后者会在两台设备同时在场时静默读错)。
//
// 与传输层的分工:transport 是「数据从哪来」,这里是「本地存储写到哪」。前者
// 仍走 prop（见 transport.ts 的接缝说明）。

import { createContext, useContext, type ReactNode } from "react";
import { useDraft } from "./draft";

/** 设备作用域的键前缀。`null`(未配对 / 同源形态 / mock)时不加前缀 —— 那些
 *  形态下只有一个数据源,加了前缀只是让老用户的草稿凭空消失。 */
export function scopedKey(deviceId: string | null, key: string): string {
  return deviceId ? `d/${deviceId}/${key}` : key;
}

const DeviceScopeContext = createContext<string | null>(null);

export function DeviceScopeProvider({
  deviceId,
  children,
}: {
  deviceId: string | null;
  children: ReactNode;
}) {
  return (
    <DeviceScopeContext.Provider value={deviceId}>{children}</DeviceScopeContext.Provider>
  );
}

/** 当前作用域设备的 id,没有则 `null`。 */
export function useDeviceScope(): string | null {
  return useContext(DeviceScopeContext);
}

/** `useDraft` 的设备作用域版本。凡是内容只对某一台机器有意义的草稿都用它 ——
 *  纯 UI 偏好(排序、折叠、筛选开关这类)仍用 `useDraft`,它们属于这台手机而不
 *  属于某台 Fleet。 */
export function useDeviceDraft<T>(
  key: string,
  fallback: T,
): [T, (v: T | ((prev: T) => T)) => void, () => void] {
  const deviceId = useDeviceScope();
  return useDraft<T>(scopedKey(deviceId, key), fallback);
}
