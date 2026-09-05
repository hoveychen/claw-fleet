// 终端页：给一个仓库开真 shell，对齐移动端的 TerminalView。
//
// 和「仓库 → 命令」面板的分工：那边是**跑一条命令并留下执行记录**（快捷命令、
// 重跑、用时），这里是**我要一个 shell**。两边共享同一个 proc 注册表，所以在
// 哪边起的 pty 另一边都看得见 —— 这是刻意的，一个 pty 只该有一个真相来源。
//
// **进程活在后端主机上，不活在这个页面里。** pty host 是分离的（见
// claw-fleet-core/src/proc_runner.rs），切走页面、重启 app 都不杀它。所以换仓库
// 第一件事是把还活着的接回来，一个都没有时才开新的 —— 否则每来一次就多一个孤儿
// shell。

import { invoke } from "@tauri-apps/api/core";
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { FolderOpen, Plus, Square, Trash2 } from "lucide-react";
import { PageShell } from "./PageShell";
import { EmptyState } from "./EmptyState";
import { ProcTerminal } from "./ProcTerminal";
import { distinctWorkspaces } from "./NewSessionForm";
import { useProcStore, useSessionsStore } from "../store";
import type { ProcRecord } from "../types";
import styles from "./MemoryView.module.css";
import termStyles from "./TerminalView.module.css";

/** 标签上显示的命令名：core 的默认 shell 显示成「shell」，其余取第一个词再截断
 *  —— 命令面板起的 `pnpm build` 也会出现在这里。
 *
 *  两种形状都要认：core 现在生成 `exec "/bin/zsh" -i`（绝对路径，见
 *  proc_runner::default_shell_command），而这次改动之前起的、仍活着的 pty 记录里
 *  存的是旧的 `exec "$SHELL" -i`。只认新形状会让老终端的标签显示成「exec」。 */
export function procLabel(proc: ProcRecord, shellLabel: string): string {
  const cmd = proc.command.trim();
  if (/^exec\s+"[^"]*"\s+-i$/.test(cmd) || /^exec\s+"?\$SHELL/.test(cmd) || cmd === "cmd") {
    return shellLabel;
  }
  const head = cmd.split(/\s+/)[0] ?? cmd;
  return head.length > 16 ? `${head.slice(0, 15)}…` : head;
}

