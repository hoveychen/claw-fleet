import { useCallback, useEffect, useRef, useMemo, useState } from "react";
import {
  FlatList,
  Modal,
  Pressable,
  StyleSheet,
  TextInput,
  View,
} from "react-native";
import {
  Button,
  Chip,
  IconButton,
  Snackbar,
  Text,
  useTheme,
  ActivityIndicator,
} from "react-native-paper";
import * as ImagePicker from "expo-image-picker";
import * as DocumentPicker from "expo-document-picker";
import { useDecisionsStore } from "../stores/decisions";
import { useConnectionStore } from "../stores/connection";
import { brandColors } from "../theme";
import { useT } from "../i18n";
import type {
  GuardDecision,
  ElicitationDecision,
  PendingDecision,
} from "../types";

function basename(p: string): string {
  const q = p.split("?")[0].split("#")[0];
  const slash = Math.max(q.lastIndexOf("/"), q.lastIndexOf("\\"));
  return slash >= 0 ? q.slice(slash + 1) : q;
}

async function uriToBytes(uri: string): Promise<Uint8Array> {
  const res = await fetch(uri);
  const buf = await res.arrayBuffer();
  return new Uint8Array(buf);
}

// ── Guard Card ──────────────────────────────────────────────────────────────

function GuardCard({ decision }: { decision: GuardDecision }) {
  const theme = useTheme();
  const t = useT();
  const client = useConnectionStore((s) => s.client);
  const remove = useDecisionsStore((s) => s.remove);
  const setAnalysis = useDecisionsStore((s) => s.setGuardAnalysis);
  const setAnalyzing = useDecisionsStore((s) => s.setGuardAnalyzing);
  const { request } = decision;

  // Trigger AI analysis on mount
  useEffect(() => {
    if (decision.analysis !== null || decision.analyzing) return;
    if (!client) return;
    setAnalyzing(decision.id, true);
    client
      .analyzeGuard(request.command, "", "en")
      .then((result) => setAnalysis(decision.id, result))
      .catch(() => setAnalysis(decision.id, null));
  }, [decision.id]);

  const handleAllow = useCallback(() => {
    client?.respondGuard(request.id, true).catch(() => {});
    remove(decision.id);
  }, [client, request.id, decision.id]);

  const handleBlock = useCallback(() => {
    client?.respondGuard(request.id, false).catch(() => {});
    remove(decision.id);
  }, [client, request.id, decision.id]);

  return (
    <View style={styles.card}>
      {/* Header */}
      <View style={styles.cardHeader}>
        <View
          style={[styles.iconCircle, { backgroundColor: brandColors.error + "20" }]}
        >
          <Text style={{ fontSize: 18 }}>⚠️</Text>
        </View>
        <View style={styles.headerText}>
          <Text variant="titleMedium" style={{ color: theme.colors.onSurface }}>
            {t("decision.guard_title")}
          </Text>
          <Text
            variant="bodySmall"
            style={{ color: theme.colors.onSurfaceVariant }}
          >
            {t("decision.workspace")}: {request.workspaceName}
          </Text>
        </View>
      </View>

      {/* Command */}
      <View style={styles.section}>
        <Text
          variant="labelMedium"
          style={{ color: theme.colors.onSurfaceVariant, marginBottom: 4 }}
        >
          {t("decision.guard_command")}
        </Text>
        <View
          style={[
            styles.commandBox,
            { backgroundColor: theme.colors.surfaceVariant },
          ]}
        >
          <Text
            variant="bodySmall"
            style={{ fontFamily: "monospace", color: theme.colors.onSurface }}
            numberOfLines={6}
          >
            {request.command}
          </Text>
        </View>
      </View>

      {/* Risk tags */}
      {request.riskTags.length > 0 && (
        <View style={styles.section}>
          <Text
            variant="labelMedium"
            style={{ color: theme.colors.onSurfaceVariant, marginBottom: 4 }}
          >
            {t("decision.guard_risk")}
          </Text>
          <View style={styles.tagsRow}>
            {request.riskTags.map((tag) => (
              <Chip
                key={tag}
                compact
                style={[styles.riskChip, { backgroundColor: brandColors.error + "20" }]}
                textStyle={{ color: brandColors.error, fontSize: 11 }}
              >
                {tag}
              </Chip>
            ))}
          </View>
        </View>
      )}

      {/* AI Analysis */}
      <View style={styles.section}>
        <Text
          variant="labelMedium"
          style={{ color: theme.colors.onSurfaceVariant, marginBottom: 4 }}
        >
          {t("decision.guard_analysis")}
        </Text>
        {decision.analyzing ? (
          <View style={styles.analysisLoading}>
            <ActivityIndicator size="small" />
            <Text
              variant="bodySmall"
              style={{ color: theme.colors.onSurfaceVariant, marginLeft: 8 }}
            >
              {t("decision.guard_analyzing")}
            </Text>
          </View>
        ) : decision.analysis ? (
          <Text variant="bodySmall" style={{ color: theme.colors.onSurface }}>
            {decision.analysis}
          </Text>
        ) : (
          <Text
            variant="bodySmall"
            style={{ color: theme.colors.onSurfaceVariant, fontStyle: "italic" }}
          >
            {t("decision.guard_analysis_unavailable")}
          </Text>
        )}
      </View>

      {/* Action buttons */}
      <View style={styles.actionRow}>
        <Button
          mode="contained"
          onPress={handleBlock}
          buttonColor={brandColors.error}
          textColor="#fff"
          style={styles.actionButton}
        >
          {t("decision.block")}
        </Button>
        <Button
          mode="contained"
          onPress={handleAllow}
          buttonColor={brandColors.primary}
          textColor="#fff"
          style={styles.actionButton}
        >
          {t("decision.allow")}
        </Button>
      </View>
    </View>
  );
}

