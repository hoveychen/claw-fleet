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
import { isWebBuild } from "../hostEnv";
import type { RemoteWorkspace, RemoteWorkspacesConfig } from "../types";
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
  /** Fired when the user backs out of the form without creating a session.
   *  Omitted where the form is the pane's *resting* state rather than something
   *  the user opened — an empty editor group — since there is nothing to back
   *  out to. The close button is then hidden along with it. */
  onCancel?: () => void;
  /** Narrow-host (lite mode) rendering: compact option pills so the toolbar
   *  fits the 340px strip without wrapping to a third line. */
  compact?: boolean;
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
 *  the system uses `/var/folders/.../T` for per-user temp (surfacing as
 *  `/private/var/folders/...` once canonicalized, since `/var`→`/private/var`) —
 *  sessions whose cwd is
 *  one of these are transient and would clutter the launcher's recents. Matches on
 *  a leading path segment so a real project merely *named* `tmp-tools` is kept. */
export function isTempWorkspacePath(path: string): boolean {
  const p = path.replace(/\\/g, "/");
  return (
    p === "/tmp" ||
    p.startsWith("/tmp/") ||
    p === "/private/tmp" ||
    p.startsWith("/private/tmp/") ||
    p.startsWith("/var/folders/") ||
    p.startsWith("/private/var/folders/")
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

/** Distinct workspaces from known sessions. The `limit` most-recently-active are
 *  kept (so a repo used yesterday isn't dropped just because its name sorts late),
 *  then the survivors are returned in alphabetical order by name for a stable,
 *  scannable dropdown. The default selection no longer rides on this order — it
 *  comes from the remembered last-used workspace (see {@link defaultWorkspace}).
 *  In-repo worktree checkouts are collapsed onto their repo root so a repo with
 *  both a main checkout and a live worktree appears once, pointing at the durable
 *  root (see {@link repoRootPath}).
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
  return [...byPath.values()]
    .sort((a, b) => b.lastMs - a.lastMs)
    .slice(0, limit)
    .sort((a, b) => a.name.localeCompare(b.name));
}

/** localStorage key for the repo the user last successfully launched a session
 *  in. Unlike the in-memory composer draft (cleared on submit), this survives so
 *  reopening the new-session form defaults to that repo. */
const LAST_WORKSPACE_KEY = "fleet:last-new-session-workspace";
const LAST_AGENT_TOOL_KEY = "fleet:last-new-session-agent-tool";

function loadLastWorkspace(): string | null {
  try {
    return window.localStorage.getItem(LAST_WORKSPACE_KEY);
  } catch {
    return null;
  }
}

function saveLastWorkspace(path: string): void {
  try {
    window.localStorage.setItem(LAST_WORKSPACE_KEY, path);
  } catch {
    // best-effort — private mode / quota. Defaulting just falls back to the top
    // of the alphabetical list next time.
  }
}

/** Remember the new-session agent independently from the transient composer
 * draft. A successful submit clears that draft, but the next composer should
 * still reopen on the user's last Claude/Codex choice. */
function loadLastAgentTool(): string | null {
  try {
    return window.localStorage.getItem(LAST_AGENT_TOOL_KEY);
  } catch {
    return null;
  }
}

function saveLastAgentTool(tool: string): void {
  try {
    window.localStorage.setItem(LAST_AGENT_TOOL_KEY, tool);
  } catch {
    // Best-effort preference only. If storage is unavailable, the launcher
    // falls back to the first currently available agent.
  }
}

/** Pick the remembered agent when it is still offered; otherwise use the
 * first available choice. During source discovery the choices are Claude-only,
 * which avoids briefly rendering Codex-only model controls before config loads. */
export function defaultAgentTool(
  choices: { value: string; label: string }[],
  lastTool: string | null,
): string {
  if (lastTool && choices.some((choice) => choice.value === lastTool)) {
    return lastTool;
  }
  return choices[0]?.value ?? "claude";
}

/** The workspace to select by default in a fresh new-session form: the remembered
 *  last-used one when it's still a valid option, else the first entry (now
 *  alphabetical), else the chat workspace — a fresh install has no recents at
 *  all, and leaving the pill on its bare "workspace" placeholder only yields a
 *  disabled submit button with nothing saying why. Chat needs no directory, so
 *  it is the one option that is always launchable. Mirrors mobile-web's
 *  `defaultWorkspace`. Returns undefined only when even chat is unavailable. */
export function defaultWorkspace(
  recents: WorkspaceOption[],
  chatPath: string | null | undefined,
  lastWorkspace: string | null,
): string | undefined {
  const valid = new Set(recents.map((w) => w.path));
  if (chatPath) valid.add(chatPath);
  if (lastWorkspace && valid.has(lastWorkspace)) return lastWorkspace;
  return recents[0]?.path ?? chatPath ?? undefined;
}

/** Plain "start a new claude session" form — no project, no task, no queue.
 *  Pick a workspace directory + type the initial prompt; the backend spawns a
 *  detached `claude -p "<prompt>"` and the scanner picks the session up. Rendered
 *  inline inside the History page's detail column (no modal chrome); on success
 *  it hands the spawned pid back so the host can switch that column to the new
 *  session's live SessionDetail. Styled in the composer design language: ghost
 *  pills and custom popovers instead of labeled form rows and native
 *  <select>s. */
export function NewSessionForm({ onCreated, onCancel, compact }: NewSessionFormProps) {
  const { t } = useTranslation();
  const sessions = useSessionsStore((s) => s.sessions);
  const { connection } = useConnectionStore();
  const isRemote = connection?.type === "remote";
  // Tauri's native directory dialog browses the machine the *desktop* runs on.
  // That is the wrong machine under a remote connection (the session spawns on
  // the probe), and in the browser build there is no such dialog at all — a tab
  // can only hand back bytes, never a host path. Both cases go through
  // `DirPickerDialog`, which lists whatever host the backend is bound to.
  const needsBackendDirPicker = isRemote || isWebBuild();
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
  // Draft (prompt / options / attachments / workspace) is lifted into a store
  // keyed "new" so navigating away from the new-session page and back no longer
  // wipes it. Headless `-p` sessions in the CLI's default mode can't approve
  // file edits, so the launcher seeds permissionMode to acceptEdits.
  const { draft, exists: draftExists, patch, clear } = useComposerDraft("new", {
    permissionMode: "acceptEdits",
  });
  const { prompt, model, effort, permissionMode, attachments, workspace } = draft;
  // Empty tool means there is no in-progress override, so restore the durable
  // last choice when it is available. Older drafts remain compatible.
  const tool = draft.tool || defaultAgentTool(toolChoices, loadLastAgentTool());
  const setPrompt = (v: string) => patch({ prompt: v });
  const setModel = (v: string) => patch({ model: v });
  const setEffort = (v: string) => patch({ effort: v });
  const setPermissionMode = (v: string) => patch({ permissionMode: v });
  // `chosenByUser` freezes the seeding effect below — see it for why an
  // auto-seeded chat fallback is provisional and a picked one never is.
  const chosenByUser = useRef(false);
  const setWorkspace = (v: string) => {
    chosenByUser.current = true;
    patch({ workspace: v });
  };
  // Switching tool clears model/effort: Claude and Codex model ids are
  // disjoint, so a leftover Claude model would reach `codex exec -m` (and vice
  // versa) as an invalid value.
  const setTool = (v: string) => {
    saveLastAgentTool(v);
    patch({ tool: v, model: "", effort: "" });
  };
  // If a stale draft (or a since-disabled source) left `tool` pointing at a tool
  // that's no longer offered, snap it back to the first available one. A fresh
  // draft is also materialized after source discovery: if the remembered tool
  // changes from the temporary Claude fallback to Codex, clear any Claude-only
  // model/effort picked while config was loading.
  useEffect(() => {
    if (sources === null) return;
    if (!draft.tool) {
      const remembered = defaultAgentTool(toolChoices, loadLastAgentTool());
      if (remembered === "claude") {
        patch({ tool: remembered });
      } else {
        patch({ tool: remembered, model: "", effort: "" });
      }
      return;
    }
    if (!toolChoices.some((c) => c.value === tool)) {
      setTool(toolChoices[0].value);
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [sources, toolChoices, tool, draft.tool]);
  const [submitting, setSubmitting] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [pathDraft, setPathDraft] = useState("");
  // Which target the backend-driven directory picker is open for, or null when
  // it is closed. It stands in for the native dialog wherever that dialog would
  // browse the wrong machine, and it is now reached from two places — the
  // workspace pill and the "add directory" attachment item — so "open" is no
  // longer enough to know what to do with the pick.
  const [pickingDir, setPickingDir] = useState<"workspace" | "attachment" | null>(null);
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

  // Chat mode is a mode, not a workspace, but the backend still spawns into a
  // directory — so the toggle below writes `chatPath` into the same `workspace`
  // field the picker uses, and this remembers what to put back. Without it,
  // leaving chat mode would land on an empty picker and a disabled submit with
  // nothing on screen saying why.
  //
  // `chatPath` is required, not just compared against: it arrives one backend
  // round-trip after the first render, and a draft that already holds the chat
  // path (form reopened, tab restored) is *not yet* recognisable as chat during
  // that window. Recording it then poisoned this ref with the chat directory,
  // and the off-switch below handed chat straight back to itself — the pill
  // could be turned on but never off again.
  const lastProjectWorkspace = useRef("");
  useEffect(() => {
    if (!chatPath) return;
    if (workspace && workspace !== chatPath) lastProjectWorkspace.current = workspace;
  }, [workspace, chatPath]);

  const setChatMode = (on: boolean) => {
    if (on) {
      if (chatPath) setWorkspace(chatPath);
      return;
    }
    // Never hand the remembered launch target back when it *is* chat — that
    // would leave the toggle off with the chat directory still selected.
    const remembered = loadLastWorkspace();
    setWorkspace(
      lastProjectWorkspace.current ||
        defaultWorkspace(
          recentWorkspaces,
          null,
          remembered === chatPath ? null : remembered,
        ) ||
        "",
    );
  };

  // Registered rca-routed remote workspaces: badge them in the picker, and
  // offer the ones with no sessions yet (they can't appear in recents).
  const [remoteWorkspaces, setRemoteWorkspaces] = useState<RemoteWorkspace[]>([]);
  useEffect(() => {
    // rca re-gated debug-only: skipping the fetch leaves remoteWorkspaces empty
    // so the remote badge / unseen-workspace entries never appear in release
    // builds (import.meta.env.DEV is false in `vite build`).
    if (!import.meta.env.DEV) return;
    invoke<RemoteWorkspacesConfig>("list_remote_workspaces")
      .then((cfg) => setRemoteWorkspaces(cfg.workspaces ?? []))
      .catch(() => {});
  }, []);
  const remotePaths = useMemo(
    () => new Set(remoteWorkspaces.map((w) => w.path)),
    [remoteWorkspaces],
  );
  const unseenRemoteWorkspaces = useMemo(
    () => remoteWorkspaces.filter((w) => !recentWorkspaces.some((r) => r.path === w.path)),
    [remoteWorkspaces, recentWorkspaces],
  );

  // Seed the workspace to the repo the user last launched in (when still
  // available) — else the first recent, else chat. Both inputs land
  // asynchronously: the session list is polled, and `chatPath` costs a backend
  // round-trip. That rules out the mount-only effect this used to be — on a host
  // with no recents (fresh install, or a cloud container that has only ever run
  // chats) mount happens before either is known, and the picker would stay empty
  // for good, leaving submit disabled with nothing saying why. Re-running is safe
  // because of the `chosenByUser` guard: once the user picks anything — a
  // directory, chat mode, or explicitly no directory — no later poll can clobber
  // it.
  useEffect(() => {
    // No draft slot means a fresh (or just-submitted, hence cleared) form —
    // nothing has been picked in it, so seeding owns the field again. The test
    // is the slot's existence, not an empty workspace *value*: leaving chat mode
    // on a host with no other directory legitimately empties the picker, and
    // re-seeding that emptiness put chat straight back — another way the toggle
    // refused to switch off.
    if (!draftExists) chosenByUser.current = false;
    else if (chosenByUser.current) return;
    const seed = defaultWorkspace(recentWorkspaces, chatPath, loadLastWorkspace());
    if (!seed) return;
    if (!draft.workspace) {
      patch({ workspace: seed });
      return;
    }
    // The chat fallback is provisional while the two inputs are still landing:
    // `chatPath` is one backend round-trip while the session list is polled, so
    // it routinely wins the race and seeds chat on a host that has plenty of
    // recents. Left alone that opened the composer in chat mode — with the
    // directory picker hidden — on every launch. Replace it once a real recent
    // shows up; anything the user picked is off limits (`chosenByUser`).
    if (chatPath && draft.workspace === chatPath && seed !== chatPath) {
      patch({ workspace: seed });
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [draft.workspace, recentWorkspaces, chatPath]);

  // The form is remounted each time the user re-enters "new session" mode, so a
  // plain mount effect stands in for the old modal's open-reset.
  useEffect(() => {
    const id = setTimeout(() => composerRef.current?.focus(), 50);
    return () => clearTimeout(id);
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
  // user staring at a form that ignored their file.
  const pickFiles = async () => {
    try {
      const picked = await openDialog({ multiple: true, directory: false });
      if (picked == null) return;
      addPaths(Array.isArray(picked) ? picked : [picked]);
    } catch (e) {
      setError(`${t("composer.attach_failed", "Attachment upload failed")}: ${String(e)}`);
    }
  };

  // A directory cannot be uploaded — and does not need to be: what the agent
  // wants is the path. The native dialog can only offer one on the machine the
  // desktop runs on, so anywhere else (a remote probe, or a browser tab whose
  // host is the backend) it has to be the backend-driven picker instead.
  const pickDirectory = async () => {
    if (needsBackendDirPicker) {
      setPickingDir("attachment");
      return;
    }
    const picked = await openDialog({ multiple: false, directory: true });
    if (typeof picked !== "string") return;
    addAttachmentEntry({ path: picked, name: basename(picked) });
  };

  // Local desktop: the native dialog is the better experience (sidebar,
  // favourites, search) and it browses the right machine. Otherwise it would
  // browse the wrong one — or not exist — so go through the backend instead.
  const browseWorkspace = async () => {
    if (needsBackendDirPicker) {
      setPickingDir("workspace");
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
      // Remember the launched repo so the next new-session form defaults to it
      // (survives the draft clear() below, which resets the in-memory workspace).
      saveLastWorkspace(ws);
      clear();
      onCreated({ pid: resp.pid, sessionId: resp.sessionId, workspacePath: ws });
    } catch (e) {
      setError(String(e));
      setSubmitting(false);
    }
  };

  // Chat mode's own switch, ahead of the workspace pill. Chat used to be one
  // more entry inside that pill's menu, which framed it as a special directory;
  // it is a mode, so it gets its own control and the picker disappears under it
  // (chat has no directory to choose). Hidden when the backend couldn't name the
  // chat workspace, since there would be nothing to switch into.
  const chatModePill = chatPath ? (
    <button
      type="button"
      className={`${pillStyles.ghost_pill} ${compact ? pillStyles.ghost_pill_compact : ""} ${
        isChat ? styles.chat_pill_on : ""
      }`}
      aria-pressed={isChat}
      disabled={submitting}
      onClick={() => setChatMode(!isChat)}
      title={t("new_session.chat_sub")}
      data-testid="chat-mode-pill"
    >
      <MessageCircle size={13} strokeWidth={1.7} />
      <span className={pillStyles.pill_label}>{t("new_session.chat")}</span>
    </button>
  ) : null;

  const workspacePill = (
    <PillMenu
      placement="below"
      icon={<FolderOpen size={13} strokeWidth={1.7} />}
      label={workspace ? basename(workspace) : t("new_session.workspace")}
      title={workspace || t("new_session.workspace_placeholder")}
      disabled={submitting}
      testId="workspace-pill"
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
        ...recentWorkspaces.map((w) => ({
          id: w.path,
          label: w.name,
          sub: remotePaths.has(w.path)
            ? `${w.path} · ${t("new_session.remote_badge")}`
            : w.path,
          checked: w.path === workspace,
          onSelect: () => setWorkspace(w.path),
        })),
        // Remote workspaces with no sessions yet — offer them here or they
        // would be unreachable except by typing the path.
        ...unseenRemoteWorkspaces.map((w) => ({
          id: w.path,
          label: w.label || basename(w.path),
          sub: `${w.path} · ${t("new_session.remote_badge")}`,
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
      compact={compact}
    />
  );

  return (
    <div className={styles.form}>
      <div className={styles.header}>
        <h3>{t("new_session.title")}</h3>
        {onCancel && (
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
        )}
      </div>

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
        submitDisabled={!workspace.trim() || !prompt.trim()}
        placeholder={t("new_session.prompt_placeholder")}
        disabled={submitting}
        wikiMentions
        contextSlot={
          <>
            {chatModePill}
            {!isChat && workspacePill}
          </>
        }
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

      {/* With chat pulled out of the picker, turning the toggle off on a host
          with no recents at all leaves nothing selected — say so, instead of
          leaving a disabled submit button to be puzzled over. */}
      <p className={styles.hint}>
        {isChat
          ? t("new_session.hint_chat")
          : workspace
            ? t("new_session.hint")
            : t("new_session.hint_pick_workspace")}
      </p>

      {error && <div className={styles.error}>{error}</div>}

      {pickingDir && (
        <DirPickerDialog
          initialPath={workspace}
          onPick={(path) => {
            if (pickingDir === "attachment") {
              addAttachmentEntry({ path, name: basename(path) });
            } else {
              setWorkspace(path);
            }
            setPickingDir(null);
          }}
          onCancel={() => setPickingDir(null)}
        />
      )}
    </div>
  );
}
