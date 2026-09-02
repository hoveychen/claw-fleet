// 未提交表单草稿的持久化。新会话 sheet / 继续会话 composer 里输入到一半的内容，
// 在意外关闭 sheet、切标签、iOS 杀掉 PWA 后不该丢失——落到 localStorage，回来自动恢复，
// 提交成功后清空。风格对齐 theme.ts / i18n.ts / secretStore.ts 的 localStorage 用法。

import { useCallback, useRef, useState } from "react";

const PREFIX = "fleet-draft:";

/** localStorage 里我们真正用到的子集，测试可注入内存实现（node 环境没有 window）。 */
export type DraftStorage = Pick<Storage, "getItem" | "setItem" | "removeItem">;

/** 私隐模式 / SSR 下 window.localStorage 可能抛异常或不存在，取不到就返回 null，
 *  草稿持久化全程 best-effort，取不到 storage 只是退化成普通 useState。 */
function defaultStorage(): DraftStorage | null {
  try {
    return window.localStorage;
  } catch {
    return null;
  }
}

function isPlainObject(v: unknown): v is Record<string, unknown> {
  return typeof v === "object" && v !== null && !Array.isArray(v);
}

/** 读草稿。对象型草稿与 fallback 做浅合并——这样后续给表单加字段时，
 *  旧草稿缺的字段自动取新默认值，不会读出 undefined。非对象（如纯字符串）直接返回解析值。 */
export function loadDraft<T>(
  key: string,
  fallback: T,
  store: DraftStorage | null = defaultStorage(),
): T {
  if (!store) return fallback;
  try {
    const raw = store.getItem(PREFIX + key);
    if (raw == null) return fallback;
    const parsed = JSON.parse(raw);
    if (isPlainObject(fallback) && isPlainObject(parsed)) {
      return { ...fallback, ...parsed } as T;
    }
    return parsed as T;
  } catch {
    return fallback;
  }
}

export function saveDraft<T>(
  key: string,
  value: T,
  store: DraftStorage | null = defaultStorage(),
): void {
  if (!store) return;
  try {
    store.setItem(PREFIX + key, JSON.stringify(value));
  } catch {
    // 配额满 / 私隐模式写入被拒——持久化是锦上添花，失败不影响表单本身可用。
  }
}

export function clearDraft(
  key: string,
  store: DraftStorage | null = defaultStorage(),
): void {
  if (!store) return;
  try {
    store.removeItem(PREFIX + key);
  } catch {
    // 同上，忽略。
  }
}

/** 清掉某个前缀下的全部草稿。移除一台设备时用它扫掉那台的命名空间
 *  （`d/<deviceId>/…`，见 deviceScope.ts）——不清的话每移除一台就留下一份
 *  永远不会再被读到的草稿、附件路径和 workspace 记忆。
 *
 *  只在能枚举键的存储上生效（`Storage` 有 length/key(i)，注入的内存实现通常
 *  没有），所以枚举能力是运行时探测的：探测不到就什么都不做，而不是抛。 */
export function clearDraftsByPrefix(
  prefix: string,
  store: DraftStorage | null = defaultStorage(),
): void {
  const enumerable = store as (DraftStorage & Partial<Storage>) | null;
  if (!enumerable || typeof enumerable.length !== "number" || !enumerable.key) return;
  const full = PREFIX + prefix;
  const doomed: string[] = [];
  try {
    for (let i = 0; i < enumerable.length; i++) {
      const k = enumerable.key(i);
      if (k && k.startsWith(full)) doomed.push(k);
    }
    for (const k of doomed) enumerable.removeItem(k);
  } catch {
    // 同上，忽略。
  }
}

/** useState 的持久化版：初值从草稿恢复，每次 set 落盘，clear() 清盘并复位到 fallback。 */
export function useDraft<T>(
  key: string,
  fallback: T,
): [T, (next: T | ((prev: T) => T)) => void, () => void] {
  // fallback 一般是每次渲染新建的字面量，用 ref 钉住首次值，避免 clear 的引用漂移。
  const fallbackRef = useRef(fallback);
  const [value, setValue] = useState<T>(() => loadDraft(key, fallbackRef.current));

  const set = useCallback(
    (next: T | ((prev: T) => T)) => {
      setValue((prev) => {
        const resolved =
          typeof next === "function" ? (next as (p: T) => T)(prev) : next;
        saveDraft(key, resolved);
        return resolved;
      });
    },
    [key],
  );

  const clear = useCallback(() => {
    clearDraft(key);
    setValue(fallbackRef.current);
  }, [key]);

  return [value, set, clear];
}
