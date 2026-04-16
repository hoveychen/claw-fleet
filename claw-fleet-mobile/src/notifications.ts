import * as Notifications from "expo-notifications";
import { Platform } from "react-native";

// ── Configuration ───────────────────────────────────────────────────────────

// Show notifications even when app is foregrounded
Notifications.setNotificationHandler({
  handleNotification: async () => ({
    shouldShowBanner: true,
    shouldShowList: true,
    shouldPlaySound: true,
    shouldSetBadge: true,
    priority: Notifications.AndroidNotificationPriority.HIGH,
  }),
});

// Android notification channel
if (Platform.OS === "android") {
  Notifications.setNotificationChannelAsync("decisions", {
    name: "Agent Decisions",
    importance: Notifications.AndroidImportance.HIGH,
    vibrationPattern: [0, 200, 100, 200],
    lightColor: "#6366f1",
  });
}

// ── Permission ──────────────────────────────────────────────────────────────

export async function requestNotificationPermissions(): Promise<boolean> {
  const { status: existing } = await Notifications.getPermissionsAsync();
  if (existing === "granted") return true;
  const { status } = await Notifications.requestPermissionsAsync();
  return status === "granted";
}

// ── Send local notifications ────────────────────────────────────────────────

export async function notifyGuardRequest(
  id: string,
  workspaceName: string,
  commandSummary: string,
) {
  await Notifications.scheduleNotificationAsync({
    content: {
      title: `⚠️ ${workspaceName}`,
      body: `Command blocked: ${commandSummary}`,
      data: { decisionId: id, kind: "guard" },
      ...(Platform.OS === "android" && { channelId: "decisions" }),
    },
    trigger: null, // immediate
  });
}

export async function notifyElicitationRequest(
  id: string,
  workspaceName: string,
  questionPreview: string,
) {
  await Notifications.scheduleNotificationAsync({
    content: {
      title: `💬 ${workspaceName}`,
      body: questionPreview,
      data: { decisionId: id, kind: "elicitation" },
      ...(Platform.OS === "android" && { channelId: "decisions" }),
    },
    trigger: null,
  });
}

export async function notifyWaitingAlert(
  sessionId: string,
  workspaceName: string,
  summary: string,
) {
  await Notifications.scheduleNotificationAsync({
    content: {
      title: `🔔 ${workspaceName}`,
      body: summary || "Agent is waiting for input",
      data: { sessionId, kind: "waiting-alert" },
      ...(Platform.OS === "android" && { channelId: "decisions" }),
    },
    trigger: null,
  });
}

// ── Notification response listener ──────────────────────────────────────────

interface NotificationTapData {
  kind: "guard" | "elicitation" | "waiting-alert";
  /** Set for guard / elicitation taps. */
  decisionId?: string;
  /** Set for waiting-alert taps. */
  sessionId?: string;
}

type NotificationTapCallback = (data: NotificationTapData) => void;

let _tapCallback: NotificationTapCallback | null = null;

/** Register a callback for when user taps a notification. */
export function onNotificationTap(cb: NotificationTapCallback) {
  _tapCallback = cb;
}

function dispatchTap(data: Record<string, unknown> | undefined) {
  if (!data || !_tapCallback) return;
  const kind = data.kind as string | undefined;
  if (kind === "guard" || kind === "elicitation") {
    _tapCallback({
      kind,
      decisionId: data.decisionId as string,
    });
  } else if (kind === "waiting-alert") {
    _tapCallback({
      kind: "waiting-alert",
      sessionId: data.sessionId as string,
    });
  }
}

// Listen for notification taps (user interaction)
Notifications.addNotificationResponseReceivedListener((response) => {
  dispatchTap(response.notification.request.content.data);
});

// Also handle the case where app was launched by tapping a notification
Notifications.getLastNotificationResponseAsync().then((response) => {
  if (response) {
    dispatchTap(response.notification.request.content.data);
  }
});
