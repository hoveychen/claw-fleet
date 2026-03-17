import { listen } from "@tauri-apps/api/event";
import { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { useDetailStore, useSessionsStore, useUIStore } from "../store";
import type { SessionInfo } from "../types";
import { AccountInfo } from "./AccountInfo";
import { GalleryView } from "./GalleryView";
import { LanguageSwitcher } from "./LanguageSwitcher";
import { SessionCard } from "./SessionCard";
import styles from "./SessionList.module.css";
import { ThemeToggle } from "./ThemeToggle";
import { TokenSpeedChart } from "./TokenSpeedChart";

export function SessionList() {
  const { t } = useTranslation();
  const { sessions, refresh, setSessions } = useSessionsStore();
  const { session: viewedSession, open } = useDetailStore();
  const { viewMode, setViewMode } = useUIStore();
  const [filter, setFilter] = useState("");

  useEffect(() => {
    refresh();
    const unlistenPromise = listen<SessionInfo[]>("sessions-updated", (e) => {
      setSessions(e.payload);
    });
    return () => {
      unlistenPromise.then((u) => u());
    };
  }, []);

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
          ["streaming", "processing", "waitingInput", "active"].includes(s.status)
      )
      .map((s) => s.parentSessionId!)
  );
  const promoted = filtered.map((s) =>
    !s.isSubagent && s.status === "idle" && activeSubagentParentIds.has(s.id)
      ? { ...s, status: "delegating" as const }
      : s
  );

  const active = promoted.filter((s) =>
    ["streaming", "processing", "waitingInput", "active", "delegating"].includes(s.status)
  );
  const idle = promoted.filter((s) => s.status === "idle");

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
    ["streaming", "processing", "waitingInput", "active"].includes(s.status)
  ).length;

  return (
    <>
      <aside className={styles.sidebar}>
        {/* Header */}
        <div className={styles.header}>
          <h1 className={styles.title}>{t("title")}</h1>
          <span className={styles.count} title={`${activeCount} active`}>
            {sessions.length}
          </span>
        </div>

        {/* Controls row */}
        <div className={styles.controls}>
          <ThemeToggle />
          <LanguageSwitcher />
          <div className={styles.view_toggle}>
            <button
              className={`${styles.view_btn} ${viewMode === "list" ? styles.view_active : ""}`}
              onClick={() => setViewMode("list")}
              title={t("view_list")}
            >
              ☰
            </button>
            <button
              className={`${styles.view_btn} ${viewMode === "gallery" ? styles.view_active : ""}`}
              onClick={() => setViewMode("gallery")}
              title={t("view_gallery")}
            >
              ⊞
            </button>
          </div>
        </div>

        {/* Token Speed Chart */}
        <TokenSpeedChart />

        {/* Search + Session list — hidden in gallery mode */}
        {viewMode === "list" && (
          <>
            <input
              className={styles.search}
              type="text"
              placeholder={t("filter_placeholder")}
              value={filter}
              onChange={(e) => setFilter(e.target.value)}
            />

            <div className={styles.list}>
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

              {filtered.length === 0 && (
                <p className={styles.empty}>{t("no_sessions")}</p>
              )}
            </div>
          </>
        )}

        <AccountInfo />
      </aside>

      {/* Gallery view rendered as sibling of sidebar, fills main area */}
      {viewMode === "gallery" && <GalleryView />}
    </>
  );
}
