import { useState } from "react";
import { useTranslation } from "react-i18next";
import { useSessionsStore } from "../store";
import type { SessionInfo, SessionStatus } from "../types";
import { InspectModal } from "./InspectModal";
import { SessionCard } from "./SessionCard";
import styles from "./GalleryView.module.css";

// ── Helpers ────────────────────────────────────────────────────────────────

const ACTIVE_STATUSES: SessionStatus[] = [
  "streaming", "processing", "waitingInput", "active", "delegating",
];

function isActive(s: SessionInfo) {
  return ACTIVE_STATUSES.includes(s.status);
}

// ── GalleryRow: one main agent with nested subagents ──────────────────────

interface RowProps {
  main: SessionInfo;
  subagents: SessionInfo[];
  onSelect: (s: SessionInfo) => void;
}

function GalleryRow({ main, subagents, onSelect }: RowProps) {
  return (
    <div className={`${styles.row} ${isActive(main) ? styles.row_active : ""}`}>
      {/* Main card */}
      <div className={styles.main_card} onClick={() => onSelect(main)}>
        <SessionCard session={main} isSelected={false} onClick={() => onSelect(main)} />
      </div>

      {/* Subagent cards */}
      {subagents.length > 0 && (
        <div className={styles.subagents}>
          {subagents.map((sub) => (
            <div key={sub.jsonlPath} className={styles.sub_card} onClick={() => onSelect(sub)}>
              <SessionCard session={sub} isSelected={false} onClick={() => onSelect(sub)} />
            </div>
          ))}
        </div>
      )}
    </div>
  );
}

// ── GalleryView ───────────────────────────────────────────────────────────

export function GalleryView() {
  const { t } = useTranslation();
  const sessions = useSessionsStore((s) => s.sessions);
  const [inspecting, setInspecting] = useState<SessionInfo | null>(null);
  const [filter, setFilter] = useState("");

  // Promote idle main sessions that have active subagents → delegating
  const activeSubagentParentIds = new Set(
    sessions
      .filter(
        (s) =>
          s.isSubagent &&
          s.parentSessionId &&
          ACTIVE_STATUSES.includes(s.status)
      )
      .map((s) => s.parentSessionId!)
  );
  const promoted = sessions.map((s) =>
    !s.isSubagent && s.status === "idle" && activeSubagentParentIds.has(s.id)
      ? { ...s, status: "delegating" as const }
      : s
  );

  // Gallery only shows active sessions
  const activeSessions = promoted.filter(isActive);

  // Filter
  const filtered = filter
    ? activeSessions.filter((s) => {
        const q = filter.toLowerCase();
        return (
          s.workspaceName.toLowerCase().includes(q) ||
          s.slug?.toLowerCase().includes(q) ||
          s.agentDescription?.toLowerCase().includes(q) ||
          s.ideName?.toLowerCase().includes(q)
        );
      })
    : activeSessions;

  // Group: main sessions + their subagents
  const mains = filtered.filter((s) => !s.isSubagent);
  const subByParent = new Map<string, SessionInfo[]>();
  for (const s of filtered) {
    if (s.isSubagent && s.parentSessionId) {
      const arr = subByParent.get(s.parentSessionId) ?? [];
      arr.push(s);
      subByParent.set(s.parentSessionId, arr);
    }
  }
  // Orphan subagents (their parent isn't in filtered)
  const orphans = filtered.filter(
    (s) =>
      s.isSubagent &&
      (!s.parentSessionId || !mains.find((m) => m.id === s.parentSessionId))
  );

  // Sort: active first
  const sortedMains = [
    ...mains.filter(isActive),
    ...mains.filter((s) => !isActive(s)),
  ];

  return (
    <div className={styles.root}>
      {/* Toolbar */}
      <div className={styles.toolbar}>
        <input
          className={styles.search}
          type="text"
          placeholder={t("filter_placeholder")}
          value={filter}
          onChange={(e) => setFilter(e.target.value)}
        />
        <span className={styles.count}>
          {activeSessions.length} {t("active")}
        </span>
      </div>

      {/* Grid */}
      <div className={styles.grid}>
        {sortedMains.map((main) => (
          <GalleryRow
            key={main.jsonlPath}
            main={main}
            subagents={subByParent.get(main.id) ?? []}
            onSelect={setInspecting}
          />
        ))}

        {orphans.map((s) => (
          <div
            key={s.jsonlPath}
            className={styles.orphan_card}
            onClick={() => setInspecting(s)}
          >
            <SessionCard session={s} isSelected={false} onClick={() => setInspecting(s)} />
          </div>
        ))}

        {filtered.length === 0 && (
          <p className={styles.empty}>{t("no_sessions")}</p>
        )}
      </div>

      {/* Inspect modal */}
      {inspecting && (
        <InspectModal session={inspecting} onClose={() => setInspecting(null)} />
      )}
    </div>
  );
}
