import { useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import type { TFunction } from "i18next";
import { ChevronDown } from "lucide-react";
import { ContextMenu, type ContextMenuAnchor, type ContextMenuItem } from "./ContextMenu";
import type { SessionInfo } from "../types";
import styles from "./SessionDetail.module.css";

/** The header identity label for one session in the family: ◈ main, or ⎇ its
 *  agent type (falling back to the generic "subagent" word when untyped). */
function agentLabel(s: SessionInfo, t: TFunction): string {
  return s.isSubagent ? `⎇ ${s.agentType ?? t("subagent")}` : `◈ ${t("main")}`;
}

/** Distinguishing tail of a subagent id, so two same-type subagents
 *  (e.g. both `general-purpose`) don't read as one indistinct row. Uses the
 *  end of the id, not the start: real ids (`agent-<uuid>`) and mock ids
 *  (`sess-fleet-*`) alike share a common prefix but diverge at the tail. */
function agentIdTail(id: string): string {
  const raw = id.replace(/^agent-/, "");
  return `#${raw.slice(-6)}`;
}

/**
 * Agent scope selector for the detail header.
 *
 * The session being inspected has a family — a main process plus any subagents
 * it spawned — and every facet (对话 / 决策 / Token / …) is scoped to whichever
 * member is selected (`onOpen` re-scopes the whole detail view, not just the
 * transcript). This used to be a segmented strip sharing the tab row with the
 * view tabs, but subagents appear dynamically and unboundedly, so the strip
 * crowded the fixed view tabs off the right edge. Here the scope is a dropdown
 * that grows downward instead of sideways, leaving the view-tab row fixed.
 *
 * With no family to switch between (no subagents) it degrades to a plain
 * identity tag — the same look a solo session had before this refactor.
 */
export function AgentScopeSwitcher({
  tabs,
  current,
  onOpen,
}: {
  tabs: SessionInfo[];
  current: SessionInfo;
  onOpen: (s: SessionInfo) => void;
}) {
  const { t } = useTranslation();
  const btnRef = useRef<HTMLButtonElement>(null);
  const [anchor, setAnchor] = useState<ContextMenuAnchor | null>(null);

  const tagClass = current.isSubagent ? styles.tag_subagent : styles.tag_main;

  // Nothing to switch to → the identity tag with no dropdown affordance.
  if (tabs.length === 0) {
    return <span className={tagClass}>{agentLabel(current, t)}</span>;
  }

  const items: ContextMenuItem[] = tabs.map((s) => {
    const isCurrent = s.id === current.id;
    return {
      id: s.id,
      label: isCurrent ? `${agentLabel(s, t)} · ${t("detail.agent_current")}` : agentLabel(s, t),
      icon: <span className={styles.tab_dot} data-status={s.status} />,
      sub: s.isSubagent ? agentIdTail(s.id) : undefined,
      onSelect: () => {
        if (!isCurrent) onOpen(s);
      },
    };
  });

  return (
    <>
      <button
        ref={btnRef}
        type="button"
        className={`${styles.agent_scope_trigger} ${tagClass} ${anchor ? styles.agent_scope_trigger_open : ""}`}
        aria-haspopup="menu"
        aria-expanded={anchor != null}
        title={t("detail.switch_agent")}
        onClick={() => {
          if (anchor) {
            setAnchor(null);
            return;
          }
          // Hang the menu off the trigger's bottom-left; ContextMenu clamps to
          // the viewport, so a trigger near an edge still lands on-screen.
          const r = btnRef.current?.getBoundingClientRect();
          if (!r) return;
          setAnchor({ x: r.left, y: r.bottom + 4 });
        }}
      >
        {agentLabel(current, t)}
        <ChevronDown size={11} className={styles.agent_scope_chevron} />
      </button>
      {anchor && <ContextMenu anchor={anchor} items={items} onClose={() => setAnchor(null)} />}
    </>
  );
}
