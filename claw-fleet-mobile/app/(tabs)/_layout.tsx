import { useEffect, useCallback, useRef, useState } from "react";
import { AppState, View } from "react-native";
import { Tabs, router } from "expo-router";
import { useTheme } from "react-native-paper";
import { Ionicons } from "@expo/vector-icons";
import { useSafeAreaInsets } from "react-native-safe-area-context";
import { useConnectionStore } from "../../src/stores/connection";
import { useSessionsStore } from "../../src/stores/sessions";
import { useDecisionsStore } from "../../src/stores/decisions";
import { useSSE } from "../../src/api/sse";
import { useT } from "../../src/i18n";
import { DecisionPanel, DecisionBadge } from "../../src/components/DecisionPanel";
import {
  requestNotificationPermissions,
  notifyGuardRequest,
  notifyElicitationRequest,
  notifyWaitingAlert,
  onNotificationTap,
} from "../../src/notifications";
import type {
  SessionInfo,
  WaitingAlert,
  GuardRequest,
  ElicitationRequest,
} from "../../src/types";

export default function TabLayout() {
  const theme = useTheme();
  const insets = useSafeAreaInsets();
  const t = useT();
  const client = useConnectionStore((s) => s.client);
  const setSessions = useSessionsStore((s) => s.setSessions);
  const addAlert = useSessionsStore((s) => s.addAlert);
  const addGuard = useDecisionsStore((s) => s.addGuard);
  const addElicitation = useDecisionsStore((s) => s.addElicitation);
  const setFocusedId = useDecisionsStore((s) => s.setFocusedId);
  const pendingCount = useDecisionsStore((s) => s.pendingCount);

  const [panelVisible, setPanelVisible] = useState(false);
  const panelVisibleRef = useRef(panelVisible);
  panelVisibleRef.current = panelVisible;

  // Request notification permissions once
  useEffect(() => {
    requestNotificationPermissions();
  }, []);

  // Handle notification tap → open panel or navigate to session
  useEffect(() => {
    onNotificationTap((data) => {
      if (data.kind === "waiting-alert" && data.sessionId) {
        // Find session to get jsonlPath, then navigate to detail screen
        const session = useSessionsStore
          .getState()
          .sessions.find((s) => s.id === data.sessionId);
        if (session) {
          router.push({
            pathname: "/agent/[id]",
            params: { id: session.id, jsonlPath: session.jsonlPath },
          });
        }
      } else if (data.decisionId) {
        // Open decision panel focused on this decision
        setFocusedId(data.decisionId);
        setPanelVisible(true);
      }
    });
  }, []);

  // Initial data fetch
  useEffect(() => {
    if (!client) return;
    client.listSessions().then(setSessions).catch(() => {});
    client
      .getWaitingAlerts()
      .then((alerts) => useSessionsStore.getState().setAlerts(alerts))
      .catch(() => {});
    // Fetch initial pending decisions (no notification for these — they're not "new")
    client
      .getGuardPending()
      .then((reqs) => reqs.forEach(addGuard))
      .catch(() => {});
    client
      .getElicitationPending()
      .then((reqs) => reqs.forEach(addElicitation))
      .catch(() => {});
  }, [client]);

  // ── SSE handlers (with notifications) ────────────────────────────────────

  const onSessionsUpdated = useCallback(
    (sessions: SessionInfo[]) => setSessions(sessions),
    [setSessions],
  );
  const onWaitingAlert = useCallback(
    (alert: WaitingAlert) => {
      const existing = useSessionsStore.getState().alerts;
      const isNew = !existing.some((a) => a.sessionId === alert.sessionId);
      addAlert(alert);
      if (isNew) {
        notifyWaitingAlert(
          alert.sessionId,
          alert.workspaceName,
          alert.summary,
        );
      }
    },
    [addAlert],
  );
  const onGuardRequest = useCallback(
    (req: GuardRequest) => {
      // addGuard deduplicates, so check if it's truly new
      const existing = useDecisionsStore.getState().decisions;
      const isNew = !existing.some((d) => d.id === req.id);
      addGuard(req);
      if (isNew) {
        notifyGuardRequest(req.id, req.workspaceName, req.commandSummary);
      }
    },
    [addGuard],
  );
  const onElicitationRequest = useCallback(
    (req: ElicitationRequest) => {
      const existing = useDecisionsStore.getState().decisions;
      const isNew = !existing.some((d) => d.id === req.id);
      addElicitation(req);
      if (isNew) {
        const preview =
          req.questions[0]?.header || req.questions[0]?.question || "Question";
        notifyElicitationRequest(req.id, req.workspaceName, preview);
      }
    },
    [addElicitation],
  );

  const sseUrl = client?.sseUrl() ?? null;
  useSSE(sseUrl, {
    onSessionsUpdated,
    onWaitingAlert,
    onGuardRequest,
    onElicitationRequest,
  });

  // Fallback polling
  useEffect(() => {
    if (!client) return;
    const interval = setInterval(() => {
      client.listSessions().then(setSessions).catch(() => {});
      client
        .getGuardPending()
        .then((reqs) =>
          reqs.forEach((req) => {
            const existing = useDecisionsStore.getState().decisions;
            const isNew = !existing.some((d) => d.id === req.id);
            addGuard(req);
            if (isNew) {
              notifyGuardRequest(req.id, req.workspaceName, req.commandSummary);
            }
          }),
        )
        .catch(() => {});
      client
        .getElicitationPending()
        .then((reqs) =>
          reqs.forEach((req) => {
            const existing = useDecisionsStore.getState().decisions;
            const isNew = !existing.some((d) => d.id === req.id);
            addElicitation(req);
            if (isNew) {
              const preview =
                req.questions[0]?.header ||
                req.questions[0]?.question ||
                "Question";
              notifyElicitationRequest(
                req.id,
                req.workspaceName,
                preview,
              );
            }
          }),
        )
        .catch(() => {});
    }, 10000);
    return () => clearInterval(interval);
  }, [client, setSessions]);

  // Auto-open panel when new decisions arrive (only if app is in foreground)
  useEffect(() => {
    if (pendingCount > 0 && !panelVisibleRef.current) {
      if (AppState.currentState === "active") {
        setPanelVisible(true);
      }
    }
  }, [pendingCount]);

  const activeCount = useSessionsStore(
    (s) => s.sessions.filter((sess) => sess.status !== "idle").length,
  );

  return (
    <View style={{ flex: 1 }}>
      <Tabs
        screenOptions={{
          tabBarActiveTintColor: theme.colors.primary,
          tabBarInactiveTintColor: theme.colors.onSurfaceVariant,
          tabBarStyle: {
            backgroundColor: theme.colors.surface,
            borderTopColor: theme.colors.outline,
            height: 52 + insets.bottom,
            paddingBottom: insets.bottom,
          },
          tabBarLabelStyle: {
            fontSize: 11,
            fontWeight: "600",
            marginTop: -2,
          },
          tabBarIconStyle: {
            marginBottom: -2,
          },
          headerStyle: {
            backgroundColor: theme.colors.surface,
          },
          headerTintColor: theme.colors.onSurface,
          headerShadowVisible: false,
          headerTitleStyle: {
            fontWeight: "700",
            fontSize: 18,
          },
        }}
      >
        <Tabs.Screen
          name="agents"
          options={{
            title: t("tabs.agents"),
            tabBarBadge: activeCount > 0 ? activeCount : undefined,
            tabBarBadgeStyle: { backgroundColor: "#22c55e", fontSize: 10 },
            tabBarIcon: ({ color, size }) => (
              <Ionicons name="terminal" size={size} color={color} />
            ),
          }}
        />
        <Tabs.Screen
          name="report"
          options={{
            title: t("tabs.report"),
            tabBarIcon: ({ color, size }) => (
              <Ionicons name="stats-chart" size={size} color={color} />
            ),
          }}
        />
        <Tabs.Screen
          name="audit"
          options={{
            title: t("tabs.audit"),
            tabBarIcon: ({ color, size }) => (
              <Ionicons name="shield-checkmark" size={size} color={color} />
            ),
          }}
        />
        <Tabs.Screen
          name="settings"
          options={{
            title: t("tabs.settings"),
            tabBarIcon: ({ color, size }) => (
              <Ionicons name="settings-sharp" size={size} color={color} />
            ),
          }}
        />
      </Tabs>

      {/* Floating decision badge */}
      <DecisionBadge onPress={() => setPanelVisible(true)} />

      {/* Decision panel modal */}
      <DecisionPanel
        visible={panelVisible}
        onDismiss={() => setPanelVisible(false)}
      />
    </View>
  );
}
