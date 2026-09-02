// 设备簿 —— 这台手机配对过的每一台 Fleet。
//
// 在此之前「配对」是单数:一个 `fleet-relay-secret`,一台桌面端。多设备之所以
// 不是把那个键存成数组就完事,是因为**每一份配对都自带它的 relay 地址**:一台
// 自建 relay 上的桌面端和一台默认 relay 上的桌面端必须能同时在册,所以 relay
// 归属跟着设备走,而不再是一个模块级常量(见 relay.ts 的 RELAY_BASE)。
//
// 持久化沿用 secretStore.ts 那套双写(localStorage + IndexedDB 镜像)及其理由:
// iOS Safari 在非 A2HS 的普通标签页下会在 7 天无访问后清掉脚本可写存储,而两
// 个存储在部分清理路径里是各自独立被回收的,所以哪一份活下来就用哪一份。IDB
// 复用 secretStore 的同一个库/store,只是换一个 key —— 无 schema 变更。
//
// 这里刻意分成两层:**纯函数**(book 的增删改查、迁移判定)与**存储 IO**。测试
// 环境(node)没有 indexedDB,所以凡是需要断言的语义都在纯函数层;IO 层只负责
// 把 book 搬进搬出,任何失败都退化成「这一份存储没读到」,绝不抛。

import { parseRelayParam } from "./relayBase";
import { extractSecretFromUrl, openDb } from "./secretStore";

/** 本机存 book 的键(localStorage 与 IDB 共用同一个名字)。 */
const BOOK_KEY = "fleet-devices";
/** 单设备时代的键。只在迁移时读一次,之后不再写。 */
const LEGACY_SECRET_KEY = "fleet-relay-secret";

/** 一台配对过的 Fleet。 */
export interface PairedDevice {
  /** 本机生成的稳定 id。用它而不是 secret 做外键,免得把密钥撒进缓存键、
   *  路由参数、React key 这些会被日志和 devtools 看见的地方。 */
  id: string;
  /** 用户可改的显示名。 */
  label: string;
  /** 配对密钥。channelToken 与 encKey 都由它 HKDF 派生(relayCrypto.ts)。 */
  secret: string;
  /** 这份配对指名的 relay origin;`null` 表示用构建默认值。 */
  relayBase: string | null;
  addedAt: number;
}

export interface DeviceBook {
  devices: PairedDevice[];
  /** 当前作用域设备(知识库/用量这些单机页面看的是它)。`null` = 一台都没配对。 */
  activeId: string | null;
}

export function emptyBook(): DeviceBook {
  return { devices: [], activeId: null };
}

/** 宽容解析。存储里的东西可能是旧版本写的、被别的工具改过的、或者半截的 ——
 *  任何一条不合格的记录只丢它自己,不让整本簿子归零(那等于静默解除全部配对)。 */
export function parseBook(raw: unknown): DeviceBook | null {
  let value = raw;
  if (typeof value === "string") {
    try {
      value = JSON.parse(value);
    } catch {
      return null;
    }
  }
  if (typeof value !== "object" || value === null) return null;
  const list = (value as { devices?: unknown }).devices;
  if (!Array.isArray(list)) return null;
  const devices: PairedDevice[] = [];
  for (const item of list) {
    if (typeof item !== "object" || item === null) continue;
    const d = item as Record<string, unknown>;
    if (typeof d.id !== "string" || !d.id) continue;
    if (typeof d.secret !== "string" || !d.secret) continue;
    devices.push({
      id: d.id,
      secret: d.secret,
      label: typeof d.label === "string" ? d.label : "",
      relayBase: typeof d.relayBase === "string" ? d.relayBase : null,
      addedAt: typeof d.addedAt === "number" ? d.addedAt : 0,
    });
  }
  if (devices.length === 0) return null;
  const activeRaw = (value as { activeId?: unknown }).activeId;
  const activeId =
    typeof activeRaw === "string" && devices.some((d) => d.id === activeRaw)
      ? activeRaw
      : devices[0].id;
  return { devices, activeId };
}

/** 单设备时代的一个 secret → 一本单条目的簿子。 */
export function bookFromLegacySecret(secret: string, opts: DeviceMint): DeviceBook {
  const device: PairedDevice = {
    id: opts.id,
    label: opts.label,
    secret,
    // 迁移过来的那台没记过 relay:它一直用的就是构建默认值(旧代码里的
    // RELAY_BASE),所以 null 在这里不是「未知」,而是「就是默认那个」。
    relayBase: null,
    addedAt: opts.now,
  };
  return { devices: [device], activeId: device.id };
}

