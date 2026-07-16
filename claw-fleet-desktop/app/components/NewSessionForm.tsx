import { invoke } from "@tauri-apps/api/core";
import { open as openDialog } from "@tauri-apps/plugin-dialog";
import { useEffect, useMemo, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { FileText, Folder, FolderOpen, MessageCircle } from "lucide-react";
import { useConnectionStore, useSessionsStore } from "../store";
import {
  ChatComposer,
  type ChatComposerAttachment,
  type ChatComposerHandle,
  type ChatComposerStagedAttachment,
} from "./ChatComposer";
import { DirPickerDialog } from "./DirPickerDialog";
import { PillMenu } from "./PillMenu";
import pillStyles from "./PillMenu.module.css";
import { SessionOptionPills } from "./SessionOptionPills";
import { agentToolsForSources, type SourceInfo } from "../modelChoices";
import { useChatWorkspace } from "../hooks/useChatWorkspace";
import { useComposerDraft } from "../composerDraft";
import { resolveStagedAttachment } from "../userAttachments";
import styles from "./NewSessionForm.module.css";

export interface NewSessionCreated {
  /** PID of the spawned `claude` process — the caller matches it against
   *  `SessionInfo.pid` to locate the freshly-created session. */
  pid: number;
  /** Session id pre-assigned via `--session-id`; lets the caller correlate
   *  the scanned session directly by id. Absent when the backend is an older
   *  remote probe that only returns the pid. */
  sessionId?: string | null;
  /** Workspace the session was spawned in — fallback correlation key when the
   *  pid hasn't been attached to a `SessionInfo` yet. */
  workspacePath: string;
}

export interface NewSessionFormProps {
  /** Fired once the backend has spawned the detached `claude -p` process. */
  onCreated: (info: NewSessionCreated) => void;
  /** Fired when the user backs out of the form without creating a session. */
  onCancel: () => void;
}

function basename(p: string): string {
  const normalized = p.replace(/\\/g, "/");
  const slash = normalized.lastIndexOf("/");
  return slash >= 0 ? normalized.slice(slash + 1) : normalized;
}

/** Collapse an in-repo worktree checkout to its repo root. Fleet develops plans
 *  inside `<repo-root>/.worktrees/<task-id>` (see the worktree workflow), which
 *  are transient — removed once the plan merges. The launcher must offer the
 *  durable repo root, never the task-id leaf. Mirrors the backend's
 *  `workspace_name` segment logic (session.rs), but returns the *path* prefix
 *  instead of the name. Paths without a `.worktrees` segment (including the
 *  unrelated `~/.fleet/worktrees/` task-workers, whose segment is `worktrees`)
 *  are returned unchanged. */
export function repoRootPath(path: string): string {
  const normalized = path.replace(/\\/g, "/");
  const idx = normalized.split("/").indexOf(".worktrees");
  if (idx <= 0) return path;
  // Rejoin the segments before `.worktrees`, preserving the original separators
  // by slicing the raw string at the segment boundary.
  const before = normalized.split("/").slice(0, idx).join("/");
  return before || path;
}

/** True when `path` lives under an OS temp/scratchpad directory that should never
 *  be offered as a launchable workspace. Fleet (and Claude Code) drop per-session
 *  scratchpads under `/tmp` (`/private/tmp` on macOS, where `/tmp` symlinks) and
 *  the system uses `/var/folders/.../T` for per-user temp — sessions whose cwd is
 *  one of these are transient and would clutter the launcher's recents. Matches on
 *  a leading path segment so a real project merely *named* `tmp-tools` is kept. */
export function isTempWorkspacePath(path: string): boolean {
  const p = path.replace(/\\/g, "/");
  return (
    p === "/tmp" ||
    p.startsWith("/tmp/") ||
    p === "/private/tmp" ||
    p.startsWith("/private/tmp/") ||
    p.startsWith("/var/folders/")
  );
}

export interface WorkspaceOption {
  path: string;
  name: string;
  lastMs: number;
}

interface WorkspaceSessionLike {
  workspacePath?: string | null;
  workspaceName?: string | null;
  lastActivityMs: number;
}

/** Distinct workspaces from known sessions, most-recently-active first. In-repo
 *  worktree checkouts are collapsed onto their repo root so a repo with both a
 *  main checkout and a live worktree appears once, pointing at the durable root
 *  (see {@link repoRootPath}).
 *
 *  `chatPath` — when the chat workspace already has sessions it would otherwise
 *  show up here as an ordinary recent entry, duplicating the pinned one. Passing
 *  the path drops it from the recents. */
export function distinctWorkspaces(
  sessions: WorkspaceSessionLike[],
  limit = 30,
  chatPath?: string | null,
): WorkspaceOption[] {
  const byPath = new Map<string, WorkspaceOption>();
  for (const s of sessions) {
    if (!s.workspacePath) continue;
    const path = repoRootPath(s.workspacePath);
    if (isTempWorkspacePath(path)) continue;
    if (chatPath && path === chatPath) continue;
    const prev = byPath.get(path);
    if (!prev || s.lastActivityMs > prev.lastMs) {
      byPath.set(path, {
        path,
        name: s.workspaceName || basename(path),
        lastMs: s.lastActivityMs,
      });
    }
  }
  return [...byPath.values()].sort((a, b) => b.lastMs - a.lastMs).slice(0, limit);
}

/** Plain "start a new claude session" form — no project, no task, no queue.
 *  Pick a workspace directory + type the initial prompt; the backend spawns a
 *  detached `claude -p "<prompt>"` and the scanner picks the session up. Rendered
 *  inline inside the History page's detail column (no modal chrome); on success
 *  it hands the spawned pid back so the host can switch that column to the new
 *  session's live SessionDetail. Styled in the composer design language: ghost
 *  pills and custom popovers instead of labeled form rows and native
 *  <select>s. */
export function NewSessionForm({ onCreated, onCancel }: NewSessionFormProps) {
  const { t } = useTranslation();
  const sessions = useSessionsStore((s) => s.sessions);
  const { connection } = useConnectionStore();
  const isRemote = connection?.type === "remote";
  // Draft (prompt / options / attachments / workspace) is lifted into a store
  // keyed "new" so navigating away from the new-session page and back no longer
  // wipes it. Headless `-p` sessions in the CLI's default mode can't approve
  // file edits, so the launcher seeds permissionMode to acceptEdits.
  const { draft, patch, clear } = useComposerDraft("new", {
    permissionMode: "acceptEdits",
  });
  const { prompt, model, effort, permissionMode, attachments, workspace } = draft;
  // Empty tool means Claude (older drafts / resume composers never set it).
  const tool = draft.tool || "claude";
  const setPrompt = (v: string) => patch({ prompt: v });
  const setModel = (v: string) => patch({ model: v });
  const setEffort = (v: string) => patch({ effort: v });
  const setPermissionMode = (v: string) => patch({ permissionMode: v });
  const setWorkspace = (v: string) => patch({ workspace: v });
  // Switching tool clears model/effort: Claude and Codex model ids are
  // disjoint, so a leftover Claude model would reach `codex exec -m` (and vice
  // versa) as an invalid value.
  const setTool = (v: string) => patch({ tool: v, model: "", effort: "" });

  // Only offer the agent tools whose source is actually being monitored (source
  // enabled in settings AND the CLI installed). Codex must not appear in the
  // launcher when its source is off — selecting it would only fail at spawn.
  const [sources, setSources] = useState<SourceInfo[] | null>(null);
  useEffect(() => {
    invoke<SourceInfo[]>("get_sources_config").then(setSources).catch(() => {});
  }, []);
  // `null` = not loaded yet → keep Claude-only so we never flash Codex then hide
  // it. Once loaded, filter to the monitored sources.
  const toolChoices = useMemo(() => agentToolsForSources(sources ?? []), [sources]);
  // If a stale draft (or a since-disabled source) left `tool` pointing at a tool
  // that's no longer offered, snap it back to the first available one.
  useEffect(() => {
    if (sources === null) return;
    if (!toolChoices.some((c) => c.value === tool)) {
      setTool(toolChoices[0].value);
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [sources, toolChoices, tool]);
  const [submitting, setSubmitting] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [pathDraft, setPathDraft] = useState("");
  // Remote-only: the backend-driven directory picker standing in for the native
  // dialog, which can only browse this desktop's own disk.
  const [pickingDir, setPickingDir] = useState(false);
  const composerRef = useRef<ChatComposerHandle | null>(null);

  // The pure-chat workspace. Unlike a project it has no prior sessions to be
  // discovered from, so it must be pinned explicitly.
  const chatPath = useChatWorkspace();

  // Distinct workspaces from known sessions, most recently active first.
  // Worktree checkouts collapse onto their repo root — see distinctWorkspaces.
  const recentWorkspaces = useMemo(
    () => distinctWorkspaces(sessions, 30, chatPath),
    [sessions, chatPath],
  );

  const isChat = !!chatPath && workspace === chatPath;

  // Seed the workspace to the most-recent one once, on mount. The form is
  // remounted each time the user re-enters "new session" mode, so a plain
  // mount effect stands in for the old modal's open-reset. recentWorkspaces is
  // read once here on purpose — re-running on every 5s session poll would
  // clobber the user's in-progress selection.
  useEffect(() => {
    if (!draft.workspace && recentWorkspaces[0]?.path) {
      patch({ workspace: recentWorkspaces[0].path });
    }
    const id = setTimeout(() => composerRef.current?.focus(), 50);
    return () => clearTimeout(id);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  const addAttachmentEntry = (entry: ChatComposerAttachment) => {
    if (attachments.some((a) => a.path === entry.path)) return;
    patch({ attachments: [...attachments, entry] });
  };

  const handleAddAttachment = async (s: ChatComposerStagedAttachment) => {
    addAttachmentEntry(await resolveStagedAttachment(s));
  };

  const handleRemoveAttachment = (path: string) => {
    const removed = attachments.find((a) => a.path === path);
    if (removed?.previewUrl) URL.revokeObjectURL(removed.previewUrl);
    patch({ attachments: attachments.filter((a) => a.path !== path) });
  };

  const pickFiles = async () => {
    const picked = await openDialog({ multiple: true, directory: false });
    if (picked == null) return;
    const arr = Array.isArray(picked) ? picked : [picked];
    for (const p of arr) {
      const path = typeof p === "string" ? p : String(p);
      addAttachmentEntry({ path, name: basename(path) });
    }
  };

  const pickDirectory = async () => {
    const picked = await openDialog({ multiple: false, directory: true });
    if (typeof picked !== "string") return;
    addAttachmentEntry({ path: picked, name: basename(picked) });
  };

  // Local: the native dialog is the better experience (sidebar, favourites,
  // search) and it browses the right machine. Remote: it would browse *this*
  // machine while the session spawns on the probe, so go through the backend
  // instead — that's what DirPickerDialog does.
  const browseWorkspace = async () => {
    if (isRemote) {
      setPickingDir(true);
      return;
    }
    const picked = await openDialog({ multiple: false, directory: true });
    if (typeof picked === "string") setWorkspace(picked);
  };

  const handleSubmit = async () => {
    const ws = workspace.trim();
    if (!ws) {
      setError(t("new_session.error_workspace_required"));
      return;
    }
    if (!prompt.trim()) {
      setError(t("new_session.error_prompt_required"));
      return;
    }
    let finalPrompt = prompt.trim();
    if (attachments.length > 0) {
      finalPrompt += `\n\nContext files:\n${attachments.map((a) => `- ${a.path}`).join("\n")}`;
    }
    setSubmitting(true);
    setError(null);
    try {
      const resp = await invoke<{ pid: number; sessionId?: string | null }>(
        "spawn_new_claude_session",
        {
          workspacePath: ws,
          prompt: finalPrompt,
          model: model || null,
          effort: effort || null,
          permissionMode: permissionMode || null,
          tool,
        },
      );
      clear();
      onCreated({ pid: resp.pid, sessionId: resp.sessionId, workspacePath: ws });
    } catch (e) {
      setError(String(e));
      setSubmitting(false);
    }
  };

  const workspacePill = (
    <PillMenu
      placement="below"
      icon={
        isChat ? (
          <MessageCircle size={13} strokeWidth={1.7} />
        ) : (
          <FolderOpen size={13} strokeWidth={1.7} />
        )
      }
      label={
        isChat
          ? t("new_session.chat")
          : workspace
            ? basename(workspace)
            : t("new_session.workspace")
      }
      title={
        isChat
          ? t("new_session.chat_sub")
          : workspace || t("new_session.workspace_placeholder")
      }
      disabled={submitting}
      menuHeader={(close) => (
        <div className={pillStyles.menu_header}>
          <input
            type="text"
            className={pillStyles.menu_header_input}
            value={pathDraft}
            onChange={(e) => setPathDraft(e.target.value)}
            onKeyDown={(e) => {
              if (e.key === "Enter" && pathDraft.trim()) {
                setWorkspace(pathDraft.trim());
                setPathDraft("");
                close();
              }
            }}
            placeholder={t("new_session.workspace_placeholder")}
            spellCheck={false}
          />
        </div>
      )}
      items={[
        // Pinned first: the chat workspace is a destination, not a recent.
        ...(chatPath
          ? [
              {
                // No `icon` here on purpose: PillMenu renders `item.icon` *in
                // place of* the check column, which would hide the selected
                // state. The pill's own icon already marks chat mode.
                id: chatPath,
                label: t("new_session.chat"),
                sub: t("new_session.chat_sub"),
                checked: isChat,
                onSelect: () => setWorkspace(chatPath),
              },
            ]
          : []),
        ...recentWorkspaces.map((w) => ({
          id: w.path,
          label: w.name,
          sub: w.path,
          checked: w.path === workspace,
          onSelect: () => setWorkspace(w.path),
        })),
      ]}
      footerItems={[
        {
          id: "browse",
          label: t("new_session.browse"),
          icon: <FolderOpen size={13} strokeWidth={1.7} className={pillStyles.menu_icon} />,
          onSelect: browseWorkspace,
        },
      ]}
    />
  );

  const optionPills = (
    <SessionOptionPills
      model={model}
      effort={effort}
      permissionMode={permissionMode}
      tool={tool}
      onToolChange={setTool}
      toolChoices={toolChoices}
      onModelChange={setModel}
      onEffortChange={setEffort}
      onPermissionModeChange={setPermissionMode}
      disabled={submitting}
    />
  );

  return (
    <div className={styles.form}>
      <div className={styles.header}>
        <h3>{t("new_session.title")}</h3>
        <button
          type="button"
          className={styles.close_btn}
          onClick={() => {
            clear();
            onCancel();
          }}
          disabled={submitting}
          aria-label={t("cancel")}
        >
          ×
        </button>
      </div>

      <ChatComposer
        ref={composerRef}
        value={prompt}
        onChange={setPrompt}
        attachments={attachments}
        onAddAttachment={handleAddAttachment}
        onRemoveAttachment={handleRemoveAttachment}
        onAttachmentError={setError}
        onSubmit={handleSubmit}
        submitting={submitting}
        submitDisabled={!workspace.trim() || !prompt.trim()}
        placeholder={t("new_session.prompt_placeholder")}
        disabled={submitting}
        wikiMentions
        contextSlot={workspacePill}
        toolbarSlot={optionPills}
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

      <p className={styles.hint}>
        {isChat ? t("new_session.hint_chat") : t("new_session.hint")}
      </p>

      {error && <div className={styles.error}>{error}</div>}

      {pickingDir && (
        <DirPickerDialog
          initialPath={workspace}
          onPick={(path) => {
            setWorkspace(path);
            setPickingDir(false);
          }}
          onCancel={() => setPickingDir(false)}
        />
      )}
    </div>
  );
}
