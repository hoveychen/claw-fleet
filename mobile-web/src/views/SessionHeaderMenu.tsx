// 会话详情页 header 右上角的汉堡菜单 —— 那些「需要时才拿一次」的东西：会话
// id、两条路径、恢复命令，外加把详情面板叫出来的入口，以及在主进程与各子代理
// 之间切作用域 —— header 上原先那个作用域下拉已经整个搬到这里，那 83px 现在
// 归标题。这里是带状态点、全名、子代理 id 尾巴的完整清单。
//
// 桌面端的对应物是 SessionHeaderMenu.tsx（同一批条目，用 ContextMenu 吊在按钮
// 下方）。手机上改成底部 sheet：拇指够得着，且不必为一个浮层算 viewport 夹取。
//
// 复制一律 await 后再显示结果 —— 移动端浏览器在非安全上下文 / 无用户手势时会
// 拒掉 `navigator.clipboard.writeText`，发后不管就会出现「显示已复制、剪贴板里
// 什么都没有」（同 CopyButton 顶部那段注释）。

import { useCallback, useEffect, useRef, useState, type ReactNode } from "react";
import { createPortal } from "react-dom";
import { Check, Copy, FileJson2, Folder, Info, Menu, Terminal, X } from "lucide-react";
import { t } from "../i18n";
import { HistoryLayer } from "../useNavStack";
import type { SessionInfo } from "../types";
import { agentIdTail, agentLabel } from "./agentScope";
import { resumeCommand } from "./sessionInfoRows";
import styles from "./SessionHeaderMenu.module.css";

interface MenuItem {
  id: string;
  label: string;
  /** 副行：要复制的那段文本本身，好让人不用复制就能核一眼。 */
  sub?: string;
  icon: ReactNode;
  /** 要写进剪贴板的文本；缺席表示这是个纯动作条目。 */
  copy?: string;
  onSelect?: () => void;
}

