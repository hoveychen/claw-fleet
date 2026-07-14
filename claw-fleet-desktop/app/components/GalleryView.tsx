import { useState } from "react";
import { useTranslation } from "react-i18next";
import { useDetailStore, useSessionsStore } from "../store";
import { useSessionSearch } from "../hooks/useSessionSearch";
import type { SessionInfo, SessionStatus } from "../types";
import { isWorkflowAgent } from "../workflowAgent";
import { SessionCard, StatusIcon, SubagentTypeIcon, formatModel } from "./SessionCard";
import { SessionsPage } from "./SessionsBanner";
import { SessionEmptyState } from "./EmptyState";
import styles from "./GalleryView.module.css";
import sessionStyles from "./SessionCard.module.css";

// ── Helpers ────────────────────────────────────────────────────────────────

// "rateLimited" counts as active: a rate-limited session is unfinished work
// waiting to resume, so it stays in the default (active-only) view and the
// active count instead of vanishing into the "show all" tail. Its card keeps a
// distinct look via RateLimitControls (countdown + resume), and SessionCard's
// own isActive (live-process pulse / stop button) deliberately excludes it.
const ACTIVE_STATUSES: SessionStatus[] = [
  "thinking", "executing", "streaming", "processing", "waitingInput", "active", "delegating", "rateLimited",
];

function isActive(s: SessionInfo) {
  return ACTIVE_STATUSES.includes(s.status);
}

// ── SubagentRow ───────────────────────────────────────────────────────────

interface ChipProps {
  session: SessionInfo;
  index: number;
  onSelect: (s: SessionInfo) => void;
}

function SubagentRow({ session, index, onSelect }: ChipProps) {
  const { t } = useTranslation();
  const active = isActive(session);

  return (
    <button
      className={`${styles.sub_row} ${active ? styles.sub_row_active : styles.sub_row_idle}`}
      onClick={(e) => { e.stopPropagation(); onSelect(session); }}
      title={session.agentDescription ?? session.agentType ?? t("subagent")}
    >
      <span className={sessionStyles[`badge_${session.status}`]}>
        <StatusIcon status={session.status} />
      </span>
      <span className={styles.sub_index}>#{index}</span>
      <span className={styles.sub_type_icon}>
        <SubagentTypeIcon type={session.agentType} />
      </span>
      <span className={styles.sub_desc}>
        {session.agentDescription ?? session.agentType ?? t("subagent")}
      </span>
      {session.model && (
        <span className={styles.sub_model}>{formatModel(session.model)}</span>
      )}
      {session.tokenSpeed >= 0.5 && (
        <span className={styles.sub_speed}>
          {session.tokenSpeed.toFixed(1)} {t("tok_s")}
        </span>
      )}
    </button>
  );
}

// ── GalleryRow ────────────────────────────────────────────────────────────

interface RowProps {
  main: SessionInfo;
  subagents: SessionInfo[];
  onSelect: (s: SessionInfo) => void;
}

function GalleryRow({ main, subagents, onSelect }: RowProps) {
  const { t } = useTranslation();
  const [idleExpanded, setIdleExpanded] = useState(false);

  // Stable sort by jsonlPath for consistent indices regardless of status changes
  const sortedSubs = [...subagents].sort((a, b) => a.jsonlPath.localeCompare(b.jsonlPath));
  const subIndexMap = new Map(sortedSubs.map((s, i) => [s.id, i + 1]));

  const activeSubagents = subagents.filter(isActive);
  const idleSubagents = subagents.filter((s) => !isActive(s));

  const groupActive = isActive(main) || activeSubagents.length > 0;

  return (
    <div className={styles.group} data-active={groupActive || undefined}>
      {/* Group header — minimal: just the workspace name, clickable to open main session */}
      <div className={styles.group_header} onClick={() => onSelect(main)}>
        <span className={styles.group_name}>{main.workspaceName}</span>
      </div>

      {/* Main agent card */}
      <div className={styles.group_body}>
        <SessionCard
          session={main}
          isSelected={false}
          onClick={() => onSelect(main)}
          variant="group-main"
          hideHeader
          subagentCount={subagents.length}
          runningSubagentCount={activeSubagents.length}
        />
      </div>

      {/* Active subagent chips */}
      {activeSubagents.length > 0 && (
        <div className={styles.sub_list}>
          {activeSubagents.map((sub) => (
            <SubagentRow key={sub.jsonlPath} session={sub} index={subIndexMap.get(sub.id) ?? 0} onSelect={onSelect} />
          ))}
        </div>
      )}

      {/* Idle subagents (collapsible) */}
      {idleSubagents.length > 0 && (
        <div className={styles.idle_section}>
          <button
            className={styles.idle_toggle}
            onClick={() => setIdleExpanded((v) => !v)}
          >
            <span className={`${styles.idle_chevron} ${idleExpanded ? styles.idle_chevron_open : ""}`} />
            {t("gallery.idle_subs", { n: idleSubagents.length })}
          </button>
          {idleExpanded && (
            <div className={styles.sub_list}>
              {idleSubagents.map((sub) => (
                <SubagentRow key={sub.jsonlPath} session={sub} index={subIndexMap.get(sub.id) ?? 0} onSelect={onSelect} />
              ))}
            </div>
          )}
        </div>
      )}
    </div>
  );
}

