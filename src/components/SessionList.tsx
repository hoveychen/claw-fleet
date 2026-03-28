import { listen } from "@tauri-apps/api/event";
import { invoke } from "@tauri-apps/api/core";
import { useCallback, useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { useConnectionStore, useDetailStore, useSessionsStore, useUIStore } from "../store";
import type { SessionInfo } from "../types";
import { GalleryView } from "./GalleryView";
import { MascotEyes } from "./MascotEyes";
import { MemoryPanel } from "./MemoryPanel";
import { SkillsPanel } from "./SkillsPanel";
import { SessionCard } from "./SessionCard";
import { SettingsPanel } from "./SettingsPanel";
import styles from "./SessionList.module.css";
import { TokenSpeedChart } from "./TokenSpeedChart";
import { UsagePanel } from "./UsagePanel";
import { AlertBadge } from "./WaitingAlerts";
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
  const { sessions, refresh, setSessions } = useSessionsStore();
  const { session: viewedSession, open } = useDetailStore();
  const { viewMode, setViewMode } = useUIStore();
  const { connection } = useConnectionStore();
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
    const unlistenPromise = listen<SessionInfo[]>("sessions-updated", (e) => {
      setSessions(e.payload);
    });
    return () => {
      unlistenPromise.then((u) => u());
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

  const filtered = sessions.filter((s) => {
    if (!filter) return true;
    const q = filter.toLowerCase();
    return (
      s.workspaceName.toLowerCase().includes(q) ||
      s.slug?.toLowerCase().includes(q) ||
      s.agentDescription?.toLowerCase().includes(q) ||
      s.ideName?.toLowerCase().includes(q)
    );
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

  // Count active agents for header
  const activeCount = sessions.filter((s) =>
    ["thinking", "executing", "streaming", "processing", "waitingInput", "active", "delegating"].includes(s.status)
  ).length;

  const isRemote = connection?.type === "remote";

  return (
    <>
      <aside className={styles.sidebar} style={{ width: sidebarWidth }}>
        {/* Header — hidden on Windows (title bar already shows product name) */}
        {!isWindows && (
          <div className={styles.header}>
            <img src="/app-icon.png" className={styles.app_icon} alt="icon" />
            <h1 className={styles.title}>{t("title")}</h1>
            <span className={styles.count} title={`${activeCount} active`}>
              {sessions.length}
            </span>
            <AlertBadge />
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
          <div className={styles.toolbar}>
            <input
              className={styles.search}
              type="text"
              placeholder={t("filter_placeholder")}
              value={filter}
              onChange={(e) => setFilter(e.target.value)}
            />
            <span className={styles.active_count}>
              {active.length} {t("active")}
            </span>
            <button
              className={`${styles.toggle_btn} ${showAll ? styles.toggle_btn_active : ""}`}
              onClick={() => setShowAll((v) => !v)}
              title={showAll ? t("gallery_show_active") : t("gallery_show_all")}
            >
              {showAll ? t("gallery_show_active") : t("gallery_show_all")}
            </button>
          </div>

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
                  <p className={styles.empty}>{t("no_sessions")}</p>
                )}
              </>
            ) : (
              <>
                {active.length > 0 && renderGroup(active)}
                {active.length === 0 && (
                  <p className={styles.empty}>{t("no_sessions")}</p>
                )}
              </>
            )}
          </div>
        </div>
      ) : (
        <GalleryView />
      )}
    </>
  );
}