export function activeDevice(book: DeviceBook): PairedDevice | null {
  if (!book.activeId) return null;
  return book.devices.find((d) => d.id === book.activeId) ?? null;
}

export function deviceById(book: DeviceBook, id: string): PairedDevice | null {
  return book.devices.find((d) => d.id === id) ?? null;
}

/** 下一台的默认名:`<prefix> N`,N 取「还没被用掉的最小序号」,这样删掉中间
 *  一台再加一台不会撞名。 */
export function nextDeviceLabel(book: DeviceBook, prefix: string): string {
  const used = new Set(book.devices.map((d) => d.label));
  for (let n = 1; ; n++) {
    const candidate = `${prefix} ${n}`;
    if (!used.has(candidate)) return candidate;
  }
}

export interface AddDeviceInput {
  secret: string;
  /** 这份配对指名的 relay;省略/`null` = 用构建默认值。 */
  relayBase?: string | null;
  label: string;
  id: string;
  now: number;
}

export interface AddDeviceResult {
  book: DeviceBook;
  device: PairedDevice;
  /** 这个 secret 本来就在册 —— 同一张二维码被扫了第二次。 */
  deduped: boolean;
}

/** 新增一台(或认出它本来就在册)。新增/重扫都把它设为当前设备 —— 用户刚扫完
 *  一张码,想看的就是那台。
 *
 *  去重按 **secret** 而非 channelToken:token 是 secret 的 HKDF 像
 *  (relayCrypto.ts),两者一一对应,而 token 派生是异步的 SubtleCrypto 调用。
 *  拿 secret 比对得到完全相同的判定,且让这个函数保持纯同步。
 *
 *  重扫时**保留原有 label**(用户可能已经改过名),但更新 relayBase —— 后者
 *  描述的是「这份配对现在挂在哪个 relay」,桌面端换了 relay 地址重出一张码时,
 *  新的那个才是对的。 */
export function addDevice(book: DeviceBook, input: AddDeviceInput): AddDeviceResult {
  const existing = book.devices.find((d) => d.secret === input.secret);
  if (existing) {
    const relayBase = input.relayBase ?? existing.relayBase;
    const updated: PairedDevice = { ...existing, relayBase };
    return {
      book: {
        devices: book.devices.map((d) => (d.id === existing.id ? updated : d)),
        activeId: existing.id,
      },
      device: updated,
      deduped: true,
    };
  }
  const device: PairedDevice = {
    id: input.id,
    label: input.label,
    secret: input.secret,
    relayBase: input.relayBase ?? null,
    addedAt: input.now,
  };
  return {
    book: { devices: [...book.devices, device], activeId: device.id },
    device,
    deduped: false,
  };
}

/** 移除一台。删掉的正好是当前设备时,焦点落到剩下的第一台(没有剩下的就是
 *  `null`,回到未配对态)。 */
export function removeDevice(book: DeviceBook, id: string): DeviceBook {
  const devices = book.devices.filter((d) => d.id !== id);
  if (devices.length === book.devices.length) return book;
  const activeId =
    book.activeId === id ? (devices[0]?.id ?? null) : book.activeId;
  return { devices, activeId };
}

/** 改名。空白名被忽略(否则列表里会出现一台没名字的设备)。 */
export function renameDevice(book: DeviceBook, id: string, label: string): DeviceBook {
  const trimmed = label.trim();
  if (!trimmed) return book;
  return {
    ...book,
    devices: book.devices.map((d) => (d.id === id ? { ...d, label: trimmed } : d)),
  };
}

/** 切换当前设备。不在册的 id 被忽略。 */
export function setActiveDevice(book: DeviceBook, id: string): DeviceBook {
  if (!book.devices.some((d) => d.id === id)) return book;
  return { ...book, activeId: id };
}

// ── 存储 IO ─────────────────────────────────────────────────────────────────

function readLocal(key: string): string | null {
  try {
    return localStorage.getItem(key);
  } catch {
    return null;
  }
}

