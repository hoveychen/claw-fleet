// 终端页：任务页顶栏「终端」进来的整页浮层。
//
// 一个工作区可以同时开几个 pty，顶部标签在它们之间切。**进程活在桌面端主机上，
// 不活在这个页面里** —— pty host 是分离的，关掉页面、退出浏览器、换台设备，
// 命令都还在跑。所以进来第一件事是 `listProcs` 把还活着的接回来，只有一个都没有
// 时才开新的；否则每次打开面板都会多一个孤儿 shell。

import { lazy, Suspense, useCallback, useEffect, useRef, useState } from "react";
import { ChevronLeft, Folder, Plus, Square, Trash2 } from "lucide-react";
import { EmptyState } from "./EmptyState";
import { t } from "../i18n";
import type { FleetTransport } from "../transport";
import { clearProc, killProc, listProcs, runProc, type ProcRecord } from "../terminal";
import { isDefaultShellCommand } from "../../../shared-ts/procShell";
import styles from "./TerminalView.module.css";

// xterm 及其 CSS 只在真的开了终端时才下载 —— 见 TerminalPane 顶部的说明。
const TerminalPane = lazy(() => import("./TerminalPane"));

/** 一个可开终端的工作区。deviceId 决定这条命令落在哪台主机上 —— 多设备时两台
 *  机器可以有同名甚至同路径的工作区，只带路径会开错机器。 */
export interface TerminalWorkspace {
  deviceId: string;
  path: string;
  name: string;
}

interface Props {
  /** 可选工作区（取自任务列表里出现过的那些）。 */
  workspaces: TerminalWorkspace[];
  /** 任务页里已经筛好了某个目录时带进来，省掉一次选择。 */
  initial?: TerminalWorkspace | null;
  clientFor: (deviceId: string) => FleetTransport | null;
  onBack: () => void;
}

/** 键位条：软键盘上没有的键。发的是完整转义序列，不再经粘滞 Ctrl 折一次。 */
const KEYS: Array<{ label: string; data: string }> = [
  { label: "Esc", data: "\x1b" },
  { label: "Tab", data: "\t" },
  { label: "^C", data: "\x03" },
  { label: "^D", data: "\x04" },
  { label: "↑", data: "\x1b[A" },
  { label: "↓", data: "\x1b[B" },
  { label: "←", data: "\x1b[D" },
  { label: "→", data: "\x1b[C" },
];

/** 软键盘遮住的高度。
 *
 *  iOS 上软键盘不会缩小布局视口，它是盖上来的 —— 而这个页面是 `position:fixed;
 *  inset:0`，于是键位条和终端底部会整个被盖住，正在输入的那一行反而看不见。
 *  visualViewport 是唯一能问出「实际还剩多少可视高度」的接口。 */
function useKeyboardInset(): number {
  const [inset, setInset] = useState(0);
  useEffect(() => {
    const vv = window.visualViewport;
    if (!vv) return; // 老浏览器：退回布局视口，键盘弹起时自然缩小（安卓多数如此）
    const apply = () => {
      setInset(Math.max(0, window.innerHeight - vv.height - vv.offsetTop));
    };
    apply();
    vv.addEventListener("resize", apply);
    vv.addEventListener("scroll", apply);
    return () => {
      vv.removeEventListener("resize", apply);
      vv.removeEventListener("scroll", apply);
    };
  }, []);
  return inset;
}

/** 标签上显示的命令名：默认 shell 显示成「shell」，其余取第一个词，太长再截断。
 *
 *  「是不是默认 shell」的判断跟桌面端共用一份（shared-ts/procShell.ts）——这里曾经
 *  各存一份正则，core 把默认命令从 `$SHELL` 改成绝对路径之后只同步了一份，另一份就
 *  把默认 shell 显示成了「exec」。截断长度是移动端自己的（窄屏 14 而不是 16）。 */
function procLabel(proc: ProcRecord): string {
  const cmd = proc.command.trim();
  if (isDefaultShellCommand(cmd)) return t("shell");
  const head = cmd.split(/\s+/)[0] ?? cmd;
  return head.length > 14 ? `${head.slice(0, 13)}…` : head;
}

