import { useEffect, useRef, useState } from "react";
import {
  Alert,
  StyleSheet,
  Text,
  TextInput,
  TouchableOpacity,
  View,
  useColorScheme,
  ActivityIndicator,
} from "react-native";
import { CameraView, useCameraPermissions } from "expo-camera";
import { router } from "expo-router";
import * as Linking from "expo-linking";
import { useConnectionStore } from "../src/stores/connection";
import { useT } from "../src/i18n";

/** Parse a QR URL like https://xxx.trycloudflare.com/mobile?token=TOKEN
 *  without relying on the URL constructor (broken on some RN/Hermes builds). */
function parseQrUrl(data: string): { url: string; token: string } | null {
  try {
    if (data.startsWith("http")) {
      // Extract origin: everything before /mobile or the first path
      const protoEnd = data.indexOf("://") + 3;
      const pathStart = data.indexOf("/", protoEnd);
      const origin = pathStart === -1 ? data : data.slice(0, pathStart);

      // Extract token from query string
      const qIdx = data.indexOf("?");
      if (qIdx === -1) return null;
      const params = data.slice(qIdx + 1).split("&");
      let token = "";
      for (const p of params) {
        const [k, v] = p.split("=");
        if (k === "token" && v) {
          token = decodeURIComponent(v);
        }
      }
      if (!token) return null;
      return { url: origin, token };
    }

    // Legacy JSON fallback
    const json = JSON.parse(data);
    if (json.url && json.token) return { url: json.url, token: json.token };
    return null;
  } catch {
    return null;
  }
}

export default function ConnectScreen() {
  const colorScheme = useColorScheme();
  const isDark = colorScheme === "dark";
  const { connected, connect } = useConnectionStore();
  const t = useT();
  const [permission, requestPermission] = useCameraPermissions();
  const [scanning, setScanning] = useState(false);
  const [manualUrl, setManualUrl] = useState("");
  const [manualToken, setManualToken] = useState("");
  const [connecting, setConnecting] = useState(false);
  // Guard against multiple rapid scans
  const scanProcessed = useRef(false);

  // If already connected, redirect to tabs
  useEffect(() => {
    if (connected) {
      router.replace("/(tabs)/agents");
    }
  }, [connected]);

  // Handle deep link: claw-fleet://connect?token=TOKEN&url=ENCODED_URL
  useEffect(() => {
    const handleDeepLink = (event: { url: string }) => {
      try {
        const parsed = Linking.parse(event.url);
        if (
          parsed.hostname === "connect" &&
          parsed.queryParams?.token &&
          parsed.queryParams?.url
        ) {
          const url = decodeURIComponent(parsed.queryParams.url as string);
          const token = parsed.queryParams.token as string;
          setConnecting(true);
          connect(url, token)
            .then(() => router.replace("/(tabs)/agents"))
            .catch((e) => Alert.alert(t("connect.connection_failed"), String(e)))
            .finally(() => setConnecting(false));
        }
      } catch {
        // ignore malformed deep links
      }
    };

    Linking.getInitialURL().then((url) => {
      if (url) handleDeepLink({ url });
    });

    const sub = Linking.addEventListener("url", handleDeepLink);
    return () => sub.remove();
  }, [connect]);

  const handleQrScanned = async (data: string) => {
    // Prevent multiple concurrent scans
    if (scanProcessed.current || connecting) return;
    scanProcessed.current = true;

    setScanning(false);
    setConnecting(true);

    try {
      const parsed = parseQrUrl(data);
      if (!parsed) {
        throw new Error(t("connect.invalid_qr"));
      }
      await connect(parsed.url, parsed.token);
      router.replace("/(tabs)/agents");
    } catch (e: unknown) {
      scanProcessed.current = false;
      Alert.alert(
        t("connect.connection_failed"),
        e instanceof Error ? e.message : t("connect.connect_failed_desktop"),
      );
    }
    setConnecting(false);
  };

  const handleManualConnect = async () => {
    if (!manualUrl.trim() || !manualToken.trim()) {
      Alert.alert(t("connect.error"), t("connect.enter_both"));
      return;
    }
    setConnecting(true);
    try {
      await connect(manualUrl.trim(), manualToken.trim());
      router.replace("/(tabs)/agents");
    } catch (e: unknown) {
      Alert.alert(
        t("connect.connection_failed"),
        e instanceof Error ? e.message : t("connect.connect_failed"),
      );
    }
    setConnecting(false);
  };

  const bg = isDark ? "#0f0f1a" : "#f5f5f5";
  const cardBg = isDark ? "#1a1a2e" : "#ffffff";
  const textColor = isDark ? "#e0e0e0" : "#1a1a2e";
  const dimColor = isDark ? "#666" : "#999";
  const accentColor = "#6366f1";

  if (scanning) {
    if (!permission?.granted) {
      return (
        <View style={[styles.container, { backgroundColor: bg }]}>
          <Text style={[styles.title, { color: textColor }]}>
            {t("connect.camera_needed")}
          </Text>
          <TouchableOpacity
            style={[styles.button, { backgroundColor: accentColor }]}
            onPress={requestPermission}
          >
            <Text style={styles.buttonText}>{t("connect.grant_permission")}</Text>
          </TouchableOpacity>
          <TouchableOpacity onPress={() => setScanning(false)}>
            <Text style={[styles.link, { color: accentColor }]}>{t("connect.cancel")}</Text>
          </TouchableOpacity>
        </View>
      );
    }

    return (
      <View style={[styles.container, { backgroundColor: "#000" }]}>
        <CameraView
          style={styles.camera}
          barcodeScannerSettings={{ barcodeTypes: ["qr"] }}
          onBarcodeScanned={(result) => handleQrScanned(result.data)}
        />
        <View style={styles.scanOverlay}>
          <View style={styles.scanFrame} />
          <Text style={styles.scanText}>
            {t("connect.scan_qr_hint")}
          </Text>
          <TouchableOpacity
            style={[styles.button, { backgroundColor: "rgba(0,0,0,0.6)", marginTop: 20 }]}
            onPress={() => {
              scanProcessed.current = false;
              setScanning(false);
            }}
          >
            <Text style={styles.buttonText}>{t("connect.cancel")}</Text>
          </TouchableOpacity>
        </View>
      </View>
    );
  }

  return (
    <View style={[styles.container, { backgroundColor: bg }]}>
      <Text style={[styles.logo, { color: textColor }]}>{t("connect.title")}</Text>
      <Text style={[styles.subtitle, { color: dimColor }]}>
        {t("connect.subtitle")}
      </Text>

      {connecting && (
        <ActivityIndicator
          size="large"
          color={accentColor}
          style={{ marginVertical: 20 }}
        />
      )}

      {!connecting && (
        <>
          <TouchableOpacity
            style={[styles.button, { backgroundColor: accentColor }]}
            onPress={() => {
              scanProcessed.current = false;
              setScanning(true);
            }}
          >
            <Text style={styles.buttonText}>{t("connect.scan_qr")}</Text>
          </TouchableOpacity>

          {__DEV__ && (
            <>
              <View style={styles.divider}>
                <View style={[styles.dividerLine, { backgroundColor: dimColor }]} />
                <Text style={[styles.dividerText, { color: dimColor }]}>{t("connect.or")}</Text>
                <View style={[styles.dividerLine, { backgroundColor: dimColor }]} />
              </View>

              <TouchableOpacity
                style={[styles.demoButton, { borderColor: dimColor }]}
                onPress={() => {
                  setConnecting(true);
                  connect("mock", "mock")
                    .then(() => router.replace("/(tabs)/agents"))
                    .catch(() => {})
                    .finally(() => setConnecting(false));
                }}
              >
                <Text style={[styles.demoText, { color: dimColor }]}>
                  {t("connect.demo_mode")}
                </Text>
              </TouchableOpacity>

              <View style={[styles.card, { backgroundColor: cardBg }]}>
                <Text style={[styles.cardTitle, { color: textColor }]}>
                  {t("connect.manual_connection")}
                </Text>
                <TextInput
                  style={[styles.input, { color: textColor, borderColor: dimColor }]}
                  placeholder={t("connect.url_placeholder")}
                  placeholderTextColor={dimColor}
                  value={manualUrl}
                  onChangeText={setManualUrl}
                  autoCapitalize="none"
                  autoCorrect={false}
                  keyboardType="url"
                />
                <TextInput
                  style={[styles.input, { color: textColor, borderColor: dimColor }]}
                  placeholder={t("connect.token_placeholder")}
                  placeholderTextColor={dimColor}
                  value={manualToken}
                  onChangeText={setManualToken}
                  autoCapitalize="none"
                  autoCorrect={false}
                  secureTextEntry
                />
                <TouchableOpacity
                  style={[styles.button, { backgroundColor: accentColor }]}
                  onPress={handleManualConnect}
                >
                  <Text style={styles.buttonText}>{t("connect.connect")}</Text>
                </TouchableOpacity>
              </View>
            </>
          )}
        </>
      )}
    </View>
  );
}