/** 同步可得的簿子:localStorage 的 book,否则由单设备时代的 secret 迁移一本。
 *
 *  迁移是**写回的**:迁移出来的簿子当场持久化,这样下一次启动读到的就是新格式。
 *  旧键刻意**不删** —— 万一新格式因为任何原因没写成,旧键还在,用户不至于被
 *  静默解除配对;它只是从此不再被写入。 */
export function loadBookSync(mint: DeviceMint): DeviceBook {
  const parsed = parseBook(readLocal(BOOK_KEY));
  if (parsed) return parsed;
  const legacy = readLocal(LEGACY_SECRET_KEY);
  if (legacy) {
    const book = bookFromLegacySecret(legacy, mint);
    persistBook(book);
    return book;
  }
  return emptyBook();
}

/** 一台新设备的三个本机字段。调用方（App）负责生成，因为默认名要走 i18n 而
 *  这一层刻意不认识 i18n。 */
export interface DeviceMint {
  id: string;
  label: string;
  now: number;
}

/** IndexedDB 兜底:localStorage 被清掉而 IDB 活下来的那条路径。同样覆盖旧键
 *  (旧版把 secret 也镜像进了 IDB)。 */
export async function loadBookFromIdb(mint: DeviceMint): Promise<DeviceBook | null> {
  const raw = await idbGet(BOOK_KEY);
  const parsed = parseBook(raw);
  if (parsed) return parsed;
  const legacy = await idbGet(LEGACY_SECRET_KEY);
  if (typeof legacy === "string" && legacy) {
    return bookFromLegacySecret(legacy, mint);
  }
  return null;
}

function idbGet(key: string): Promise<unknown> {
  return openDb()
    .then(
      (db) =>
        new Promise<unknown>((resolve) => {
          const tx = db.transaction("kv", "readonly");
          const req = tx.objectStore("kv").get(key);
          req.onsuccess = () => resolve(req.result);
          req.onerror = () => resolve(null);
        }),
    )
    .catch(() => null);
}

/** 双写。localStorage 是同步真相,IDB 是发后不管的镜像。 */
export function persistBook(book: DeviceBook): void {
  const json = JSON.stringify(book);
  try {
    localStorage.setItem(BOOK_KEY, json);
  } catch {
    // 存储满 / 隐私模式 —— 下面的 IDB 仍可能写成
  }
  void openDb()
    .then(
      (db) =>
        new Promise<void>((resolve) => {
          const tx = db.transaction("kv", "readwrite");
          tx.objectStore("kv").put(json, BOOK_KEY);
          tx.oncomplete = () => resolve();
          tx.onerror = () => resolve();
        }),
    )
    .catch(() => {});
}

/** 一次扫码落地:把 secret 并入簿子并**当场持久化**。
 *
 *  两条配对入口(PWA 的 `#k=…` fragment、原生壳的 Universal/App Link)必须走
 *  同一个函数 —— 它们此前各自写一遍「存下来、设为当前」,而多设备之后这段逻辑
 *  长出了去重、保留用户改名、焦点转移三条规则,复制两份就是等着它们漂移。
 *
 *  返回新簿子;`added` 为 false 表示这张码本来就在册(同一台被扫了第二次)。 */
export function adoptScannedDevice(
  book: DeviceBook,
  secret: string,
  mint: DeviceMint,
  relayBase?: string | null,
  opts?: { focus?: boolean },
): { book: DeviceBook; device: PairedDevice; added: boolean } {
  const { book: added, device, deduped } = addDevice(book, {
    secret,
    relayBase,
    id: mint.id,
    label: mint.label,
    now: mint.now,
  });
  // 原生壳每次启动都把它存的那份配对重新注入一遍(见 mobile-harmony 的
  // WebShell.ets)。那不是一次「扫码」,所以不该把焦点抢回那一台 —— 否则用户在
  // 设备列表里切过去的那一台,每次重开 app 都被打回原形。壳用 `&boot=1` 说明
  // 这是启动重注,而不是刚扫的码。
  const keepFocus = opts?.focus === false && deduped;
  const next = keepFocus ? { ...added, activeId: book.activeId ?? added.activeId } : added;
  persistBook(next);
  return { book: next, device, added: !deduped };
}