export function TerminalView() {
  const { t } = useTranslation();
  const sessions = useSessionsStore((s) => s.sessions);
  const procs = useProcStore((s) => s.procs);
  const fetchProcs = useProcStore((s) => s.fetchProcs);

  const [selected, setSelected] = useState<string | null>(null);
  const [activeId, setActiveId] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  // 一个仓库只自动开一次 shell：接回列表是异步的，没有这道闸，「列表空 → 开一个」
  // 会在下一次渲染时再开一个。
  const autoSpawned = useRef<string | null>(null);

  // 列表与 FilesView / 新建会话共用同一套推导：worktree 折叠回仓库根，临时
  // scratchpad cwd 丢掉。limit 放到最大 —— 下拉框才需要截断，这里是整页。
  const workspaces = useMemo(
    () => distinctWorkspaces(sessions, Number.MAX_SAFE_INTEGER),
    [sessions],
  );

  // 轮询 proc 注册表：标签的存活状态、以及别处（命令面板、手机）起的 pty 都靠它
  // 进来。ProcTerminal 自己的输出轮询只认它挂着的那一个。
  useEffect(() => {
    void fetchProcs();
    const timer = setInterval(() => void fetchProcs(), 2000);
    return () => clearInterval(timer);
  }, [fetchProcs]);

  const wsProcs = useMemo(
    () => procs.filter((p) => p.workspacePath === selected),
    [procs, selected],
  );

  const spawn = useCallback(async () => {
    if (!selected || busy) return;
    setBusy(true);
    setError(null);
    try {
      // 空命令 = 让**后端主机**决定用哪个交互式 shell（unix `$SHELL -i`，
      // Windows `cmd`）。前端猜会在远端 backend 上猜错主机。80x24 只是起点，
      // ProcTerminal 挂上去立刻发真实尺寸。
      const rec = await invoke<ProcRecord>("run_workspace_proc", {
        workspacePath: selected,
        command: "",
        cols: 80,
        rows: 24,
      });
      setActiveId(rec.id);
      void fetchProcs();
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(false);
    }
  }, [selected, busy, fetchProcs]);

  // 换仓库 → 接回它已有的终端，没有活的才开一个。
  useEffect(() => {
    if (!selected) return;
    let stale = false;
    setError(null);
    void (async () => {
      await fetchProcs();
      if (stale) return;
      const list = useProcStore
        .getState()
        .procs.filter((p) => p.workspacePath === selected);
      const live = list.find((p) => p.status !== "exited");
      setActiveId(live?.id ?? list[0]?.id ?? null);
      if (!live && autoSpawned.current !== selected) {
        autoSpawned.current = selected;
        void spawn();
      }
    })();
    return () => {
      stale = true;
    };
    // spawn 依赖 busy，每次开终端都会换引用；这个 effect 只该在换仓库时跑。
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [selected]);

  const active = wsProcs.find((p) => p.id === activeId) ?? null;
  const exited = active?.status === "exited";

  const kill = async () => {
    if (!active) return;
    try {
      await invoke("kill_workspace_proc", { id: active.id, force: false });
    } catch (e) {
      setError(String(e));
    }
    void fetchProcs();
  };

  /** 关掉一个已退出的终端：删记录 + 从标签里摘掉。 */
  const clear = async () => {
    if (!active) return;
    try {
      await invoke("clear_workspace_procs", { id: active.id, workspacePath: null });
    } catch (e) {
      setError(String(e));
    }
    setActiveId(wsProcs.find((p) => p.id !== active.id)?.id ?? null);
    void fetchProcs();
  };

  return (
    <PageShell
      view="terminal"
      title={t("terminal.title")}
      count={workspaces.length > 0 ? workspaces.length : null}
      secondary={
        <div className={styles.list_pane}>
          {workspaces.length === 0 && (
            <EmptyState
              icon={<FolderOpen size={28} strokeWidth={1.5} />}
              title={t("terminal.no_workspaces")}
            />
          )}
          <div className={styles.card_list}>
            {workspaces.map((ws) => (
              <button
                key={ws.path}
                className={`${styles.card} ${selected === ws.path ? styles.card_active : ""}`}
                onClick={() => setSelected(ws.path)}
              >
                <div className={styles.card_body}>
                  <div className={styles.card_title}>{ws.name}</div>
                  <div className={styles.card_hook}>{ws.path}</div>
                </div>
              </button>
            ))}
          </div>
        </div>
      }
    >
      {!selected ? (
        <div className={styles.placeholder}>{t("terminal.select_workspace")}</div>
      ) : (
        <div className={termStyles.pane}>
          <div className={termStyles.bar}>
            <div className={termStyles.tabs}>
              {wsProcs.map((p) => (
                <button
                  key={p.id}
                  className={termStyles.tab}
                  data-active={p.id === activeId}
                  data-exited={p.status === "exited"}
                  onClick={() => setActiveId(p.id)}
                  title={p.command || t("terminal.shell")}
                >
                  {procLabel(p, t("terminal.shell"))}
                </button>
              ))}
            </div>
            <div className={termStyles.actions}>
              {active &&
                (exited ? (
                  <button
                    className={termStyles.icon_btn}
                    onClick={() => void clear()}
                    title={t("terminal.close")}
                  >
                    <Trash2 size={13} strokeWidth={1.6} />
                  </button>
                ) : (
                  <button
                    className={termStyles.icon_btn}
                    onClick={() => void kill()}
                    title={t("terminal.kill")}
                  >
                    <Square size={12} strokeWidth={1.8} />
                  </button>
                ))}
              <button
                className={termStyles.icon_btn}
                onClick={() => void spawn()}
                disabled={busy}
                title={t("terminal.new")}
              >
                <Plus size={14} strokeWidth={1.8} />
              </button>
            </div>
          </div>

          {error && <div className={termStyles.error}>{error}</div>}

          {active ? (
            // key = proc id：切标签必须重建 xterm，否则新终端会继续写进上一个的
            // 缓冲区（ProcTerminal 的 effect 就是按 proc.id 生命周期建的）。
            <div className={termStyles.screen}>
              <ProcTerminal key={active.id} proc={active} height="100%" />
            </div>
          ) : (
            <div className={styles.placeholder}>
              {busy ? t("terminal.starting") : t("terminal.none")}
            </div>
          )}

          {exited && active && (
            <div className={termStyles.exit_bar}>
              <span>
                {active.exitCode === null || active.exitCode === undefined
                  ? t("terminal.exited")
                  : t("terminal.exited_code", { code: active.exitCode })}
              </span>
              <button className={termStyles.exit_action} onClick={() => void spawn()}>
                {t("terminal.restart")}
              </button>
            </div>
          )}
        </div>
      )}
    </PageShell>
  );
}
