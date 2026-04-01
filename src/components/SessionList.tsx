import { listen } from "@tauri-apps/api/event";
import { invoke } from "@tauri-apps/api/core";
import { useCallback, useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { useAuditStore, useConnectionStore, useDetailStore, useSessionsStore, useUIStore } from "../store";
import type { SessionInfo } from "../types";
import { GalleryView } from "./GalleryView";
import { MascotEyes } from "./MascotEyes";
import { MemoryPanel } from "./MemoryPanel";
import { AuditView } from "./AuditView";
import { ReportView } from "./report/ReportView";
import { SkillsPanel } from "./SkillsPanel";
import { SessionCard } from "./SessionCard";
import { SessionToolbar } from "./SessionToolbar";
import { SettingsPanel } from "./SettingsPanel";
import styles from "./SessionList.module.css";
import { TokenSpeedChart } from "./TokenSpeedChart";
import { UsagePanel } from "./UsagePanel";
import { useSessionSearch } from "../hooks/useSessionSearch";
import { getItem, setItem } from "../storage";

const MIN_WIDTH = 200;
const MAX_WIDTH = 520;
const DEFAULT_WIDTH = 280;

function getSavedWidth(): number {
  const saved = getItem("sidebar-width");
  if (saved) {
    const n = parseInt(saved, 10);
    if (n >= MIN_WIDTH && n <= MAX_WIDTH) return n;
  }
  return DEFAULT_WIDTH;
}

export function SessionList() {
  const { t } = useTranslation();
  const { sessions, refresh, setSessions, scanReady, setScanReady } = useSessionsStore();
  const { session: viewedSession, open } = useDetailStore();
  const { viewMode, setViewMode } = useUIStore();
  const { connection } = useConnectionStore();
  const unreadCriticalCount = useAuditStore((s) => s.unreadCriticalCount);
  const [filter, setFilter] = useState("");
  const [showAll, setShowAll] = useState(false);
  const [sidebarWidth, setSidebarWidth] = useState(getSavedWidth);
  const [isDragging, setIsDragging] = useState(false);
  const [isWindows, setIsWindows] = useState(false);
  const [showSettings, setShowSettings] = useState(false);
  const dragRef = useRef<{ startX: number; startWidth: number } | null>(null);

  useEffect(() => {
    refresh();
    invoke<string>("get_platform").then((p) => setIsWindows(p === "windows"));
    // Load audit data for the unread critical badge
    invoke<import("../types").AuditSummary>("get_audit_events")
      .then((data) => {
        useAuditStore.getState().setCriticalEvents(
          data.events.filter((e) => e.riskLevel === "critical")
        );
      })
      .catch(() => {});
    const unlistenPromise = listen<SessionInfo[]>("sessions-updated", (e) => {
      setSessions(e.payload);
    });
    const unlistenScanReady = listen<boolean>("scan-ready", () => {
      setScanReady(true);
    });
    return () => {
      unlistenPromise.then((u) => u());
      unlistenScanReady.then((u) => u());
    };
  }, []);

  const handleResizeMouseDown = useCallback((e: React.MouseEvent) => {
    e.preventDefault();
    dragRef.current = { startX: e.clientX, startWidth: sidebarWidth };
    setIsDragging(true);

    function onMouseMove(e: MouseEvent) {
      if (!dragRef.current) return;
      const delta = e.clientX - dragRef.current.startX;
      const newWidth = Math.min(MAX_WIDTH, Math.max(MIN_WIDTH, dragRef.current.startWidth + delta));
      setSidebarWidth(newWidth);
    }

    function onMouseUp() {
      dragRef.current = null;
      setIsDragging(false);
      setSidebarWidth((prev) => {
        setItem("sidebar-width", String(prev));
        return prev;
      });
      document.removeEventListener("mousemove", onMouseMove);
      document.removeEventListener("mouseup", onMouseUp);
    }

    document.addEventListener("mousemove", onMouseMove);
    document.addEventListener("mouseup", onMouseUp);
  }, [sidebarWidth]);

  const { searching, ftsMatchPaths } = useSessionSearch(filter);

  const filtered = sessions.filter((s) => {
    if (!filter) return true;
    const q = filter.toLowerCase();
    const clientMatch =
      s.workspaceName.toLowerCase().includes(q) ||
      s.slug?.toLowerCase().includes(q) ||
      s.agentDescription?.toLowerCase().includes(q) ||
      s.ideName?.toLowerCase().includes(q);
    return clientMatch || ftsMatchPaths.has(s.jsonlPath);
  });

  // Promote idle main sessions that have active subagents → delegating
  const activeSubagentParentIds = new Set(
    filtered
      .filter(
        (s) =>
          s.isSubagent &&
          s.parentSessionId &&
          ["thinking", "executing", "streaming", "processing", "waitingInput", "active"].includes(s.status)
      )
      .map((s) => s.parentSessionId!)
  );
  const promoted = filtered.map((s) =>
    !s.isSubagent &&
    ["idle", "active", "waitingInput", "processing"].includes(s.status) &&
    activeSubagentParentIds.has(s.id)
      ? { ...s, status: "delegating" as const }
      : s
  );

  const active = promoted.filter((s) =>
    ["thinking", "executing", "streaming", "processing", "waitingInput", "active", "delegating"].includes(s.status)
  );
  const idle = promoted
    .filter((s) => s.status === "idle")
    .sort((a, b) => b.lastActivityMs - a.lastActivityMs);

  function buildTree(list: SessionInfo[]) {
    const mains = list.filter((s) => !s.isSubagent);
    const subagentsByParent = new Map<string, SessionInfo[]>();
    for (const s of list) {
      if (s.isSubagent && s.parentSessionId) {
        const arr = subagentsByParent.get(s.parentSessionId) ?? [];
        arr.push(s);
        subagentsByParent.set(s.parentSessionId, arr);
      }
    }
    const orphans = list.filter(
      (s) =>
        s.isSubagent &&
        (!s.parentSessionId ||
          !subagentsByParent.has(s.parentSessionId) ||
          !mains.find((m) => m.id === s.parentSessionId))
    );
    const result: { session: SessionInfo; indented: boolean }[] = [];
    for (const main of mains) {
      result.push({ session: main, indented: false });
      for (const sub of subagentsByParent.get(main.id) ?? []) {
        result.push({ session: sub, indented: true });
      }
    }
    for (const orphan of orphans) {
      result.push({ session: orphan, indented: false });
    }
    return result;
  }

  function renderGroup(list: SessionInfo[]) {
    return buildTree(list).map(({ session: s, indented }) => (
      <div key={s.jsonlPath} className={indented ? styles.indented : undefined}>
        <SessionCard
          session={s}
          isSelected={viewedSession?.jsonlPath === s.jsonlPath}
          onClick={() => open(s)}
        />
      </div>
    ));
  }

  const isRemote = connection?.type === "remote";

  return (
    <>
      <aside className={styles.sidebar} style={{ width: sidebarWidth }}>
        {/* Header — hidden on Windows (title bar already shows product name) */}
        {!isWindows && (
          <div className={styles.header} data-tauri-drag-region>
            <h1 className={styles.title} data-tauri-drag-region>{t("title")}</h1>
          </div>
        )}

        {/* Navigation */}
        <nav className={styles.nav} data-wizard="view-toggle">
          <button
            className={`${styles.nav_item} ${viewMode === "list" ? styles.nav_active : ""}`}
            onClick={() => setViewMode("list")}
          >
            <span className={styles.nav_icon}>☰</span>
            <span className={styles.nav_label}>{t("view_list")}</span>
          </button>
          <button
            className={`${styles.nav_item} ${viewMode === "gallery" ? styles.nav_active : ""}`}
            onClick={() => setViewMode("gallery")}
          >
            <span className={styles.nav_icon}>⊞</span>
            <span className={styles.nav_label}>{t("view_gallery")}</span>
          </button>
          <button
            className={`${styles.nav_item} ${viewMode === "audit" ? styles.nav_active : ""}`}
            onClick={() => setViewMode("audit")}
          >
            <span className={styles.nav_icon}>⛨</span>
            <span className={styles.nav_label}>{t("view_audit")}</span>
            {unreadCriticalCount > 0 && (
              <span className={styles.nav_badge}>{unreadCriticalCount}</span>
            )}
          </button>
          <button
            className={`${styles.nav_item} ${viewMode === "report" ? styles.nav_active : ""}`}
            onClick={() => setViewMode("report")}
          >
            <span className={styles.nav_icon}><svg width="15" height="15" viewBox="0 0 16 16" fill="none" stroke="currentColor" strokeWidth="1.3"><rect x="1.5" y="2.5" width="13" height="12" rx="1.5"/><line x1="1.5" y1="5.5" x2="14.5" y2="5.5"/><line x1="5" y1="1" x2="5" y2="4"/><line x1="11" y1="1" x2="11" y2="4"/></svg></span>
            <span className={styles.nav_label}>{t("view_report")}</span>
          </button>
        </nav>

        <div className={styles.separator} />

        {/* Scrollable sidebar content */}
        <div className={styles.sidebar_content}>
          {/* Token Speed Chart */}
          <div data-wizard="token-speed">
            <TokenSpeedChart />
          </div>

          {/* Usage panel */}
          <UsagePanel />

          {/* Memory panel */}
          <MemoryPanel />

          {/* Skills panel */}
          <SkillsPanel />

          {/* Mascot */}
          <MascotEyes />
        </div>

        {/* Footer profile card */}
        <div className={styles.footer} data-wizard="settings-footer">
          <button
            className={styles.footer_card}
            onClick={() => setShowSettings(true)}
            title={t("settings.title")}
          >
            <div className={styles.footer_avatar}>
              <img src="/app-icon.png" alt="" style={{ width: "100%", height: "100%", objectFit: "contain" }} />
            </div>
            <div className={styles.footer_info}>
              <span className={styles.footer_name}>{t("title")}</span>
              <span className={styles.footer_status}>
                <span className={`${styles.footer_dot} ${isRemote ? styles.footer_dot_remote : ""}`} />
                {isRemote ? t("settings.remote") : t("settings.local")}
              </span>
            </div>
            <span className={styles.footer_gear}>⚙</span>
          </button>
        </div>

        {/* Resize handle */}
        <div
          className={`${styles.resize_handle} ${isDragging ? styles.resize_handle_active : ""}`}
          onMouseDown={handleResizeMouseDown}
        />
      </aside>

      {/* Settings panel */}
      {showSettings && <SettingsPanel onClose={() => setShowSettings(false)} />}

      {/* Main content area */}
      {viewMode === "list" ? (
        <div className={styles.main_area}>
          <SessionToolbar
            filter={filter}
            onFilterChange={setFilter}
            activeCount={active.length}
            totalCount={sessions.length}
            showAll={showAll}
            onToggleShowAll={() => setShowAll((v) => !v)}
            ftsMatchCount={filter.trim().length >= 2 ? ftsMatchPaths.size : undefined}
            searching={searching}
          />

          <div className={styles.list}>
            {showAll ? (
              <>
                {active.length > 0 && (
                  <section>
                    <div className={styles.group_label}>{t("active")}</div>
                    {renderGroup(active)}
                  </section>
                )}
                {idle.length > 0 && (
                  <section>
                    <div className={styles.group_label}>{t("recent")}</div>
                    {renderGroup(idle)}
                  </section>
                )}
                {promoted.length === 0 && (
                  <p className={styles.empty}>{scanReady ? t("no_sessions") : t("scanning")}</p>
                )}
              </>
            ) : (
              <>
                {active.length > 0 && renderGroup(active)}
                {active.length === 0 && (
                  <p className={styles.empty}>{scanReady ? t("no_sessions") : t("scanning")}</p>
                )}
              </>
            )}
          </div>
        </div>
      ) : viewMode === "gallery" ? (
        <GalleryView />
      ) : viewMode === "audit" ? (
        <AuditView />
      ) : (
        <ReportView />
      )}
    </>
  );
}