export function TerminalView({ workspaces, initial, clientFor, onBack }: Props) {
  const [ws, setWs] = useState<TerminalWorkspace | null>(initial ?? null);
  const [procs, setProcs] = useState<ProcRecord[]>([]);
  const [activeId, setActiveId] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  // 一个工作区只自动开一次 shell：接回列表是异步的，没有这道闸，
  // 「列表空 → 开一个」会在第二次渲染时再开一个。
  const autoSpawned = useRef<string | null>(null);
  // 键位条 → 当前终端的通道。TerminalPane 挂上时把 send 交过来，卸载时交 null。
  const sendRef = useRef<((data: string) => void) | null>(null);
  const [ctrl, setCtrl] = useState(false);
  const keyboardInset = useKeyboardInset();

  const client = ws ? clientFor(ws.deviceId) : null;

  const spawn = useCallback(async () => {
    if (!client || !ws || busy) return;
    setBusy(true);
    setError(null);
    try {
      // 空命令 = 让主机自己决定用哪个交互式 shell（unix `$SHELL -i`，
      // Windows `cmd`）。80x24 只是个起点，面板挂上去会立刻发真实尺寸。
      const rec = await runProc(client, ws.path, "", 80, 24);
      setProcs((prev) => [rec, ...prev]);
      setActiveId(rec.id);
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setBusy(false);
    }
  }, [client, ws, busy]);

  // 换工作区 → 接回它已有的终端。
  useEffect(() => {
    if (!client || !ws) return;
    let stale = false;
    setError(null);
    void (async () => {
      try {
        const list = await listProcs(client, ws.path);
        if (stale) return;
        setProcs(list);
        const live = list.find((p) => p.status !== "exited");
        setActiveId(live?.id ?? list[0]?.id ?? null);
        if (!live && autoSpawned.current !== ws.path) {
          autoSpawned.current = ws.path;
          void spawn();
        }
      } catch (e) {
        if (!stale) setError(e instanceof Error ? e.message : String(e));
      }
    })();
    return () => {
      stale = true;
    };
    // spawn 依赖 busy，会在每次开新终端时变引用；这个 effect 只该在换工作区/
    // 换设备时跑，所以刻意不把它列进来。
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [client, ws?.path]);

  const active = procs.find((p) => p.id === activeId) ?? null;

  /** 输出轮询顺便带回来的最新记录 —— 拿它更新标签上的存活状态。 */
  const onRecord = useCallback((rec: ProcRecord) => {
    setProcs((prev) => prev.map((p) => (p.id === rec.id ? rec : p)));
  }, []);

  const handleKill = useCallback(async () => {
    if (!client || !active) return;
    try {
      await killProc(client, active.id);
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    }
  }, [client, active]);

  /** 关掉一个已退出的终端：删记录 + 从标签里摘掉。 */
  const handleClear = useCallback(async () => {
    if (!client || !active) return;
    try {
      await clearProc(client, active.id);
      setProcs((prev) => {
        const rest = prev.filter((p) => p.id !== active.id);
        setActiveId(rest[0]?.id ?? null);
        return rest;
      });
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    }
  }, [client, active]);

  // ── 还没选工作区：先选 ────────────────────────────────────────────────
  if (!ws) {
    return (
      <div className={styles.page}>
        <div className={styles.header}>
          <button className={styles.backButton} onClick={onBack}>
            <ChevronLeft size={20} />
            {t("返回")}
          </button>
          <div className={styles.headerTitle}>{t("终端")}</div>
        </div>
        {workspaces.length === 0 ? (
          <EmptyState icon={Folder} title={t("还没有可用的工作目录")} />
        ) : (
          <div className={styles.pickList}>
            {workspaces.map((w) => (
              <button
                key={`${w.deviceId}::${w.path}`}
                className={styles.pickRow}
                onClick={() => setWs(w)}
              >
                <Folder size={15} />
                <span className={styles.pickName}>{w.name}</span>
                <span className={styles.pickPath}>{w.path}</span>
              </button>
            ))}
          </div>
        )}
      </div>
    );
  }

  const exited = active?.status === "exited";

  return (
    <div className={styles.page} style={{ bottom: keyboardInset }}>
      <div className={styles.header}>
        <button className={styles.backButton} onClick={onBack}>
          <ChevronLeft size={20} />
          {t("返回")}
        </button>
        <button
          className={styles.headerTitle}
          onClick={() => setWs(null)}
          title={ws.path}
        >
          {ws.name}
        </button>
        {exited ? (
          <button className={styles.iconButton} onClick={() => void handleClear()}>
            <Trash2 size={16} />
          </button>
        ) : (
          active && (
            <button className={styles.iconButton} onClick={() => void handleKill()}>
              <Square size={14} />
            </button>
          )
        )}
        <button
          className={styles.iconButton}
          onClick={() => void spawn()}
          disabled={busy}
          aria-label={t("新终端")}
        >
          <Plus size={18} />
        </button>
      </div>

      {procs.length > 1 && (
        <div className={styles.tabs}>
          {procs.map((p) => (
            <button
              key={p.id}
              className={styles.tab}
              data-active={p.id === activeId}
              data-exited={p.status === "exited"}
              onClick={() => setActiveId(p.id)}
            >
              {procLabel(p)}
            </button>
          ))}
        </div>
      )}

      {error && <div className={styles.error}>{error}</div>}

      {client && active ? (
        <Suspense fallback={<EmptyState compact icon={Folder} title={t("加载中…")} />}>
          <TerminalPane
            key={active.id}
            client={client}
            proc={active}
            onRecord={onRecord}
            registerInput={(send) => {
              sendRef.current = send;
            }}
            ctrl={ctrl}
            onCtrlConsumed={() => setCtrl(false)}
          />
        </Suspense>
      ) : (
        <EmptyState compact icon={Folder} title={busy ? t("正在开终端…") : t("没有终端")} />
      )}

      {!exited && active && (
        <div className={styles.keyBar}>
          <button
            className={styles.key}
            data-sticky={ctrl}
            // onPointerDown + preventDefault:按下就发，且不让浏览器把焦点从
            // xterm 的隐藏 textarea 上挪走 —— 焦点一丢，软键盘就收起来了，
            // 而键位条本来就是配着软键盘用的。
            onPointerDown={(e) => {
              e.preventDefault();
              setCtrl((v) => !v);
            }}
          >
            Ctrl
          </button>
          {KEYS.map((k) => (
            <button
              key={k.label}
              className={styles.key}
              onPointerDown={(e) => {
                e.preventDefault();
                sendRef.current?.(k.data);
              }}
            >
              {k.label}
            </button>
          ))}
        </div>
      )}

      {exited && active && (
        <div className={styles.exitBar}>
          {active.exitCode === null || active.exitCode === undefined
            ? t("已退出")
            : t("已退出 · 退出码 {0}", String(active.exitCode))}
          <button className={styles.exitAction} onClick={() => void spawn()}>
            {t("重开")}
          </button>
        </div>
      )}
    </div>
  );
}
