import { useEffect, useMemo, useRef, useState } from "react";
import type { SessionInfo } from "../types";

export type HealthLevel = "green" | "yellow" | "red";

export interface FleetState {
  busyCount: number;
  subBusyCount: number;
  waitingCount: number;
  stuckCount: number;
  totalSpeed: number;
  recentlyCompletedIds: string[];
  healthLevel: HealthLevel;
  nextAttentionId: string | null;
}

const BUSY_STATUSES = new Set<string>([
  "thinking", "executing", "streaming", "processing", "active", "delegating",
]);

const COMPLETION_WINDOW_MS = 30_000;

export function useFleetState(sessions: SessionInfo[]): FleetState {
  const prevBusyIds = useRef<Set<string>>(new Set());
  const [completedAt, setCompletedAt] = useState<Record<string, number>>({});

  useEffect(() => {
    const now = Date.now();
    const currentBusy = new Set(
      sessions.filter((s) => BUSY_STATUSES.has(s.status)).map((s) => s.id),
    );
    const sessionIds = new Set(sessions.map((s) => s.id));

    setCompletedAt((prev) => {
      let changed = false;
      const next = { ...prev };

      prevBusyIds.current.forEach((id) => {
        if (!currentBusy.has(id) && sessionIds.has(id) && next[id] === undefined) {
          next[id] = now;
          changed = true;
        }
      });

      for (const id of Object.keys(next)) {
        if (now - next[id] > COMPLETION_WINDOW_MS || !sessionIds.has(id)) {
          delete next[id];
          changed = true;
        }
      }

      return changed ? next : prev;
    });

    prevBusyIds.current = currentBusy;
  }, [sessions]);

  useEffect(() => {
    const timer = setInterval(() => {
      const now = Date.now();
      setCompletedAt((prev) => {
        let changed = false;
        const next = { ...prev };
        for (const id of Object.keys(next)) {
          if (now - next[id] > COMPLETION_WINDOW_MS) {
            delete next[id];
            changed = true;
          }
        }
        return changed ? next : prev;
      });
    }, 5000);
    return () => clearInterval(timer);
  }, []);

  return useMemo(() => {
    const busySessions = sessions.filter((s) => BUSY_STATUSES.has(s.status));
    const waitingSessions = sessions.filter((s) => s.status === "waitingInput");
    const stuckSessions = sessions.filter((s) => {
      const tags = s.lastOutcome ?? [];
      return tags.includes("stuck") || tags.includes("overwhelmed");
    });
    const subBusyCount = busySessions.filter((s) => s.isSubagent).length;
    const totalSpeed = sessions.reduce((sum, s) => sum + s.tokenSpeed, 0);
    const recentlyCompletedIds = Object.keys(completedAt);
    const healthLevel: HealthLevel =
      stuckSessions.length > 0 ? "red" :
      waitingSessions.length > 0 ? "yellow" :
      "green";
    const nextAttentionId =
      waitingSessions[0]?.id ??
      stuckSessions[0]?.id ??
      recentlyCompletedIds[0] ??
      null;

    return {
      busyCount: busySessions.length,
      subBusyCount,
      waitingCount: waitingSessions.length,
      stuckCount: stuckSessions.length,
      totalSpeed,
      recentlyCompletedIds,
      healthLevel,
      nextAttentionId,
    };
  }, [sessions, completedAt]);
}
