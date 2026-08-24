import { invoke } from "@tauri-apps/api/core";
import { open as openDialog } from "@tauri-apps/plugin-dialog";
import { useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { FileText, Folder } from "lucide-react";
import {
  ChatComposer,
  type ChatComposerAttachment,
  type ChatComposerHandle,
  type ChatComposerStagedAttachment,
} from "./ChatComposer";
import { SessionOptionPills } from "./SessionOptionPills";
import { toolForAgentSource } from "../modelChoices";
import { StopControl, canControl } from "./StopControl";
import { useComposerDraft } from "../composerDraft";
import { resolveStagedAttachment } from "../userAttachments";
import { isWebBuild } from "../hostEnv";
import { DirPickerDialog } from "./DirPickerDialog";
import { useConnectionStore } from "../store";
import type { SessionInfo } from "../types";
import styles from "./ResumeComposer.module.css";

function basename(p: string): string {
  const normalized = p.replace(/\\/g, "/");
  const slash = normalized.lastIndexOf("/");
  return slash >= 0 ? normalized.slice(slash + 1) : normalized;
}

/** Full-featured resume form for the history panel — the same ChatComposer +
 *  option-pill surface as NewSessionForm (paste screenshots, attach files or
 *  a folder, override model / effort / permission mode), targeting
 *  `claude --resume <sid>` instead of a fresh session. `""` overrides mean
 *  "don't pass the flag": the resumed session keeps its own settings. */
export function ResumeComposer({
  sessionId,
  workspacePath,
  agentSource,
  session,
  onResumed,
  mode = "resume",
  pendingMessages = [],
}: {
  sessionId: string;
  workspacePath: string;
  /** Session's source ("claude-code" / "codex"), so the resume routes to the
   *  right launcher. Defaults to claude when omitted. */
  agentSource?: string;
  /** The live session, so an enqueue composer can surface a Stop button (same
   *  interrupt/kill escalation as the session list) in the empty send slot
   *  while the turn is running. */
  session?: SessionInfo;
  /** Fired the moment the backend accepts the follow-up, with the final prompt
   *  text (including any appended attachment context) and which mode delivered
   *  it. The parent echoes a resume as an optimistic user bubble while
   *  `claude --resume` cold-starts; an enqueue keeps its "已排队" chip instead. */
  onResumed: (finalPrompt: string, mode: "resume" | "enqueue") => void;
  /** `"resume"`: the turn ended, submit spawns `claude --resume` now.
   *  `"enqueue"`: the turn is still running, submit queues the message to be
   *  delivered when the turn ends (see `pending_message`). */
  mode?: "resume" | "enqueue";
  /** Follow-ups already queued for this session (from `SessionInfo`), shown as
   *  chips so the user sees what's waiting. */
  pendingMessages?: string[];
}) {
  const enqueueing = mode === "enqueue";
  const { t } = useTranslation();
  // Draft (prompt / options / attachments) is lifted into a store keyed by the
  // session id, so switching to another session and back — or the resume dock
  // unmounting — no longer wipes an in-progress follow-up. Each session keeps
  // its own draft slot.
  const { draft, patch, clear } = useComposerDraft(sessionId);
  const { prompt, model, effort, permissionMode, attachments } = draft;
  const setPrompt = (v: string) => patch({ prompt: v });
  const setModel = (v: string) => patch({ model: v });
  const setEffort = (v: string) => patch({ effort: v });
  const setPermissionMode = (v: string) => patch({ permissionMode: v });
  const [submitting, setSubmitting] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [cancellingIndex, setCancellingIndex] = useState<number | null>(null);
  const composerRef = useRef<ChatComposerHandle | null>(null);
  // The native directory dialog browses the machine the *desktop* runs on: the
  // wrong one under a remote connection, and absent entirely in the browser
  // build. `DirPickerDialog` browses whichever host the backend is bound to,
  // which is the host that will read the attachment (mirrors NewSessionForm).
  const isRemote = useConnectionStore((c) => c.connection?.type === "remote");
  const needsBackendDirPicker = isRemote || isWebBuild();
  const [pickingDir, setPickingDir] = useState(false);

  // Drop one queued follow-up by its chip position. The backend re-emits the
  // sessions snapshot on success, so the chip disappears without local state.
  const handleCancelQueued = async (index: number) => {
    if (cancellingIndex !== null) return;
    setCancellingIndex(index);
    setError(null);
    try {
      await invoke("cancel_session_pending_message", { sessionId, index });
    } catch (e) {
      setError(String((e as { message?: string })?.message ?? e));
    } finally {
      setCancellingIndex(null);
    }
  };

  useEffect(() => {
    setTimeout(() => composerRef.current?.focus(), 50);
  }, []);

  const addAttachmentEntry = (entry: ChatComposerAttachment) => {
    // Updater form (not the render-time `attachments` snapshot): picking or
    // dropping several files fires this in a synchronous loop, and a snapshot
    // write would let each addition overwrite the last so only one survived.
    patch((d) =>
      d.attachments.some((a) => a.path === entry.path)
        ? {}
        : { attachments: [...d.attachments, entry] },
    );
  };

  const handleAddAttachment = async (s: ChatComposerStagedAttachment) => {
    addAttachmentEntry(await resolveStagedAttachment(s));
  };

  const handleRemoveAttachment = (path: string) => {
    const removed = attachments.find((a) => a.path === path);
    if (removed?.previewUrl) URL.revokeObjectURL(removed.previewUrl);
    patch({ attachments: attachments.filter((a) => a.path !== path) });
  };

  // Shared by the native file picker and OS drag-drop: both surface absolute
  // paths. addAttachmentEntry now accumulates across a loop (see its comment).
  const addPaths = (paths: string[]) => {
    for (const p of paths) {
      const path = typeof p === "string" ? p : String(p);
      addAttachmentEntry({ path, name: basename(path) });
    }
  };

  // Reported, not swallowed: opening the picker can fail (and in the browser
  // build it also *uploads*, which can be refused for size), and this menu item
  // has no other way to say so — an unhandled rejection here just leaves the
  // user staring at a composer that ignored their file.
  const pickFiles = async () => {
    try {
      const picked = await openDialog({ multiple: true, directory: false });
      if (picked == null) return;
      addPaths(Array.isArray(picked) ? picked : [picked]);
    } catch (e) {
      setError(`${t("composer.attach_failed", "Attachment upload failed")}: ${String(e)}`);
    }
  };

  const pickDirectory = async () => {
    if (needsBackendDirPicker) {
      setPickingDir(true);
      return;
    }
    const picked = await openDialog({ multiple: false, directory: true });
    if (typeof picked !== "string") return;
    addAttachmentEntry({ path: picked, name: basename(picked) });
  };

  const handleSubmit = async () => {
    if (!prompt.trim() || submitting) return;
    let finalPrompt = prompt.trim();
    if (attachments.length > 0) {
      finalPrompt += `\n\nContext files:\n${attachments.map((a) => `- ${a.path}`).join("\n")}`;
    }
    setSubmitting(true);
    setError(null);
    try {
      if (enqueueing) {
        // Turn still running: queue it. Model/effort are carried from the
        // session's own launch flags at drain time, so no overrides here.
        await invoke("enqueue_session_message", {
          sessionId,
          workspacePath,
          text: finalPrompt,
        });
      } else {
        await invoke("resume_rate_limited_session", {
          sessionId,
          workspacePath,
          prompt: finalPrompt,
          model: model || null,
          effort: effort || null,
          permissionMode: permissionMode || null,
          agentSource: agentSource || null,
        });
      }
      clear();
      onResumed(finalPrompt, mode);
    } catch (e) {
      setError(String((e as { message?: string })?.message ?? e));
    } finally {
      setSubmitting(false);
    }
  };

  return (
    <div className={styles.wrap}>
      {pendingMessages.length > 0 && (
        <div className={styles.queued}>
          <div className={styles.queued_label}>
            {t("history.enqueue_queued_label", "已排队，本轮结束后自动发送")}
          </div>
          {pendingMessages.map((m, i) => (
            <div key={i} className={styles.queued_chip}>
              <span className={styles.queued_text}>{m}</span>
              <button
                type="button"
                className={styles.queued_cancel}
                onClick={() => handleCancelQueued(i)}
                disabled={cancellingIndex !== null}
                title={t("history.enqueue_cancel", "取消这条排队消息")}
                aria-label={t("history.enqueue_cancel", "取消这条排队消息")}
              >
                ×
              </button>
            </div>
          ))}
        </div>
      )}
      <ChatComposer
        ref={composerRef}
        value={prompt}
        onChange={setPrompt}
        attachments={attachments}
        onAddAttachment={handleAddAttachment}
        onRemoveAttachment={handleRemoveAttachment}
        onDropFiles={addPaths}
        onAttachmentError={setError}
        onSubmit={handleSubmit}
        submitting={submitting}
        submitDisabled={!prompt.trim()}
        emptyTrailingControl={
          enqueueing && session && canControl(session) ? (
            <StopControl session={session} variant="solid" />
          ) : undefined
        }
        placeholder={
          enqueueing
            ? t("history.enqueue_placeholder", "会话运行中，发送后排队，本轮结束自动接上…")
            : t("history.resume_placeholder", "输入追问提示词后恢复会话…")
        }
        disabled={submitting}
        wikiMentions
        toolbarSlot={
          enqueueing ? undefined : (
          <SessionOptionPills
            placement="below"
            tool={toolForAgentSource(agentSource)}
            model={model}
            effort={effort}
            permissionMode={permissionMode}
            onModelChange={setModel}
            onEffortChange={setEffort}
            onPermissionModeChange={setPermissionMode}
            disabled={submitting}
            permissionDefaultLabel={t("history.resume_permission_default", "沿用会话设置")}
          />
          )
        }
        addMenuItems={[
          {
            id: "files",
            label: t("launcher.add_files"),
            onSelect: pickFiles,
            icon: <FileText size={14} strokeWidth={1.5} />,
          },
          {
            id: "folder",
            label: t("launcher.add_directory"),
            onSelect: pickDirectory,
            icon: <Folder size={14} strokeWidth={1.5} />,
          },
        ]}
      />
      {error && <div className={styles.error}>{error}</div>}
      {pickingDir && (
        <DirPickerDialog
          initialPath=""
          onPick={(path) => {
            addAttachmentEntry({ path, name: basename(path) });
            setPickingDir(false);
          }}
          onCancel={() => setPickingDir(false)}
        />
      )}
    </div>
  );
}