// ── Elicitation Card ────────────────────────────────────────────────────────

function ElicitationCard({ decision }: { decision: ElicitationDecision }) {
  const theme = useTheme();
  const t = useT();
  const client = useConnectionStore((s) => s.client);
  const remove = useDecisionsStore((s) => s.remove);
  const setStep = useDecisionsStore((s) => s.setElicitationStep);
  const toggleSelection = useDecisionsStore((s) => s.toggleSelection);
  const setCustomAnswer = useDecisionsStore((s) => s.setCustomAnswer);
  const setMultiSelectOverride = useDecisionsStore(
    (s) => s.setMultiSelectOverride,
  );
  const addAttachment = useDecisionsStore((s) => s.addAttachment);
  const removeAttachment = useDecisionsStore((s) => s.removeAttachment);
  const [attachError, setAttachError] = useState<string | null>(null);
  const { request, step, selections, customAnswers, multiSelectOverrides, attachments } = decision;
  const question = request.questions[step];
  const totalSteps = request.questions.length;
  const isLast = step === totalSteps - 1;

  const currentSelections = question ? (selections[question.question] ?? []) : [];
  const currentCustom = question ? (customAnswers[question.question] ?? "") : "";
  const currentAttachments = question ? (attachments[question.question] ?? []) : [];
  const effectiveMulti = question
    ? question.multiSelect || multiSelectOverrides[question.question] === true
    : false;
  const canToggleMode = !!question && !question.multiSelect;

  const uploadAndAttach = useCallback(
    async (uri: string, name: string) => {
      if (!client || !question) return;
      try {
        const bytes = await uriToBytes(uri);
        const path = await client.uploadElicitationAttachment(bytes, name);
        addAttachment(decision.id, question.question, path, name);
      } catch (err) {
        const detail = err instanceof Error ? err.message : String(err);
        setAttachError(`${t("decision.attach_failed")}: ${detail}`);
      }
    },
    [client, decision.id, question, addAttachment, t],
  );

  const handlePickImage = useCallback(async () => {
    try {
      const perm = await ImagePicker.requestMediaLibraryPermissionsAsync();
      if (!perm.granted) return;
      const result = await ImagePicker.launchImageLibraryAsync({
        mediaTypes: ImagePicker.MediaTypeOptions.All,
        allowsMultipleSelection: true,
        quality: 1,
      });
      if (result.canceled) return;
      for (const asset of result.assets ?? []) {
        const name = asset.fileName || basename(asset.uri) || "image";
        await uploadAndAttach(asset.uri, name);
      }
    } catch (err) {
      const detail = err instanceof Error ? err.message : String(err);
      setAttachError(`${t("decision.attach_failed")}: ${detail}`);
    }
  }, [uploadAndAttach, t]);

  const handlePickDocument = useCallback(async () => {
    try {
      const result = await DocumentPicker.getDocumentAsync({
        multiple: true,
        copyToCacheDirectory: true,
      });
      if (result.canceled) return;
      for (const asset of result.assets ?? []) {
        const name = asset.name || basename(asset.uri) || "file";
        await uploadAndAttach(asset.uri, name);
      }
    } catch (err) {
      const detail = err instanceof Error ? err.message : String(err);
      setAttachError(`${t("decision.attach_failed")}: ${detail}`);
    }
  }, [uploadAndAttach, t]);

  const handleDecline = useCallback(() => {
    client?.respondElicitation(request.id, true, {}).catch(() => {});
    remove(decision.id);
  }, [client, request.id, decision.id]);

  const handleSubmit = useCallback(() => {
    // Build answers map: question text -> joined selection labels or custom text
    const answers: Record<string, string> = {};
    for (const q of request.questions) {
      const sel = selections[q.question] ?? [];
      const custom = customAnswers[q.question] ?? "";
      let answer = "";
      if (custom) {
        answer = custom;
      } else if (sel.length > 0) {
        answer = sel.join(", ");
      }
      const overridden = multiSelectOverrides[q.question] === true && !q.multiSelect;
      if (overridden && answer) {
        answer = `${answer} [用户将此题从单选改为多选 / user switched this question from single-select to multi-select]`;
      }
      const atts = attachments[q.question] ?? [];
      if (atts.length > 0) {
        const mentions = atts.map((a) => `@${a.path}`).join(" ");
        answer = answer ? `${answer} ${mentions}` : mentions;
      }
      if (answer) answers[q.question] = answer;
    }
    client?.respondElicitation(request.id, false, answers).catch(() => {});
    remove(decision.id);
  }, [client, request.id, decision.id, selections, customAnswers, multiSelectOverrides, attachments, request.questions]);

  if (!question) return null;

  return (
    <View style={styles.card}>
      {/* Header */}
      <View style={styles.cardHeader}>
        <View
          style={[styles.iconCircle, { backgroundColor: brandColors.primary + "20" }]}
        >
          <Text style={{ fontSize: 18 }}>💬</Text>
        </View>
        <View style={styles.headerText}>
          <Text variant="titleMedium" style={{ color: theme.colors.onSurface }}>
            {t("decision.elicitation_title")}
          </Text>
          <Text
            variant="bodySmall"
            style={{ color: theme.colors.onSurfaceVariant }}
          >
            {t("decision.workspace")}: {request.workspaceName}
          </Text>
        </View>
      </View>

      {/* Step indicator + mode toggle row */}
      {(totalSteps > 1 || canToggleMode) && (
        <View style={styles.stepRow}>
          {totalSteps > 1 && (
            <Text
              variant="labelSmall"
              style={{ color: theme.colors.onSurfaceVariant }}
            >
              {t("decision.step", { current: step + 1, total: totalSteps })}
            </Text>
          )}
          {canToggleMode && question && (
            <Pressable
              onPress={() =>
                setMultiSelectOverride(
                  decision.id,
                  question.question,
                  !effectiveMulti,
                )
              }
              style={[
                styles.modeToggle,
                {
                  backgroundColor: effectiveMulti
                    ? brandColors.primary + "24"
                    : theme.colors.surfaceVariant,
                  borderColor: effectiveMulti
                    ? brandColors.primary
                    : theme.colors.outline,
                },
              ]}
            >
              <Text
                variant="labelSmall"
                style={{
                  color: effectiveMulti
                    ? brandColors.primary
                    : theme.colors.onSurfaceVariant,
                  fontWeight: "600",
                }}
              >
                {effectiveMulti
                  ? t("decision.mode_multi")
                  : t("decision.mode_single")}
              </Text>
            </Pressable>
          )}
        </View>
      )}

      {/* Question */}
      <Text
        variant="bodyMedium"
        style={{ color: theme.colors.onSurface, marginBottom: 12, fontWeight: "600" }}
      >
        {question.header || question.question}
      </Text>

      {/* Options */}
      {question.options.length > 0 && (
        <View style={styles.optionsContainer}>
          {question.options.map((opt) => {
            const selected = currentSelections.includes(opt.label);
            return (
              <View key={opt.label} style={styles.optionRow}>
                <Pressable
                  onPress={() =>
                    toggleSelection(
                      decision.id,
                      question.question,
                      opt.label,
                      effectiveMulti,
                    )
                  }
                  style={[
                    styles.optionItem,
                    styles.optionItemFlex,
                    {
                      backgroundColor: selected
                        ? brandColors.primary + "18"
                        : theme.colors.surfaceVariant,
                      borderColor: selected
                        ? brandColors.primary
                        : theme.colors.outline,
                    },
                  ]}
                >
                  <Text
                    variant="bodySmall"
                    style={{
                      color: selected
                        ? brandColors.primary
                        : theme.colors.onSurface,
                      fontWeight: selected ? "600" : "400",
                    }}
                  >
                    {opt.label}
                  </Text>
                  {opt.description ? (
                    <Text
                      variant="bodySmall"
                      style={{
                        color: theme.colors.onSurfaceVariant,
                        fontSize: 11,
                        marginTop: 2,
                      }}
                    >
                      {opt.description}
                    </Text>
                  ) : null}
                </Pressable>
                <Pressable
                  onPress={() => {
                    const seed = opt.description
                      ? `${opt.label} — ${opt.description}`
                      : opt.label;
                    setCustomAnswer(decision.id, question.question, seed);
                  }}
                  accessibilityLabel={t("decision.edit_option")}
                  style={[
                    styles.editOptionButton,
                    {
                      backgroundColor: theme.colors.surfaceVariant,
                      borderColor: theme.colors.outline,
                    },
                  ]}
                >
                  <Text style={{ fontSize: 14 }}>✏️</Text>
                </Pressable>
              </View>
            );
          })}
        </View>
      )}

      {/* Custom answer ("Other") */}
      <View style={styles.section}>
        <Text
          variant="labelSmall"
          style={{ color: theme.colors.onSurfaceVariant, marginBottom: 4 }}
        >
          {t("decision.other")}
        </Text>
        {currentAttachments.length > 0 && (
          <View style={styles.attachmentRow}>
            {currentAttachments.map((a) => (
              <Chip
                key={a.path}
                compact
                onClose={() =>
                  removeAttachment(decision.id, question.question, a.path)
                }
                closeIconAccessibilityLabel={t("decision.attachment_remove")}
                style={styles.attachmentChip}
              >
                {a.name}
                {a.fromClipboard ? ` · ${t("decision.attachment_pasted")}` : ""}
              </Chip>
            ))}
          </View>
        )}
        <View style={styles.otherRow}>
          <IconButton
            icon="paperclip"
            size={18}
            onPress={handlePickDocument}
            accessibilityLabel={t("decision.attach_file")}
            style={styles.attachIconBtn}
          />
          <IconButton
            icon="image-outline"
            size={18}
            onPress={handlePickImage}
            accessibilityLabel={t("decision.attach_image")}
            style={styles.attachIconBtn}
          />
          <TextInput
            value={currentCustom}
            onChangeText={(text) =>
              setCustomAnswer(decision.id, question.question, text)
            }
            placeholder={t("decision.other_placeholder")}
            placeholderTextColor={theme.colors.onSurfaceVariant}
            style={[
              styles.textInput,
              styles.textInputFlex,
              {
                backgroundColor: theme.colors.surfaceVariant,
                color: theme.colors.onSurface,
                borderColor: theme.colors.outline,
              },
            ]}
            multiline
          />
        </View>
      </View>

      {/* Navigation + action buttons */}
      <View style={styles.actionRow}>
        <Button
          mode="text"
          onPress={handleDecline}
          textColor={brandColors.error}
        >
          {t("decision.decline")}
        </Button>
        <View style={{ flex: 1 }} />
        {step > 0 && (
          <Button
            mode="outlined"
            onPress={() => setStep(decision.id, step - 1)}
            style={{ marginRight: 8 }}
          >
            {t("decision.back")}
          </Button>
        )}
        {isLast ? (
          <Button
            mode="contained"
            onPress={handleSubmit}
            buttonColor={brandColors.primary}
            textColor="#fff"
          >
            {t("decision.submit")}
          </Button>
        ) : (
          <Button
            mode="contained"
            onPress={() => setStep(decision.id, step + 1)}
            buttonColor={brandColors.primary}
            textColor="#fff"
          >
            {t("decision.next")}
          </Button>
        )}
      </View>
      <Snackbar
        visible={attachError !== null}
        onDismiss={() => setAttachError(null)}
        duration={5000}
        action={{
          label: t("decision.attach_error_dismiss"),
          onPress: () => setAttachError(null),
        }}
      >
        {attachError ?? ""}
      </Snackbar>
    </View>
  );
}

