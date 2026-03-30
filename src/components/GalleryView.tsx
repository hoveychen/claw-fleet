import { useState } from "react";
import { useTranslation } from "react-i18next";
import { useDetailStore, useSessionsStore } from "../store";
import { useSessionSearch } from "../hooks/useSessionSearch";
import type { SessionInfo, SessionStatus } from "../types";
import { SessionCard, StatusBadge, StatusIcon, SubagentTypeIcon, formatModel } from "./SessionCard";
import { SessionToolbar } from "./SessionToolbar";
import styles from "./GalleryView.module.css";

// ── Helpers ────────────────────────────────────────────────────────────────

const ACTIVE_STATUSES: SessionStatus[] = [
  "thinking", "executing", "streaming", "processing", "waitingInput", "active", "delegating",
];

function isActive(s: SessionInfo) {
  return ACTIVE_STATUSES.includes(s.status);
}

// ── Stable chip color from session id ─────────────────────────────────────

const CHIP_HUES = [210, 160, 30, 350, 280, 55, 330, 120, 190, 90];

function chipHue(id: string): number {
  let h = 0;
  for (let i = 0; i < id.length; i++) h = (h * 31 + id.charCodeAt(i)) >>> 0;
  return CHIP_HUES[h % CHIP_HUES.length];
}

// ── SubagentChip ──────────────────────────────────────────────────────────

interface ChipProps {
  session: SessionInfo;
  index: number;
  onSelect: (s: SessionInfo) => void;
}

function SubagentChip({ session, index, onSelect }: ChipProps) {
  const { t } = useTranslation();
  const active = isActive(session);
  const hue = chipHue(session.id);

  const chipStyle = { '--chip-hue': hue } as React.CSSProperties;

  return (
    <button
      className={`${styles.chip} ${active ? styles.chip_active : ""}`}
      style={chipStyle}
      onClick={(e) => { e.stopPropagation(); onSelect(session); }}
      title={session.agentDescription ?? session.agentType ?? t("subagent")}
    >
      <StatusIcon status={session.status} />
      <span className={styles.chip_index}>#{index}</span>
      <span className={styles.chip_type_icon}>
        <SubagentTypeIcon type={session.agentType} />
      </span>
      {session.agentDescription && (
        <span className={styles.chip_desc}>{session.agentDescription}</span>
      )}
      {session.model && (
        <span className={styles.chip_model}>{formatModel(session.model)}</span>
      )}
      {session.thinkingLevel && session.thinkingLevel !== "medium" && (
        <span className={styles.chip_thinking}>{session.thinkingLevel}</span>
      )}
      {session.tokenSpeed >= 0.5 && (
        <span className={styles.chip_speed}>
          {session.tokenSpeed.toFixed(1)}{t("tok_s")}
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

  const totalTokens = [main, ...subagents].reduce((sum, s) => sum + s.totalOutputTokens, 0);
  const totalSpeed = [main, ...subagents].reduce((sum, s) => sum + s.tokenSpeed, 0);
  const groupActive = isActive(main) || activeSubagents.length > 0;

  // Solo session (no subagents): same group structure as multi-agent, just no chips
  if (subagents.length === 0) {
    return (
      <div className={`${styles.group} ${isActive(main) ? styles.group_active : ""}`}>
        <div className={styles.group_header} onClick={() => onSelect(main)}>
          <span className={styles.group_name}>{main.workspaceName}</span>
          <StatusBadge status={main.status} />
          <div className={styles.group_stats}>
            <span className={styles.group_stat}>
              {main.totalOutputTokens.toLocaleString()} {t("tokens")}
            </span>
            {main.tokenSpeed >= 0.5 && (
              <>
                <span className={styles.group_divider}>·</span>
                <span className={`${styles.group_stat} ${styles.group_speed}`}>
                  {main.tokenSpeed.toFixed(1)} {t("tok_s")}
                </span>
              </>
            )}
          </div>
        </div>
        <div className={styles.group_body}>
          <SessionCard session={main} isSelected={false} onClick={() => onSelect(main)} variant="group-main" hideHeader />
        </div>
      </div>
    );
  }

  return (
    <div className={`${styles.group} ${groupActive ? styles.group_active : ""}`}>
      {/* Group header */}
      <div className={styles.group_header} onClick={() => onSelect(main)}>
        <span className={styles.group_name}>{main.workspaceName}</span>
        <StatusBadge status={main.status} />
        <div className={styles.group_stats}>
          <span className={styles.group_stat}>
            {subagents.length} {t("gallery.subs")}
          </span>
          <span className={styles.group_divider}>·</span>
          <span className={styles.group_stat}>
            {totalTokens.toLocaleString()} {t("tokens")}
          </span>
          {totalSpeed >= 0.5 && (
            <>
              <span className={styles.group_divider}>·</span>
              <span className={`${styles.group_stat} ${styles.group_speed}`}>
                {totalSpeed.toFixed(1)} {t("tok_s")}
              </span>
            </>
          )}
        </div>
      </div>

      {/* Main agent card */}
      <div className={styles.group_body}>
        <SessionCard
          session={main}
          isSelected={false}
          onClick={() => onSelect(main)}
          variant="group-main"
          hideHeader
        />
      </div>

      {/* Active subagent chips */}
      {activeSubagents.length > 0 && (
        <div className={styles.chips_row}>
          {activeSubagents.map((sub) => (
            <SubagentChip key={sub.jsonlPath} session={sub} index={subIndexMap.get(sub.id) ?? 0} onSelect={onSelect} />
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
            <div className={styles.chips_row}>
              {idleSubagents.map((sub) => (
                <SubagentChip key={sub.jsonlPath} session={sub} index={subIndexMap.get(sub.id) ?? 0} onSelect={onSelect} />
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
  // Build subagent map from ALL sessions so idle subs of active mains are included
  const subByParent = new Map<string, SessionInfo[]>();
  for (const s of allSessions) {
    if (s.isSubagent && s.parentSessionId) {
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
    <div className={styles.root}>
      <SessionToolbar
        filter={filter}
        onFilterChange={setFilter}
        activeCount={activeSessions.filter((s) => !s.isSubagent).length}
        showAll={showAll}
        onToggleShowAll={() => setShowAll((v) => !v)}
        ftsMatchCount={filter.trim().length >= 2 ? ftsMatchPaths.size : undefined}
        searching={searching}
      />

      {/* Grid */}
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
              <p className={styles.empty}>{scanReady ? t("no_sessions") : t("scanning")}</p>
            )}
          </>
        ) : (
          <>
            <div className={gridCls} onClick={handleGridClick}>
              {buildRows(filteredActiveMains, sessions, handleSelect)}
            </div>
            {filteredActiveMains.length === 0 && (
              <p className={styles.empty}>{scanReady ? t("no_sessions") : t("scanning")}</p>
            )}
          </>
        )}
      </div>

    </div>
  );
}