// ── buildRows ─────────────────────────────────────────────────────────────
// mains: non-subagent sessions to render rows for (already filtered/sorted by caller)
// allSessions: full session list used to look up subagents by parent id

function buildRows(
  mains: SessionInfo[],
  allSessions: SessionInfo[],
  onSelect: (s: SessionInfo) => void,
) {
  // Build subagent map from ALL sessions so idle subs of active mains are included.
  // Workflow fan-out agents (jsonlPath under subagents/workflows/) are kept in the
  // session list only so a DAG node can open them — they must NOT inflate the
  // group's "+N sub" count or render as cards (a single run can have 100+).
  const subByParent = new Map<string, SessionInfo[]>();
  for (const s of allSessions) {
    if (s.isSubagent && s.parentSessionId && !isWorkflowAgent(s)) {
      const arr = subByParent.get(s.parentSessionId) ?? [];
      arr.push(s);
      subByParent.set(s.parentSessionId, arr);
    }
  }

  const mainSessions = mains.filter((s) => !s.isSubagent);
  const sortedMains = [
    ...mainSessions.filter(isActive),
    ...mainSessions.filter((s) => !isActive(s)),
  ];

  return (
    <>
      {sortedMains.map((main) => (
        <GalleryRow
          key={main.jsonlPath}
          main={main}
          subagents={subByParent.get(main.id) ?? []}
          onSelect={onSelect}
        />
      ))}
    </>
  );
}

// ── GalleryView ───────────────────────────────────────────────────────────

export function GalleryView() {
  const { t } = useTranslation();
  const sessions = useSessionsStore((s) => s.sessions);
  const scanReady = useSessionsStore((s) => s.scanReady);
  const { open, close, session: openSession } = useDetailStore();
  const [filter, setFilter] = useState("");
  const [showAll, setShowAll] = useState(false);
  const handleSelect = (s: SessionInfo) => open(s);
  const gridCls = `${styles.rows_grid}${openSession ? ` ${styles.compact}` : ""}`;

  // Close detail drawer when clicking on empty gallery space
  const handleGridClick = (e: React.MouseEvent) => {
    if (e.target === e.currentTarget && openSession) {
      close();
    }
  };

  const activeSessions = sessions.filter(isActive);
  const { searching, ftsMatchPaths } = useSessionSearch(filter);

  // Filter source: active only or all sessions
  const filterSource = showAll ? sessions : activeSessions;

  const filtered = filter
    ? filterSource.filter((s) => {
        const q = filter.toLowerCase();
        const clientMatch =
          s.workspaceName.toLowerCase().includes(q) ||
          s.aiTitle?.toLowerCase().includes(q) ||
          s.slug?.toLowerCase().includes(q) ||
          s.agentDescription?.toLowerCase().includes(q) ||
          s.ideName?.toLowerCase().includes(q);
        return clientMatch || ftsMatchPaths.has(s.jsonlPath);
      })
    : filterSource;

  // Only pass non-subagent sessions as mains; subagents are looked up from sessions
  const filteredMains = filtered.filter((s) => !s.isSubagent);

  const filteredActiveMains = showAll ? filteredMains.filter(isActive) : filteredMains;
  const filteredRecentMains = showAll
    ? filteredMains.filter((s) => !isActive(s)).sort((a, b) => b.lastActivityMs - a.lastActivityMs)
    : [];

  return (
    <SessionsPage
      filter={filter}
      onFilterChange={setFilter}
      activeCount={activeSessions.filter((s) => !s.isSubagent).length}
      totalCount={sessions.length}
      showAll={showAll}
      onToggleShowAll={() => setShowAll((v) => !v)}
      ftsMatchCount={filter.trim().length >= 2 ? ftsMatchPaths.size : undefined}
      searching={searching}
    >
      <div className={styles.grid} onClick={handleGridClick}>
        {showAll ? (
          <>
            {filteredActiveMains.length > 0 && (
              <div className={styles.section}>
                <div className={styles.section_label}>{t("active")}</div>
                <div className={gridCls} onClick={handleGridClick}>
                  {buildRows(filteredActiveMains, sessions, handleSelect)}
                </div>
              </div>
            )}
            {filteredRecentMains.length > 0 && (
              <div className={styles.section}>
                <div className={styles.section_label}>{t("recent")}</div>
                <div className={gridCls} onClick={handleGridClick}>
                  {buildRows(filteredRecentMains, sessions, handleSelect)}
                </div>
              </div>
            )}
            {filteredMains.length === 0 && (
              <SessionEmptyState scanReady={scanReady} hasSessions={sessions.length > 0} />
            )}
          </>
        ) : (
          <>
            <div className={gridCls} onClick={handleGridClick}>
              {buildRows(filteredActiveMains, sessions, handleSelect)}
            </div>
            {filteredActiveMains.length === 0 && (
              <SessionEmptyState scanReady={scanReady} hasSessions={sessions.length > 0} />
            )}
          </>
        )}
      </div>
    </SessionsPage>
  );
}