/** PWA 的配对入口:地址栏 fragment 里的 `#k=…` 就是一次配对(桌面端二维码编
 *  码的就是这个 URL)。取到后立刻把 fragment 从地址栏抹掉 —— 密钥不该留在那
 *  里被截图、被历史记录带走。
 *
 *  只读一次:调用后 hash 已被清空,所以第二次调用返回 `null`。原生壳走的是
 *  deepLink.ts,不经这条路。
 *
 *  一并取走同一个 fragment 里的 `&relay=` —— 它说的是**这次配对**挂在哪个
 *  relay 上,而 fragment 马上就要被抹掉,所以必须在这里一次读完,不能留给后面
 *  某个模块再去读一遍(那正是 relay.ts 从前那个模块加载期 `RELAY_BASE` 常量
 *  存在的理由)。 */
export function consumeHashSecret(): {
  secret: string;
  relayBase: string | null;
  /** 这次注入是原生壳的**启动重注**,不是用户刚扫的码(`&boot=1`)。 */
  boot: boolean;
} | null {
  const hash = window.location.hash;
  const secret = extractSecretFromUrl(hash);
  if (!secret) return null;
  const relayBase = parseRelayParam(hash);
  const boot = /[#&]boot=1\b/.test(hash);
  history.replaceState(null, "", window.location.pathname);
  return { secret, relayBase, boot };
}

// ── 待办退订 ─────────────────────────────────────────────────────────────────
//
// 移除一台设备时要顺手告诉它的 relay channel「别再往我推」。这一步可能失败
// (relay 不可达、手机离线),而失败的后果是用户明明删掉了一台设备,却继续收到
// 它的通知——点开还找不到对应的卡。所以退订不上就把它记下来,下次启动重试。
//
// 记的是 secret + relayBase,因为退订必须以那个 channel 的身份连上去(channel
// token 由 secret 派生)。它们本来就存在同一个存储里(设备簿),没有新增暴露面。

const PENDING_UNSUB_KEY = "fleet-pending-unsub";
/** 超过这个时长就放弃重试:那台桌面端可能早就不用了,而一条永远失败的记录不该
 *  在每次启动时都去拨一个连不上的地址。 */
const PENDING_UNSUB_TTL_MS = 7 * 24 * 60 * 60 * 1000;

export interface PendingUnsub {
  secret: string;
  relayBase: string | null;
  at: number;
}

export function loadPendingUnsub(now: number): PendingUnsub[] {
  const raw = readLocal(PENDING_UNSUB_KEY);
  if (!raw) return [];
  let value: unknown;
  try {
    value = JSON.parse(raw);
  } catch {
    return [];
  }
  if (!Array.isArray(value)) return [];
  return value.filter((v): v is PendingUnsub => {
    if (typeof v !== "object" || v === null) return false;
    const e = v as Record<string, unknown>;
    if (typeof e.secret !== "string" || !e.secret) return false;
    if (typeof e.at !== "number") return false;
    return now - e.at < PENDING_UNSUB_TTL_MS;
  });
}

function savePendingUnsub(list: PendingUnsub[]): void {
  try {
    if (list.length === 0) localStorage.removeItem(PENDING_UNSUB_KEY);
    else localStorage.setItem(PENDING_UNSUB_KEY, JSON.stringify(list));
  } catch {
    // 存储满 / 隐私模式 —— 退订就只能靠用户手动关通知了,不值得让移除失败
  }
}

/** 记一笔没退成的退订。同一个 secret 只留最新一条。 */
export function addPendingUnsub(entry: PendingUnsub): void {
  const rest = loadPendingUnsub(entry.at).filter((e) => e.secret !== entry.secret);
  savePendingUnsub([...rest, entry]);
}

/** 退订成功后销账。 */
export function dropPendingUnsub(secret: string, now: number): void {
  savePendingUnsub(loadPendingUnsub(now).filter((e) => e.secret !== secret));
}

/** 清空全部配对(「重新配对」入口)。旧键一并清掉,否则下次启动会被上面的迁移
 *  路径原地复活。 */
export function clearBook(): void {
  try {
    localStorage.removeItem(BOOK_KEY);
    localStorage.removeItem(LEGACY_SECRET_KEY);
  } catch {
    // ignore
  }
  void openDb()
    .then(
      (db) =>
        new Promise<void>((resolve) => {
          const tx = db.transaction("kv", "readwrite");
          tx.objectStore("kv").delete(BOOK_KEY);
          tx.objectStore("kv").delete(LEGACY_SECRET_KEY);
          tx.oncomplete = () => resolve();
          tx.onerror = () => resolve();
        }),
    )
    .catch(() => {});
}