export function SessionHeaderMenu({
  session,
  family,
  onOpenSession,
  infoOpen,
  onToggleInfo,
}: {
  session: SessionInfo;
  /** 主进程 + 各子代理（调用方按桌面端同一套规则组装、排序、封顶）。为空表示
   *  这是个没有子代理的独会话，这一节整段不出现。 */
  family: SessionInfo[];
  onOpenSession: (s: SessionInfo) => void;
  /** 详情面板当前是否展开 —— 决定菜单里那条是「查看」还是「收起」。 */
  infoOpen: boolean;
  onToggleInfo: () => void;
}) {
  const [open, setOpen] = useState(false);
  // 刚点过的条目 → 它的结果。菜单关掉就清空。
  const [result, setResult] = useState<{ id: string; ok: boolean } | null>(null);
  const closeTimer = useRef<number | undefined>(undefined);

  useEffect(() => () => window.clearTimeout(closeTimer.current), []);
  // 换会话时把菜单收掉，免得停留在上一个会话的 id 上。
  useEffect(() => {
    setOpen(false);
    setResult(null);
  }, [session.id]);

  const close = useCallback(() => {
    window.clearTimeout(closeTimer.current);
    setOpen(false);
    setResult(null);
  }, []);

  const run = useCallback(
    async (item: MenuItem) => {
      if (item.copy === undefined) {
        item.onSelect?.();
        close();
        return;
      }
      window.clearTimeout(closeTimer.current);
      try {
        await navigator.clipboard.writeText(item.copy);
        setResult({ id: item.id, ok: true });
        // 让 ✓ 停留一拍再收 —— 立刻关掉会让人不确定到底复制成功没有。
        closeTimer.current = window.setTimeout(close, 700);
      } catch {
        // 失败就把菜单留在原地：错误提示得有地方显示，人也还能再试一次。
        setResult({ id: item.id, ok: false });
      }
    },
    [close],
  );

  const title = session.titleOverride || session.aiTitle || session.slug || "";
  const resume = resumeCommand(session);

  const items: MenuItem[] = [
    {
      id: "info",
      label: infoOpen ? t("收起会话详情") : t("查看会话详情"),
      icon: <Info size={16} />,
      onSelect: onToggleInfo,
    },
    {
      id: "copy-id",
      label: t("复制会话 ID"),
      sub: session.id,
      icon: <Copy size={16} />,
      copy: session.id,
    },
  ];
  if (title) {
    items.push({
      id: "copy-title",
      label: t("复制标题"),
      sub: title,
      icon: <Copy size={16} />,
      copy: title,
    });
  }
  items.push(
    {
      id: "copy-workspace",
      label: t("复制工作区路径"),
      sub: session.workspacePath,
      icon: <Folder size={16} />,
      copy: session.workspacePath,
    },
    {
      id: "copy-transcript",
      label: t("复制会话记录路径"),
      sub: session.jsonlPath,
      icon: <FileJson2 size={16} />,
      copy: session.jsonlPath,
    },
  );
  if (resume) {
    items.push({
      id: "copy-resume",
      label: t("复制恢复命令"),
      sub: resume,
      icon: <Terminal size={16} />,
      copy: resume,
    });
  }

  return (
    <>
      <button
        type="button"
        className={styles.menuButton}
        aria-label={t("更多")}
        aria-haspopup="menu"
        aria-expanded={open}
        onClick={() => (open ? close() : setOpen(true))}
      >
        <Menu size={18} />
      </button>
      {/* Portalled to <body> on purpose: the detail page is `position:fixed;
          z-index:30`, i.e. its own stacking context, so a sheet rendered inside
          it can never rise above the decision drawer's z-45 bar no matter what
          z-index it takes — measured: the bar covered the last two menu items. */}
      {/* 菜单也算一层：不登记的话返回键弹掉的是整个会话详情页。 */}
      {open && <HistoryLayer onBack={close} />}
      {open && createPortal(
        <div className={styles.backdrop} onClick={close}>
          <div
            className={styles.sheet}
            role="menu"
            onClick={(e) => e.stopPropagation()}
          >
            <div className={styles.sheetHead}>
              <span className={styles.sheetTitle}>{t("会话操作")}</span>
              <button className={styles.sheetClose} onClick={close} aria-label={t("关闭")}>
                <X size={18} />
              </button>
            </div>
            {family.length > 0 && (
              <>
                <div className={styles.sectionLabel}>{t("切换代理")}</div>
                {family.map((s) => {
                  const isCurrent = s.id === session.id;
                  return (
                    <button
                      key={s.id}
                      type="button"
                      role="menuitem"
                      className={styles.item}
                      data-current={isCurrent ? "" : undefined}
                      onClick={() => {
                        close();
                        if (!isCurrent) onOpenSession(s);
                      }}
                    >
                      <span className={styles.statusDot} data-status={s.status} />
                      <span className={styles.itemText}>
                        <span className={styles.itemLabel}>
                          {agentLabel(s)}
                          {isCurrent && ` · ${t("当前")}`}
                        </span>
                        {s.isSubagent && (
                          <span className={styles.itemSub}>{agentIdTail(s.id)}</span>
                        )}
                      </span>
                    </button>
                  );
                })}
                <div className={styles.sectionLabel}>{t("会话")}</div>
              </>
            )}
            {items.map((item) => {
              const r = result?.id === item.id ? result : null;
              return (
                <button
                  key={item.id}
                  type="button"
                  role="menuitem"
                  className={styles.item}
                  data-failed={r && !r.ok ? "" : undefined}
                  onClick={() => void run(item)}
                >
                  <span className={styles.itemIcon}>
                    {r ? r.ok ? <Check size={16} /> : <X size={16} /> : item.icon}
                  </span>
                  <span className={styles.itemText}>
                    <span className={styles.itemLabel}>
                      {r && !r.ok ? t("复制失败（需要 HTTPS 或用户手势）") : item.label}
                    </span>
                    {item.sub && <span className={styles.itemSub}>{item.sub}</span>}
                  </span>
                </button>
              );
            })}
          </div>
        </div>,
        document.body,
      )}
    </>
  );
}
