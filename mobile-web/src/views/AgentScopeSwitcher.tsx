import { useState } from "react";
import { ChevronDown } from "lucide-react";
import type { SessionInfo } from "../types";
import { t } from "../i18n";
import styles from "./SessionDetailView.module.css";

/** Header identity label for one family member: ◈ main, or ⎇ its agent type
 *  (falling back to the generic "subagent" word when untyped). Mirrors the
 *  desktop AgentScopeSwitcher. */
function agentLabel(s: SessionInfo): string {
  return s.isSubagent ? `⎇ ${s.agentType || t("子代理")}` : `◈ ${t("主进程")}`;
}

/** Distinguishing tail of a subagent id, so two same-type subagents (e.g. both
 *  `general-purpose`) don't read as one indistinct row. Uses the end of the id:
 *  real ids (`agent-<uuid>`) share the `agent-` prefix but diverge at the tail. */
function agentIdTail(id: string): string {
  const raw = id.replace(/^agent-/, "");
  return `#${raw.slice(-6)}`;
}

/**
 * Agent scope selector for the mobile detail header — the phone counterpart of
 * the desktop `AgentScopeSwitcher`. The session being viewed has a family (a
 * main process plus any subagents it spawned); tapping a member re-scopes the
 * whole detail view via `onOpen` (which pushes it as a new stack layer).
 *
 * `family` is assembled by the caller the same way the desktop does — matched by
 * `parentSessionId`, workflow fan-out agents excluded, active members first,
 * capped — so running subagents appear the moment their transcript is scanned,
 * with no dependence on a completed Agent tool result. The caller only renders
 * this when `family` is non-empty (there is something to switch between).
 */
export function AgentScopeSwitcher({
  family,
  current,
  onOpen,
}: {
  family: SessionInfo[];
  current: SessionInfo;
  onOpen: (s: SessionInfo) => void;
}) {
  const [open, setOpen] = useState(false);

  return (
    <span className={styles.scopeWrap}>
      <button
        type="button"
        className={styles.scopeTrigger}
        data-subagent={current.isSubagent}
        data-open={open}
        aria-haspopup="menu"
        aria-expanded={open}
        title={t("切换代理")}
        onClick={(e) => {
          e.stopPropagation();
          setOpen((o) => !o);
        }}
      >
        {/* The label owns the ellipsis: a long agentType shortens here instead of
            widening the trigger until the session title has no room left. */}
        <span className={styles.scopeTriggerLabel}>{agentLabel(current)}</span>
        <ChevronDown size={11} className={styles.scopeChevron} />
      </button>
      {open && (
        <>
          {/* Full-screen catcher so a tap anywhere else closes the menu. */}
          <div className={styles.scopeBackdrop} onClick={() => setOpen(false)} />
          <div className={styles.scopeMenu} role="menu">
            {family.map((s) => {
              const isCurrent = s.id === current.id;
              return (
                <button
                  key={s.id}
                  type="button"
                  role="menuitem"
                  className={styles.scopeItem}
                  data-current={isCurrent}
                  onClick={() => {
                    setOpen(false);
                    if (!isCurrent) onOpen(s);
                  }}
                >
                  <span className={styles.scopeDot} data-status={s.status} />
                  <span className={styles.scopeItemLabel}>
                    {agentLabel(s)}
                    {isCurrent && <span className={styles.scopeCurrent}> · {t("当前")}</span>}
                  </span>
                  {s.isSubagent && <span className={styles.scopeItemTail}>{agentIdTail(s.id)}</span>}
                </button>
              );
            })}
          </div>
        </>
      )}
    </span>
  );
}