// ── Decision Panel (full-screen modal) ──────────────────────────────────────

function DecisionCard({ decision }: { decision: PendingDecision }) {
  if (decision.kind === "guard") return <GuardCard decision={decision} />;
  if (decision.kind === "elicitation")
    return <ElicitationCard decision={decision} />;
  return null;
}

export function DecisionPanel({
  visible,
  onDismiss,
}: {
  visible: boolean;
  onDismiss: () => void;
}) {
  const theme = useTheme();
  const t = useT();
  const decisions = useDecisionsStore((s) => s.decisions);
  const focusedId = useDecisionsStore((s) => s.focusedId);
  const setFocusedId = useDecisionsStore((s) => s.setFocusedId);
  const listRef = useRef<FlatList>(null);

  // Sort: focused decision first, then by arrival time
  const sorted = useMemo(() => {
    if (!focusedId) return decisions;
    return [...decisions].sort((a, b) => {
      if (a.id === focusedId) return -1;
      if (b.id === focusedId) return 1;
      return a.arrivedAt - b.arrivedAt;
    });
  }, [decisions, focusedId]);

  // Scroll to focused decision when panel opens
  useEffect(() => {
    if (visible && focusedId && sorted.length > 0) {
      const idx = sorted.findIndex((d) => d.id === focusedId);
      if (idx >= 0) {
        // Small delay to ensure FlatList has rendered
        setTimeout(() => {
          listRef.current?.scrollToIndex({ index: idx, animated: true });
        }, 300);
      }
      // Clear focused after scrolling
      setFocusedId(null);
    }
  }, [visible, focusedId]);

  // Auto-dismiss when no more decisions
  useEffect(() => {
    if (visible && decisions.length === 0) {
      onDismiss();
    }
  }, [visible, decisions.length]);

  const renderItem = useCallback(
    ({ item }: { item: PendingDecision }) => (
      <View style={{ paddingHorizontal: 16, paddingBottom: 16 }}>
        <DecisionCard decision={item} />
      </View>
    ),
    [],
  );

  return (
    <Modal
      visible={visible && decisions.length > 0}
      animationType="slide"
      presentationStyle="pageSheet"
      onRequestClose={onDismiss}
    >
      <View
        style={[styles.modalContainer, { backgroundColor: theme.colors.background }]}
      >
        {/* Top bar */}
        <View style={styles.topBar}>
          <Text variant="titleLarge" style={{ color: theme.colors.onSurface }}>
            {t("decision.title")}
          </Text>
          <IconButton icon="close" onPress={onDismiss} />
        </View>

        <FlatList
          ref={listRef}
          data={sorted}
          keyExtractor={(d) => d.id}
          renderItem={renderItem}
          contentContainerStyle={{ paddingTop: 16 }}
          onScrollToIndexFailed={() => {}}
        />
      </View>
    </Modal>
  );
}