const styles = StyleSheet.create({
  container: {
    flex: 1,
    justifyContent: "center",
    alignItems: "center",
    padding: 24,
  },
  logo: {
    fontSize: 32,
    fontWeight: "bold",
    marginBottom: 8,
  },
  subtitle: {
    fontSize: 14,
    marginBottom: 32,
  },
  title: {
    fontSize: 18,
    fontWeight: "600",
    marginBottom: 16,
  },
  button: {
    paddingHorizontal: 24,
    paddingVertical: 14,
    borderRadius: 12,
    minWidth: 200,
    alignItems: "center",
  },
  buttonText: {
    color: "#fff",
    fontWeight: "600",
    fontSize: 16,
  },
  link: {
    marginTop: 16,
    fontSize: 14,
  },
  divider: {
    flexDirection: "row",
    alignItems: "center",
    marginVertical: 24,
    width: "100%",
    maxWidth: 300,
  },
  dividerLine: {
    flex: 1,
    height: 1,
  },
  dividerText: {
    marginHorizontal: 12,
    fontSize: 12,
  },
  card: {
    width: "100%",
    maxWidth: 300,
    padding: 16,
    borderRadius: 12,
    gap: 12,
  },
  cardTitle: {
    fontSize: 14,
    fontWeight: "600",
    marginBottom: 4,
  },
  input: {
    borderWidth: 1,
    borderRadius: 8,
    padding: 12,
    fontSize: 14,
  },
  demoButton: {
    borderWidth: 1,
    borderStyle: "dashed",
    borderRadius: 10,
    paddingVertical: 10,
    paddingHorizontal: 20,
    marginBottom: 16,
  },
  demoText: {
    fontSize: 13,
  },
  camera: {
    flex: 1,
    width: "100%",
  },
  scanOverlay: {
    position: "absolute",
    top: 0,
    left: 0,
    right: 0,
    bottom: 0,
    justifyContent: "center",
    alignItems: "center",
  },
  scanFrame: {
    width: 240,
    height: 240,
    borderWidth: 2,
    borderColor: "rgba(99,102,241,0.8)",
    borderRadius: 20,
    marginBottom: 24,
  },
  scanText: {
    color: "#fff",
    fontSize: 16,
    fontWeight: "500",
    textShadowColor: "#000",
    textShadowRadius: 4,
  },
});