// ── Floating badge / FAB that opens the panel ───────────────────────────────

export function DecisionBadge({ onPress }: { onPress: () => void }) {
  const count = useDecisionsStore((s) => s.pendingCount);
  if (count === 0) return null;

  return (
    <Pressable onPress={onPress} style={styles.fab}>
      <Text style={styles.fabIcon}>⚡</Text>
      <View style={styles.fabBadge}>
        <Text style={styles.fabBadgeText}>{count}</Text>
      </View>
    </Pressable>
  );
}

// ── Styles ──────────────────────────────────────────────────────────────────

const styles = StyleSheet.create({
  modalContainer: {
    flex: 1,
  },
  topBar: {
    flexDirection: "row",
    alignItems: "center",
    justifyContent: "space-between",
    paddingHorizontal: 16,
    paddingTop: 12,
    paddingBottom: 8,
  },
  scrollView: {
    flex: 1,
  },
  scrollContent: {
    padding: 16,
    gap: 16,
  },
  card: {
    borderRadius: 12,
    padding: 16,
    backgroundColor: "rgba(255,255,255,0.05)",
    borderWidth: 1,
    borderColor: "rgba(255,255,255,0.1)",
  },
  cardHeader: {
    flexDirection: "row",
    alignItems: "center",
    marginBottom: 12,
  },
  iconCircle: {
    width: 36,
    height: 36,
    borderRadius: 18,
    alignItems: "center",
    justifyContent: "center",
  },
  headerText: {
    marginLeft: 10,
    flex: 1,
  },
  section: {
    marginBottom: 12,
  },
  commandBox: {
    borderRadius: 8,
    padding: 10,
  },
  tagsRow: {
    flexDirection: "row",
    flexWrap: "wrap",
    gap: 6,
  },
  riskChip: {
    height: 26,
  },
  analysisLoading: {
    flexDirection: "row",
    alignItems: "center",
  },
  actionRow: {
    flexDirection: "row",
    alignItems: "center",
    justifyContent: "flex-end",
    gap: 8,
    marginTop: 4,
  },
  actionButton: {
    minWidth: 100,
  },
  optionsContainer: {
    gap: 8,
    marginBottom: 12,
  },
  optionRow: {
    flexDirection: "row",
    alignItems: "stretch",
    gap: 6,
  },
  optionItem: {
    borderRadius: 8,
    borderWidth: 1,
    padding: 10,
  },
  optionItemFlex: {
    flex: 1,
  },
  editOptionButton: {
    width: 40,
    borderRadius: 8,
    borderWidth: 1,
    alignItems: "center",
    justifyContent: "center",
  },
  stepRow: {
    flexDirection: "row",
    alignItems: "center",
    justifyContent: "space-between",
    marginBottom: 8,
    gap: 8,
  },
  modeToggle: {
    paddingHorizontal: 10,
    paddingVertical: 3,
    borderRadius: 12,
    borderWidth: 1,
  },
  textInput: {
    borderRadius: 8,
    borderWidth: 1,
    padding: 10,
    fontSize: 14,
    minHeight: 44,
    maxHeight: 120,
  },
  textInputFlex: {
    flex: 1,
  },
  otherRow: {
    flexDirection: "row",
    alignItems: "flex-start",
    gap: 4,
  },
  attachIconBtn: {
    margin: 0,
    marginTop: 2,
  },
  attachmentRow: {
    flexDirection: "row",
    flexWrap: "wrap",
    gap: 6,
    marginBottom: 6,
  },
  attachmentChip: {
    maxWidth: 220,
  },
  fab: {
    position: "absolute",
    right: 16,
    bottom: 80,
    width: 52,
    height: 52,
    borderRadius: 26,
    backgroundColor: brandColors.error,
    alignItems: "center",
    justifyContent: "center",
    elevation: 6,
    shadowColor: "#000",
    shadowOffset: { width: 0, height: 3 },
    shadowOpacity: 0.3,
    shadowRadius: 4,
  },
  fabIcon: {
    fontSize: 22,
  },
  fabBadge: {
    position: "absolute",
    top: -4,
    right: -4,
    minWidth: 20,
    height: 20,
    borderRadius: 10,
    backgroundColor: "#fff",
    alignItems: "center",
    justifyContent: "center",
    paddingHorizontal: 4,
  },
  fabBadgeText: {
    fontSize: 11,
    fontWeight: "700",
    color: brandColors.error,
  },
});
