// New-session sheet + resume composer for the mobile web app. Attachments go
// through the relay's `upload_attachment` (bytes → desktop's user-attachments
// store) and ride the prompt as a `Context files:` list, same as the desktop.

import {
  useCallback,
  useEffect,
  useLayoutEffect,
  useMemo,
  useReducer,
  useRef,
  useState,
} from "react";
import {
  Check,
  FolderSearch,
  LoaderCircle,
  MapPin,
  Plus,
  Send,
  SlidersHorizontal,
  X,
} from "lucide-react";
import { randomId } from "../clientId";
import { loadDraft, saveDraft, type DraftStorage } from "../draft";
import { scopedKey, useDeviceDraft, useDeviceScope } from "../deviceScope";
import { t } from "../i18n";
import { UPLOAD_REQUEST_TIMEOUT_MS, isDesktopRejection, type FleetTransport } from "../transport";
import { waitForSessionId } from "../spawnConfirm";
import { isSessionLive, type SessionInfo } from "../types";
import { useChatWorkspace } from "../useChatWorkspace";
import { useSourcesConfig } from "../useSourcesConfig";
import { toolChoicesForSources, toolForAgentSource } from "../agentSource";
import { dshEffortsFor, dshLadderSpec, dshModelGroups, useDshModels } from "../dshModels";
import { codexProfileChoices, useCodexProfiles } from "../useCodexProfiles";
import {
  effortChoicesFor,
  modelChoicesFor,
  useModelCatalog,
} from "../useModelCatalog";
import { HistoryLayer } from "../useNavStack";
import { basename } from "./taskNotification";
import { timeAgo } from "./TasksView";
import { useFollowTail, useVoiceRecorder } from "../useVoiceRecorder";
import styles from "./Composer.module.css";
import { DirPicker } from "./DirPicker";
import { AttachmentThumbs, type PendingAttachmentUpload } from "./AttachmentThumb";
import { VoiceBar, VoiceMicButton } from "./VoiceBar";

// Model and effort choices were once hardcoded here and manually sync'd with the
// desktop's modelChoices.ts. Both drifted: each claimed Codex efforts were
// `minimal/low/medium/high`, but testing showed no Codex model accepts `minimal`,
// yet all accept `xhigh`/`max`. Now unified via `claw-fleet-core/models.toml` and
// `model_catalog`; see ../useModelCatalog.

const PERMISSION_LABEL: Record<string, string> = {
  acceptEdits: "自动接受编辑",
  plan: "计划模式",
  bypassPermissions: "跳过权限",
};

export function newSessionLocationSummary({
  deviceLabel,
  connected,
  isChat,
  workspaceName,
  workspacePath,
  labels,
}: {
  deviceLabel: string;
  connected: boolean;
  isChat: boolean;
  workspaceName: string;
  workspacePath: string;
  labels?: { online: string; offline: string; chat: string; noProject: string };
}): { title: string; detail: string } {
  const copy = labels ?? {
    online: "在线",
    offline: "离线",
    chat: "纯聊天",
    noProject: "不绑定任何项目目录",
  };
  return {
    title: `${deviceLabel} · ${isChat ? copy.chat : workspaceName}`,
    detail: `${connected ? copy.online : copy.offline} · ${isChat ? copy.noProject : workspacePath}`,
  };
}

export function newSessionConfigSummary({
  toolLabel,
  modelLabel,
  effortLabel,
  permissionLabel,
  labels,
}: {
  toolLabel: string;
  modelLabel: string;
  effortLabel: string;
  permissionLabel: string;
  labels?: { defaultModel: string; defaultEffort: string; defaultPermission: string };
}): { title: string; detail: string } {
  const copy = labels ?? {
    defaultModel: "默认模型",
    defaultEffort: "默认努力度",
    defaultPermission: "按 Agent 默认权限运行",
  };
  return {
    title: `${toolLabel} · ${modelLabel || copy.defaultModel} · ${effortLabel || copy.defaultEffort}`,
    detail: permissionLabel || copy.defaultPermission,
  };
}

/**
 * Resume composer config chips text.
 *
 * Three resident dropdowns (model / thinking intensity / permission) in the
 * resume window are touched maybe never in a year yet occupy 44px permanent
 * height. Collapsed into chips, they report only current value; open-on-tap
 * expands the selector — demoting "changeable any time" to "visible any time,
 * tap once to change", not hiding the feature.
 *
 * Model and effort merge into one chip (always viewed together); permission is
 * separate and Claude-only: codex and dsh have no `--permission-mode` concept.
 */
export function resumeConfigChips({
  tool,
  modelLabel,
  effortLabel,
  permissionLabel,
  labels,
}: {
  tool: string;
  modelLabel: string;
  effortLabel: string;
  permissionLabel: string;
  labels?: { defaultModel: string; defaultPermission: string };
}): string[] {
  const copy = labels ?? { defaultModel: "默认模型", defaultPermission: "沿用权限" };
  const chips = [
    [modelLabel || copy.defaultModel, effortLabel].filter(Boolean).join(" · "),
  ];
  if (tool !== "codex" && tool !== "dsh") chips.push(permissionLabel || copy.defaultPermission);
  return chips;
}

/**
 * Whether a follow-up should override the session's model / effort.
 *
 * One rule only: **send only if the user hand-changed it**. The chip's initial
 * value comes from the snapshot's `session.model`, parsed from the transcript
 * and missing `[1m]`-like spec suffixes (see memory `model-suffix-not-in-jsonl`);
 * sending it as-is means using a degraded spec to override the authoritative
 * launch-spec on the desktop side. If untouched, send no field; let
 * `resume_codex_session` / `claude --resume` pull from launch-spec.
 */
export function resumeConfigOverrides({
  touched,
  model,
  effort,
}: {
  touched: boolean;
  model: string;
  effort: string;
}): { model?: string; effort?: string } {
  if (!touched) return {};
  return {
    ...(model ? { model } : {}),
    ...(effort ? { effort } : {}),
  };
}

/**
 * Composer pill inset from viewport bottom, for transcript to pad its base.
 *
 * Uses layout values only: `offsetHeight` is the element's own layout height,
 * `bottomCss` is parsed px from `getComputedStyle(el).bottom` — both exclude
 * transform.
 *
 * Do not switch back to `getBoundingClientRect()`: rect includes transform.
 * Past code did that with a `translateY` collapse animation; on open, first
 * frame measured inset near 0, then the component stopped re-rendering and that
 * 0 became final — last message rows were buried. Collapse animation is removed,
 * but the measurement discipline must hold: no transform should affect this.
 */
export function composerInset(offsetHeight: number, bottomCss: string): number {
  const inset = Number.parseFloat(bottomCss);
  return Math.round(offsetHeight + (Number.isFinite(inset) ? inset : 0));
}

/** 10 MiB — mirrors MAX_UPLOAD_BYTES on the relay side. */
const MAX_UPLOAD_BYTES = 10 * 1024 * 1024;

export interface Attachment {
  name: string;
  path: string;
}

/** An upload plus the `blob:` URL of the very bytes that were uploaded, when
 *  they were an image. The chip row shows that instead of asking the relay for
 *  a thumbnail of a file this device is literally holding. Kept out of
 *  [`Attachment`] because that one is persisted as a draft, and a `blob:` URL
 *  dies with the page. */
export interface UploadedAttachment extends Attachment {
  previewUrl?: string;
}

/** Push files through the relay's `upload_attachment` (bytes → the desktop's
 *  user-attachments store) and return the persistent paths. Oversize files
 *  are skipped with an alert; a failed upload aborts the rest. */
/** `files` is a `FileList` from an `<input type="file">`, or a plain array when
 *  the files came from somewhere without one — e.g. a share from another app,
 *  whose content:// URIs are fetched into `File`s (see shareTarget.ts). Both
 *  are handled by the `Array.from` below. */
/** Monotonic id for an in-flight upload; the store path it will get does not
 *  exist yet, so it cannot be the key. */
let uploadSeq = 0;

export async function uploadAttachmentFile(
  client: FleetTransport,
  file: File,
  /** Already-made local preview, so the caller can show the picture before the
   *  upload rather than after it. */
  previewUrl?: string,
): Promise<UploadedAttachment | null> {
  if (file.size > MAX_UPLOAD_BYTES) {
    window.alert(t("「{0}」超过 10 MB 上限，已跳过", file.name));
    return null;
  }
  const b64 = await new Promise<string>((resolve, reject) => {
    const reader = new FileReader();
    reader.onload = () => {
      const url = String(reader.result ?? "");
      resolve(url.slice(url.indexOf(",") + 1));
    };
    reader.onerror = () => reject(reader.error);
    reader.readAsDataURL(file);
  });
  const { path } = await client.request<{ path: string }>(
    "upload_attachment",
    { name: file.name, base64: b64 },
    UPLOAD_REQUEST_TIMEOUT_MS,
  );
  return {
    name: file.name,
    path,
    previewUrl:
      previewUrl ?? (file.type.startsWith("image/") ? URL.createObjectURL(file) : undefined),
  };
}

export async function uploadAttachmentFiles(
  client: FleetTransport,
  files: FileList | File[],
): Promise<UploadedAttachment[]> {
  const out: UploadedAttachment[] = [];
  for (const file of Array.from(files)) {
    const one = await uploadAttachmentFile(client, file);
    if (one) out.push(one);
  }
  return out;
}

// draftKey makes the selected-attachment chip list persist with form text —
// after accidental sheet close / session switch, attachments don't need re-picking.
// Stores paths already uploaded to relay; if desktop cleared user-attachments,
// recovered paths go stale but chips can be manually deleted, so no existence check.
function useAttachments(client: FleetTransport | null, draftKey: string) {
  // Device scope: attachments are paths "uploaded to **one** desktop instance";
  // restoring them on another device yields a list of dead paths.
  const [attachments, setAttachments, clearAttachments] = useDeviceDraft<Attachment[]>(
    draftKey,
    [],
  );
  const [uploading, setUploading] = useState(false);
  // Uploads in flight, shown as chips in the same strip as the settled ones.
  // Not persisted with the draft: the bytes only exist in this page's life.
  const [pending, setPending] = useState<PendingAttachmentUpload[]>([]);
  // path → `blob:` URL for files picked in *this* page life. Not state: it is
  // only ever read during a render that `attachments` already triggered, and
  // deliberately not persisted — a restored draft has no bytes here, so those
  // chips fall back to the relay thumbnail.
  const previews = useRef(new Map<string, string>());

  // Paths from draft may have been cleared on the desktop. On mount (when client
  // is ready), validate once and drop stale chips to avoid restored
  // `Context files:` pointing to nonexistent paths. Validation failure (offline
  // etc.) leaves it as-is, no false deletions. Runs only on initial recovery —
  // newly uploaded files exist, no need to re-check.
  const validatedRef = useRef(false);
  useEffect(() => {
    if (validatedRef.current || !client || attachments.length === 0) return;
    validatedRef.current = true;
    void (async () => {
      try {
        const { existing } = await client.request<{ existing: string[] }>("attachments_exist", {
          paths: attachments.map((a) => a.path),
        });
        const keep = new Set(existing);
        setAttachments((prev) => prev.filter((a) => keep.has(a.path)));
      } catch {
        // Keep as-is, avoid false deletions.
      }
    })();
  }, [client, attachments, setAttachments]);

  const addFiles = useCallback(
    async (files: FileList | File[] | null) => {
      if (!client || !files || files.length === 0) return;
      // Bytes travel to desktop over relay — network hop, not local copy — so the
      // chip must exist pre-upload or the strip stays empty for seconds making the
      // pick look ignored. All of this first pass is free: a name and (if image) a
      // blob: URL object.
      const queued = Array.from(files).map((file) => ({
        id: `upload-${++uploadSeq}`,
        file,
        name: file.name,
        previewUrl: file.type.startsWith("image/") ? URL.createObjectURL(file) : undefined,
      }));
      setPending((cur) => [...cur, ...queued]);
      setUploading(true);
      try {
        for (const item of queued) {
          try {
            const a = await uploadAttachmentFile(client, item.file, item.previewUrl);
            if (!a) {
              // Skipped (oversize) — nothing will ever key this preview.
              if (item.previewUrl) URL.revokeObjectURL(item.previewUrl);
              continue;
            }
            const { previewUrl, ...entry } = a;
            setAttachments((prev) => {
              // The blob URL is held aside, never in the persisted draft.
              if (previewUrl) previews.current.set(entry.path, previewUrl);
              return prev.some((x) => x.path === entry.path) ? prev : [...prev, entry];
            });
          } catch (e) {
            // Per-file error handling, not per-batch: one upload rejection no
            // longer cascades to later files.
            if (item.previewUrl) URL.revokeObjectURL(item.previewUrl);
            window.alert(e instanceof Error ? e.message : t("附件上传失败"));
          } finally {
            setPending((cur) => cur.filter((p) => p.id !== item.id));
          }
        }
      } finally {
        setUploading(false);
      }
    },
    [client, setAttachments],
  );

  const remove = useCallback(
    (path: string) => {
      const url = previews.current.get(path);
      if (url) {
        URL.revokeObjectURL(url);
        previews.current.delete(path);
      }
      setAttachments((prev) => prev.filter((a) => a.path !== path));
    },
    [setAttachments],
  );

  return {
    attachments,
    uploading,
    pending,
    addFiles,
    remove,
    reset: clearAttachments,
    previews: previews.current,
  };
}

/**
 * Textarea auto-grows with content.
 *
 * Reset height to "auto" first, then measure scrollHeight — without the reset,
 * scrollHeight never shrinks below current height, so deleting text only grows
 * the box. CSS max-height caps it (contents scroll inside); no hardcoded pixel
 * values here. Both composers (new session and resume) share the same input
 * shape, so this logic is shared.
 */
function useAutoGrow(ref: React.RefObject<HTMLTextAreaElement | null>, text: string) {
  useLayoutEffect(() => {
    const el = ref.current;
    if (!el) return;
    el.style.height = "auto";
    el.style.height = `${el.scrollHeight}px`;
  }, [ref, text]);
}

function withContextFiles(prompt: string, attachments: Attachment[]): string {
  if (attachments.length === 0) return prompt;
  return `${prompt}\n\nContext files:\n${attachments.map((a) => `- ${a.path}`).join("\n")}`;
}

function OptionSelects({
  tool = "claude",
  client,
  model,
  effort,
  permissionMode,
  permissionDefaultLabel,
  onChange,
}: {
  /**
   * The three sources (claude/codex/dsh) have disjoint model/effort ids, and
   * only Claude has the `--permission-mode` concept, so lists and permission
   * pickers fan out by tool.
   */
  tool?: string;
  /**
   * Used to request codex profiles / dsh model catalog from the host (the only
   * source for third-party models). Null shows only builtin models.
   */
  client: FleetTransport | null;
  model: string;
  effort: string;
  permissionMode: string;
  permissionDefaultLabel: string;
  onChange: (patch: { model?: string; effort?: string; permissionMode?: string }) => void;
}) {
  const isCodex = tool === "codex";
  const isDsh = tool === "dsh";
  // Host profile files supplement the codex model list; Claude side unaffected.
  const codexProfiles = useCodexProfiles(isCodex ? client : null);
  // dsh's model catalog is determined by the host's provider config; Fleet
  // hard-codes no entries.
  const dshCatalog = useDshModels(isDsh ? client : null);
  const dshGroups = useMemo(
    () => (isDsh ? dshModelGroups(dshCatalog) : []),
    [isDsh, dshCatalog],
  );
  // Effort ladder follows the model the session actually runs: if explicitly
  // chosen, use it; if model is still "default", use dsh's own default from the
  // catalog — otherwise default model offers only "default" effort.
  const dshEffort = useMemo(
    () =>
      isDsh
        ? dshEffortsFor(dshCatalog, dshLadderSpec(dshCatalog, model))
        : { efforts: [], defaultEffort: "" },
    [isDsh, dshCatalog, model],
  );
  const catalog = useModelCatalog(client);
  const modelChoices = isCodex
    ? [
        ...modelChoicesFor(catalog, "codex", t("默认模型")),
        ...codexProfileChoices(codexProfiles),
      ]
    : modelChoicesFor(catalog, "claude", t("默认模型"));
  // dsh efforts are **per-model** — Claude's fixed ladder doesn't apply to it.
  // When catalog hasn't arrived or the model has no reasoning controls, only
  // "default" remains — an honest degradation: the session runs on the host's
  // effort chosen in ~/.dsh/settings.yaml.
  const effortChoices: Array<[string, string]> = isDsh
    ? [
        [
          "",
          dshEffort.defaultEffort ? t("默认（{0}）", dshEffort.defaultEffort) : "默认努力度",
        ],
        ...dshEffort.efforts,
      ]
    : isCodex
      ? effortChoicesFor(catalog, "codex", model, t("默认努力度"))
      : effortChoicesFor(catalog, "claude", model, t("默认努力度"));
  return (
    <div className={styles.optionRow}>
      <label className={styles.optionField}>
        <span>{t("模型")}</span>
        <select
          className={styles.optionSelect}
          value={model}
          aria-label={t("模型")}
          onChange={(e) => {
            const nextModel = e.target.value;
            const supportedEfforts = effortChoicesFor(
              catalog,
              "codex",
              nextModel,
              "",
            ).map(([value]) => value);
            onChange({
              model: nextModel,
              ...(isCodex && !supportedEfforts.includes(effort) ? { effort: "" } : {}),
            });
          }}
        >
          {isDsh ? (
            <>
              <option value="">{t("默认模型")}</option>
              {dshGroups.map((g) => (
                <optgroup key={g.label} label={g.label}>
                  {g.models.map(([v, label]) => (
                    <option key={v} value={v}>
                      {label}
                    </option>
                  ))}
                </optgroup>
              ))}
            </>
          ) : (
            modelChoices.map(([v, label]) => (
              <option key={v} value={v}>
                {t(label)}
              </option>
            ))
          )}
        </select>
      </label>
      <label className={styles.optionField}>
        <span>{t("思考强度")}</span>
        <select
          className={styles.optionSelect}
          value={effort}
          aria-label={t("思考强度")}
          onChange={(e) => onChange({ effort: e.target.value })}
        >
          {effortChoices.map(([v, label]) => (
            <option key={v} value={v}>
              {t(label)}
            </option>
          ))}
        </select>
      </label>
      {!isCodex && !isDsh && (
        <label className={styles.optionField}>
          <span>{t("权限")}</span>
          <select
            className={styles.optionSelect}
            value={permissionMode}
            aria-label={t("权限")}
            onChange={(e) => onChange({ permissionMode: e.target.value })}
          >
            <option value="">{t(permissionDefaultLabel)}</option>
            {Object.entries(PERMISSION_LABEL).map(([v, label]) => (
              <option key={v} value={v}>
                {t(label)}
              </option>
            ))}
          </select>
        </label>
      )}
    </div>
  );
}

// ── New session sheet ─────────────────────────────────────────────────────────

interface NewSessionProps {
  sessions: SessionInfo[];
  client: FleetTransport | null;
  /**
   * Target device list. With one device, used only for summary display labels;
   * no picker is rendered. Read only id and label; don't write secrets into
   * React keys or DOM.
   */
  devices?: readonly { id: string; label: string }[];
  /**
   * Which device to launch on this time (an id from `devices`).
   */
  targetDeviceId?: string;
  /**
   * Switch target device. App receives it, swaps provider, and re-mounts this
   * component by new id.
   */
  onTargetDevice?: (id: string) => void;
  /**
   * Files shared in from another app (see shareTarget.ts). Attachment state
   * lives in this component, so App just passes File objects; this component
   * uploads via the normal path once client is ready.
   */
  initialFiles?: File[];
  /**
   * Is relay connected yet. Non-null `client` only means the object exists; the
   * connection might still be handshaking — shared files arrive at cold start,
   * when upload would immediately hit "relay not connected yet".
   */
  relayReady?: boolean;
  onClose: () => void;
}

/**
 * Unsaved draft key for the new-session form (device-prefixed at persist time;
 * see deviceScope.tsx). Only one new-session sheet per device at a time;
 * accidental close/tab switch/iOS PWA kill restores it identically; creates only
 * when successful. Attachments skip the draft — they're already-uploaded relay
 * outputs, re-picking is fine on re-open.
 */
export const NEW_SESSION_DRAFT_KEY = "new-session";
const NEW_SESSION_ATTACH_KEY = "new-session:attachments";

/**
 * Collapse worktree checkouts back to repo root. Fleet develops plans in
 * `<repo-root>/.worktrees/<task-id>` (temporary, removed post-merge); the
 * launcher should return the persistent repo root, never the task-id leaf. Paths
 * lacking `.worktrees` segment (including unrelated `~/.fleet/worktrees/`, whose
 * segment name is `worktrees`) pass through unchanged. Mirrors desktop
 * NewSessionForm.repoRootPath.
 */
export function repoRootPath(path: string): string {
  const normalized = path.replace(/\\/g, "/");
  const idx = normalized.split("/").indexOf(".worktrees");
  if (idx <= 0) return path;
  const before = normalized.split("/").slice(0, idx).join("/");
  return before || path;
}

/**
 * True if workspace path falls in OS temp/scratch directories — these should
 * never be launchable workspaces. Fleet (like Claude Code) puts per-session
 * scratch in `/tmp` (on macOS `/tmp` symlinks to `/private/tmp`); the system
 * uses `/var/folders/.../T` for per-user temp (normalized to `/private/var/folders/...`
 * because `/var` → `/private/var`). Sessions with cwd in either are temporary
 * and pollute the launcher's recent list. Matches by leading path segment, so a
 * project truly named `tmp-tools` is preserved. Mirrors desktop
 * NewSessionForm.isTempWorkspacePath.
 */
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

/**
 * Most recently used workspaces (`[path, name]`). **Two-stage sort** (mirrors
 * desktop NewSessionForm.distinctWorkspaces): first, take the most recent
 * `limit` items by last-activity timestamp descending (yesterday's repo doesn't
 * drop just for low alphabetical rank), then sort survivors alphabetically,
 * yielding a stable, scannable list. Worktree checkouts collapse to repo root
 * ({@link repoRootPath}) for dedup; drops temp directories ({@link isTempWorkspacePath})
 * and chat-only path (nailed separately as the first option). Default selection
 * **does not** depend on this order — it comes from the remembered "last repo
 * used to successfully create a session" ({@link defaultWorkspace}).
 */
/**
 * One workspace row shown in the new session sheet's main area.
 *
 * Beyond the name, includes "last active" and "session count running": both
 * exist in the sessions snapshot but were lost by recentWorkspaces on return.
 * When picking a project, these two facts are what you actually want to know —
 * which is most active, which already has sessions running.
 */
export interface WorkspaceRow {
  path: string;
  name: string;
  lastMs: number;
  running: number;
}

export function recentWorkspaceRows(
  sessions: SessionInfo[],
  chatPath: string | null,
  limit = 30,
): WorkspaceRow[] {
  const byPath = new Map<string, { name: string; lastMs: number; running: number }>();
  for (const s of sessions) {
    if (!s.workspacePath) continue;
    const path = repoRootPath(s.workspacePath);
    if (isTempWorkspacePath(path)) continue;
    if (path === chatPath) continue;
    const prev = byPath.get(path);
    const running = (prev?.running ?? 0) + (isSessionLive(s) ? 1 : 0);
    // For the same path, keep the name and timestamp from the most recently active session.
    if (!prev || s.lastActivityMs > prev.lastMs) {
      byPath.set(path, {
        name: s.workspaceName || basename(path),
        lastMs: s.lastActivityMs,
        running,
      });
    } else {
      prev.running = running;
    }
  }
  return [...byPath.entries()]
    .sort((a, b) => b[1].lastMs - a[1].lastMs)
    .slice(0, limit)
    .sort((a, b) => a[1].name.localeCompare(b[1].name))
    .map(([path, v]) => ({ path, ...v }));
}

export function recentWorkspaces(
  sessions: SessionInfo[],
  chatPath: string | null,
  limit = 30,
): [string, string][] {
  return recentWorkspaceRows(sessions, chatPath, limit).map((r) => [r.path, r.name]);
}

/**
 * localStorage key (prefixed `fleet-draft:` from draft.ts, then per-device namespaced),
 * remembers the repo used to last successfully create a session — repo paths are
 * per-machine so must be device-scoped. Independent of the new-session draft key,
 * so clearDraft() at submit doesn't erase it.
 */
const LAST_WORKSPACE_KEY = "last-new-session-workspace";

/**
 * Default workspace for new session: if user chose one this session and it's
 * still valid (draftWorkspace), reuse it; otherwise prefer the last-used repo
 * (lastWorkspace) — if stale, fall back to the list's first item, then to the
 * chat-only path.
 */
export function defaultWorkspace(
  draftWorkspace: string,
  recents: [string, string][],
  chatPath: string | null,
  lastWorkspace: string,
): string {
  const valid = new Set(recents.map((r) => r[0]));
  if (chatPath) valid.add(chatPath);
  if (draftWorkspace === "__custom__" || valid.has(draftWorkspace)) return draftWorkspace;
  if (valid.has(lastWorkspace)) return lastWorkspace;
  return recents[0]?.[0] ?? chatPath ?? "";
}

const NEW_SESSION_DEFAULT = {
  workspace: "",
  customWorkspace: "",
  prompt: "",
  // Which agent tool to launch: "claude" (default), "codex", or "dsh". Routed
  // by relay's spawn_session → agent_source::spawn_session.
  tool: "claude",
  model: "",
  effort: "",
  // acceptEdits by default: headless -p sessions in default mode can't approve
  // file edits (same default as desktop launcher). Ignored for Codex / dsh.
  permissionMode: "acceptEdits",
};

/**
 * When switching the new session's target device, carry this prompt into the
 * **target device**'s draft.
 *
 * Why only the prompt: all other form fields are "machine-specific" — workspace
 * is a path on device A, model/effort may be A's codex profile, attachments are
 * paths uploaded to A. After switching to B, App re-mounts this component by new
 * id (see App.tsx `key`), so those three restore themselves from B's namespace.
 * Not carrying them is what we want.
 *
 * Prompt is different: it's the text the user just typed, machine-independent,
 * and re-mount shouldn't lose it. The cost: overwriting any draft text on the
 * target device — the text in hand takes priority.
 */
export function carryPromptToDevice(
  nextDeviceId: string,
  prompt: string,
  store?: DraftStorage | null,
): void {
  const key = scopedKey(nextDeviceId, NEW_SESSION_DRAFT_KEY);
  saveDraft(key, { ...loadDraft(key, NEW_SESSION_DEFAULT, store), prompt }, store);
}

export function NewSessionSheet({
  sessions,
  client,
  devices,
  targetDeviceId,
  onTargetDevice,
  initialFiles,
  relayReady,
  onClose,
}: NewSessionProps) {
  // Chat-only workspace: not project-bound, no "recent sessions" to discover; must
  // be explicitly nailed as the first option.
  const chatPath = useChatWorkspace(client);

  const recentRows = recentWorkspaceRows(sessions, chatPath);
  const recents = recentRows.map((r): [string, string] => [r.path, r.name]);
  // For grace-period confirmation after timeout to read fresh snapshot (prop
  // updates on every snapshot push).
  const sessionsRef = useRef(sessions);
  sessionsRef.current = sessions;
  // New-session draft is device-scoped: workspace path and model are
  // machine-specific things.
  const [draft, setDraft, clearDraft] = useDeviceDraft(
    NEW_SESSION_DRAFT_KEY,
    NEW_SESSION_DEFAULT,
  );
  const deviceId = useDeviceScope();
  const patch = (p: Partial<typeof NEW_SESSION_DEFAULT>) => setDraft((d) => ({ ...d, ...p }));
  // Voice writes back via functional update: recognition results arrive async,
  // and the user might have typed more text meanwhile, and a closure-captured
  // prompt would clobber those new characters. The `submit` in onSend is a
  // const declared below — its arrow function body evaluates only on tap, when
  // it's already defined; the hook also tracks the latest version via ref, so
  // "stop and send" flows the final text after that point, not the stale closure
  // from tap time.
  const voice = useVoiceRecorder({
    value: draft.prompt,
    onChange: (next) => setDraft((d) => ({ ...d, prompt: next })),
    onSend: () => void submit(),
  });
  const voiceTailRef = useFollowTail<HTMLTextAreaElement>(voice.showingPreview, voice.preview);
  useAutoGrow(voiceTailRef, voice.showingPreview ? voice.preview : draft.prompt);
  const newFileRef = useRef<HTMLInputElement>(null);
  const [busy, setBusy] = useState(false);
  const [created, setCreated] = useState(false);
  const closeTimerRef = useRef<number | null>(null);
  const [picking, setPicking] = useState(false);
  const [picker, setPicker] = useState<"location" | "config" | null>(null);
  useEffect(
    () => () => {
      if (closeTimerRef.current !== null) window.clearTimeout(closeTimerRef.current);
    },
    [],
  );
  const { attachments, uploading, pending, addFiles, remove, reset, previews } = useAttachments(
    client,
    NEW_SESSION_ATTACH_KEY,
  );
  // Shared files go through normal upload once.
  //
  // Must wait for `relayReady`, not just non-null `client`: the client object
  // exists before connection is live, and request() would throw "relay not
  // connected yet". Shares almost always arrive at cold start, hitting right in
  // the handshake window — real logs show "upload FAILED: relay not connected
  // yet", and after one failure the files never get attention again. The ref
  // guarantees post-connect upload happens once only, no re-upload on reconnect.
  const sharedUploadedRef = useRef(false);
  useEffect(() => {
    if (sharedUploadedRef.current || !client || !relayReady || !initialFiles?.length) return;
    sharedUploadedRef.current = true;
    void addFiles(initialFiles);
  }, [client, relayReady, initialFiles, addFiles]);

  const { customWorkspace, prompt, model, effort, permissionMode } = draft;
  // Older persisted drafts predate the tool field → default to Claude.
  const tool = draft.tool || "claude";
  // Only Claude has the --permission-mode concept.
  const sendsPermissionMode = tool === "claude";
  // The three sources' model/effort ids are disjoint, so switching tools clears
  // them — leftover Claude models would otherwise enter `codex exec -m` (and
  // vice versa). Mirrors the desktop NewSessionForm.
  const setTool = (v: string) => patch({ tool: v, model: "", effort: "" });

  // Only offer the agent tools whose source is actually being monitored (source
  // enabled AND CLI installed on the desktop host). Mirrors the desktop
  // NewSessionForm — Codex must not appear when its source is off; selecting it
  // would only fail at spawn. `null` = config not loaded yet → Claude-only so we
  // never flash Codex then hide it.
  const sources = useSourcesConfig(client);
  const toolChoices = useMemo(() => toolChoicesForSources(sources), [sources]);
  // A stale draft (or a since-disabled source) may leave `tool` pointing at a
  // tool that's no longer offered — snap it back to the first available one.
  useEffect(() => {
    if (sources === null) return;
    if (!toolChoices.some(([v]) => v === tool)) {
      setTool(toolChoices[0][0]);
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [sources, toolChoices, tool]);

  // Default-select the "last repo used to successfully create a session"
  // (independently persisted, doesn't clear with draft); if stale, fall back to
  // first list item to avoid blank <select>. If user picked something this
  // session and it's still valid, reuse their choice.
  const workspace = defaultWorkspace(
    draft.workspace,
    recents,
    chatPath,
    loadDraft(scopedKey(deviceId, LAST_WORKSPACE_KEY), ""),
  );

  const switchDevice = (nextId: string) => {
    if (!onTargetDevice || nextId === targetDeviceId) return;
    carryPromptToDevice(nextId, prompt);
    onTargetDevice(nextId);
  };

  const isChat = Boolean(chatPath) && workspace === chatPath;
  const effectiveWorkspace = workspace === "__custom__" ? customWorkspace.trim() : workspace;
  const canSubmit = Boolean(
    client && effectiveWorkspace && prompt.trim() && !busy && !created && !uploading,
  );

  const deviceLabel =
    devices?.find((device) => device.id === targetDeviceId)?.label ?? t("当前设备");
  const workspaceName =
    workspace === "__custom__"
      ? basename(effectiveWorkspace) || t("自定义路径")
      : recents.find(([path]) => path === workspace)?.[1] || basename(effectiveWorkspace);
  const locationSummary = newSessionLocationSummary({
    deviceLabel,
    connected: relayReady !== false,
    isChat,
    workspaceName,
    workspacePath: effectiveWorkspace,
    labels: {
      online: t("在线"),
      offline: t("离线"),
      chat: t("纯聊天"),
      noProject: t("不绑定任何项目目录"),
    },
  });
  const toolLabel = t(toolChoices.find(([value]) => value === tool)?.[1] ?? tool);
  const sheetCatalog = useModelCatalog(client);
  const modelLabel = model
    ? (modelChoicesFor(sheetCatalog, tool === "codex" ? "codex" : "claude", "").find(
        ([value]) => value === model,
      )?.[1] ?? model)
    : "";
  const configSummary = newSessionConfigSummary({
    toolLabel,
    modelLabel,
    effortLabel: effort,
    permissionLabel: sendsPermissionMode ? t(PERMISSION_LABEL[permissionMode] ?? "默认权限") : "",
    labels: {
      defaultModel: t("默认模型"),
      defaultEffort: t("默认努力度"),
      defaultPermission: t("按 Agent 默认权限运行"),
    },
  });

  const submit = async () => {
    if (!client || !canSubmit) return;
    // Phone pre-allocates session_id: desktop uses it as `claude --session-id`,
    // so even if the reply frame is lost, it can recognize the session in later
    // snapshots; and desktop dedupes by this id (idempotent), so retrying the
    // same request on timeout doesn't double-launch (plan C).
    const sessionId = crypto.randomUUID();
    const params = {
      workspacePath: effectiveWorkspace,
      prompt: withContextFiles(prompt.trim(), attachments),
      sessionId,
      tool,
      ...(model ? { model } : {}),
      ...(effort ? { effort } : {}),
      // Codex / dsh have no --permission-mode equivalent; only send to Claude.
      ...(sendsPermissionMode && permissionMode ? { permissionMode } : {}),
    };
    setBusy(true);
    // On confirmation (ack / reply / snapshot), optimistically conclude once;
    // `settled` prevents repeat.
    let settled = false;
    const succeed = () => {
      if (settled) return;
      settled = true;
      // Remember this repo; next time the new-session sheet opens, default-select
      // it (independent key, unaffected by clearDraft).
      saveDraft(scopedKey(deviceId, LAST_WORKSPACE_KEY), effectiveWorkspace);
      setCreated(true);
      // Once ack arrives, clear sent draft; even if the system back key closes
      // the page within the 650ms success state, next time won't recover the
      // already-launched task. Brief stay is only for success feedback.
      clearDraft();
      reset();
      closeTimerRef.current = window.setTimeout(() => {
        onClose();
      }, 650);
    };
    // Plan A: on early ack from desktop, close optimistically — submit reached
    // desktop, no need to wait for reply.
    const send = () => client.request("spawn_session", params, undefined, succeed);
    try {
      await send();
      succeed(); // reply arrival also succeeds, idempotent with onAck
    } catch (e) {
      // Desktop explicit rejection (path doesn't exist, prompt empty, etc.): it
      // received, judged, said no. Session can't appear in any snapshot; error
      // directly, no retry, no grace period.
      if (isDesktopRejection(e)) {
        window.alert(e.message);
        return; // finally clears busy
      }
      if (settled) return; // Already closed via ack; timeout reject is fine
      // Plan C: timeout without ack — submit may never have reached desktop
      // (relay does best-effort, no queuing/re-send). Retry the same request
      // once; desktop dedupes by sessionId (idempotent), won't double-launch.
      try {
        await send();
        succeed();
        return;
      } catch (e2) {
        if (isDesktopRejection(e2)) {
          window.alert(e2.message);
          return;
        }
        if (settled) return;
        // Last fallback: desktop may have spawned but both ack and reply were
        // lost. Enter grace period watching snapshots; if same id appears, call
        // it success; if not, error.
        const confirmed = await waitForSessionId(sessionId, () => sessionsRef.current);
        if (confirmed) succeed();
        else window.alert(e2 instanceof Error ? e2.message : t("创建会话失败"));
      }
    } finally {
      setBusy(false);
    }
  };

  return (
    <div className={styles.sheetBackdrop}>
      <div className={styles.sheet} role="dialog" aria-label={t("新会话")}>
        <div className={styles.sheetHead}>
          <button
            className={styles.sheetClose}
            onClick={onClose}
            aria-label={t("关闭")}
            disabled={created}
          >
            <X size={21} />
          </button>
          <span className={styles.sheetTitle}>{t("新会话")}</span>
          <span
            className={styles.connectionDot}
            data-online={relayReady !== false}
            aria-label={relayReady === false ? t("离线") : t("在线")}
          />
        </div>

        {/* Main area for "where have you been". Previously this was three 64px
            summary rows + one 190px input card: a full 844px screen held only
            three items, and launching a session took three taps. With location
            and config pushed to the bottom pill row, this space now has room for
            content. */}
        <div className={styles.sheetBody}>
          <span className={styles.sectionLabel}>{t("最近")}</span>
          <div className={styles.recentList}>
            {recentRows.map((row) => (
              <button
                key={row.path}
                className={styles.recentRow}
                data-active={workspace === row.path || undefined}
                onClick={() => patch({ workspace: row.path })}
              >
                <span className={styles.recentName}>{row.name}</span>
                <span className={styles.recentMeta}>
                  {row.running > 0 && (
                    <span className={styles.recentRunning}>
                      <span className={styles.recentDot} />
                      {t("{0} 个在跑", row.running)}
                    </span>
                  )}
                  {timeAgo(row.lastMs)}
                </span>
                {workspace === row.path && <Check size={17} className={styles.recentCheck} />}
              </button>
            ))}
            {chatPath && (
              <button
                className={styles.recentRow}
                data-active={isChat || undefined}
                onClick={() => patch({ workspace: chatPath })}
              >
                <span className={styles.recentName}>{t("纯聊天")}</span>
                <span className={styles.recentMeta}>{t("不绑定任何项目目录")}</span>
                {isChat && <Check size={17} className={styles.recentCheck} />}
              </button>
            )}
            <button className={styles.recentRow} onClick={() => setPicker("location")}>
              <span className={styles.recentName}>
                <FolderSearch size={15} />
                {t("选目录…")}
              </span>
            </button>
          </div>
          {sendsPermissionMode && permissionMode === "bypassPermissions" && (
            <span className={styles.permissionHint} data-danger="true">
              {t("高风险：Agent 将不再请求命令或文件操作确认")}
            </span>
          )}
        </div>

        {/* Bottom mirrors the resume composer's pill shape: config chip row +
            attachments + input pill. "Launch session" is no longer a 50px large
            button but a circular send on the pill's right edge — both input
            areas now look the same, users don't have to learn twice. */}
        <div className={styles.sheetFooter}>
          <div className={styles.resumeChips}>
            <button className={styles.resumeChip} onClick={() => setPicker("location")}>
              <MapPin size={13} />
              {locationSummary.title}
            </button>
            <button className={styles.resumeChip} onClick={() => setPicker("config")}>
              <SlidersHorizontal size={13} />
              {configSummary.title}
            </button>
          </div>
          {(attachments.length > 0 || pending.length > 0) && !voice.active && (
            <div className={styles.resumeThumbs}>
              <AttachmentThumbs
                paths={attachments.map((a) => a.path)}
                pending={pending}
                client={client}
                previews={previews}
                onRemove={remove}
                compact
              />
            </div>
          )}
          {voice.active ? (
            <VoiceBar rec={voice} />
          ) : (
            <div className={styles.pill}>
              <button
                className={styles.pillBtn}
                disabled={uploading}
                onClick={() => newFileRef.current?.click()}
                aria-label={uploading ? t("上传中…") : t("附件")}
              >
                {uploading ? (
                  <LoaderCircle size={19} className={styles.spin} />
                ) : (
                  <Plus size={20} />
                )}
              </button>
              <textarea
                ref={voiceTailRef}
                className={styles.composerInput}
                aria-label={t("第一条指令")}
                placeholder={t("要让 agent 做什么？")}
                rows={1}
                value={voice.showingPreview ? voice.preview : prompt}
                readOnly={voice.showingPreview}
                onChange={(e) => patch({ prompt: e.target.value })}
              />
              {voice.available && !prompt.trim() && (
                <span className={styles.pillMic}>
                  <VoiceMicButton rec={voice} />
                </span>
              )}
              <button
                className={styles.sendBtn}
                data-success={created || undefined}
                disabled={!canSubmit}
                onClick={() => void submit()}
                aria-label={created ? t("已启动") : busy ? t("创建中…") : t("启动会话")}
              >
                {created ? (
                  <Check size={17} />
                ) : busy ? (
                  <LoaderCircle size={17} className={styles.spin} />
                ) : (
                  <Send size={16} />
                )}
              </button>
              <input
                ref={newFileRef}
                type="file"
                multiple
                hidden
                onChange={(e) => {
                  void addFiles(e.target.files);
                  e.target.value = "";
                }}
              />
            </div>
          )}
          <span className={styles.submitHint} aria-live="polite">
            {created
              ? t("目标设备已确认收到")
              : busy
                ? t("正在发往 {0}…", deviceLabel)
                : !prompt.trim()
                  ? t("输入任务后即可启动")
                  : locationSummary.detail}
          </span>
        </div>

        {picker && (
          <>
            <HistoryLayer onBack={() => setPicker(null)} />
            <div className={styles.pickerBackdrop} onClick={() => setPicker(null)} />
            <div
              className={styles.pickerSheet}
              role="dialog"
              aria-modal="true"
              aria-label={picker === "location" ? t("运行位置") : t("运行配置")}
            >
              <span className={styles.pickerGrabber} />
              <div className={styles.pickerHead}>
                <span />
                <strong>{picker === "location" ? t("运行位置") : t("运行配置")}</strong>
                <button onClick={() => setPicker(null)}>{t("完成")}</button>
              </div>
              <div className={styles.pickerBody}>
                {picker === "location" ? (
                  <>
                    {chatPath && (
                      <div
                        className={styles.modeSwitch}
                        role="group"
                        aria-label={t("会话类型")}
                      >
                        <button
                          data-active={!isChat}
                          onClick={() =>
                            patch({ workspace: recents[0]?.[0] ?? "__custom__" })
                          }
                        >
                          {t("项目")}
                        </button>
                        <button
                          data-active={isChat}
                          onClick={() => patch({ workspace: chatPath })}
                        >
                          {t("纯聊天")}
                        </button>
                      </div>
                    )}
                    {devices && devices.length > 1 && (
                      <label className={styles.pickerField}>
                        <span>{t("设备")}</span>
                        <select
                          className={styles.deviceSelect}
                          value={targetDeviceId ?? ""}
                          aria-label={t("开在哪台设备上")}
                          onChange={(e) => switchDevice(e.target.value)}
                        >
                          {devices.map((device) => (
                            <option key={device.id} value={device.id}>{device.label}</option>
                          ))}
                        </select>
                      </label>
                    )}
                    {!isChat && (
                      <label className={styles.pickerField}>
                        <span>{t("项目目录")}</span>
                        <select
                          className={styles.workspaceSelect}
                          value={workspace}
                          aria-label={t("选择工作目录")}
                          onChange={(e) => patch({ workspace: e.target.value })}
                        >
                          {recents.map(([path, name]) => (
                            <option key={path} value={path}>{name} — {path}</option>
                          ))}
                          <option value="__custom__">{t("自定义路径…")}</option>
                        </select>
                      </label>
                    )}
                    {workspace === "__custom__" && !isChat && (
                      <div className={styles.customPathRow}>
                        <input
                          className={styles.customPath}
                          aria-label={t("自定义路径")}
                          placeholder={t("~/workspace/项目 或点右侧浏览")}
                          value={customWorkspace}
                          onChange={(e) => patch({ customWorkspace: e.target.value })}
                        />
                        <button
                          className={styles.browseBtn}
                          onClick={() => setPicking(true)}
                          disabled={!client}
                        >
                          <FolderSearch size={17} />
                          {t("浏览…")}
                        </button>
                      </div>
                    )}
                    {devices && devices.length > 1 && relayReady === false && (
                      <span className={styles.deviceOffline}>
                        {t("这台设备当前离线，创建请求可能要等它连上才生效")}
                      </span>
                    )}
                  </>
                ) : (
                  <>
                    {toolChoices.length > 1 && (
                      <label className={styles.pickerField}>
                        <span>{t("Agent")}</span>
                        <select
                          className={styles.optionSelect}
                          value={tool}
                          aria-label={t("Agent")}
                          onChange={(e) => setTool(e.target.value)}
                        >
                          {toolChoices.map(([value, label]) => (
                            <option key={value} value={value}>{t(label)}</option>
                          ))}
                        </select>
                      </label>
                    )}
                    <OptionSelects
                      tool={tool}
                      client={client}
                      model={model}
                      effort={effort}
                      permissionMode={permissionMode}
                      permissionDefaultLabel="默认权限"
                      onChange={(next) => patch(next)}
                    />
                  </>
                )}
              </div>
            </div>
          </>
        )}

        {picking && (
          <>
            <HistoryLayer onBack={() => setPicking(false)} />
            <DirPicker
              client={client}
              initialPath={customWorkspace.trim()}
              onPick={(path) => {
                patch({ customWorkspace: path });
                setPicking(false);
              }}
              onClose={() => setPicking(false)}
            />
          </>
        )}
      </div>
    </div>
  );
}

// ── Resume session composer ────────────────────────────────────────────────────

interface ResumeProps {
  session: SessionInfo;
  client: FleetTransport | null;
  /** `"resume"`: turn ended, submit resumes now. `"enqueue"`: turn still
   *  running, submit queues the message for delivery when the turn ends. */
  mode?: "resume" | "enqueue";
  /** Fired (resume mode only) with the final text the moment the desktop
   *  accepts the follow-up, so the parent can echo it as a user bubble while
   *  `claude --resume` cold-starts — instead of leaving the transcript blank
   *  for the seconds before the real row lands via `tail`. */
  onOptimisticSend?: (text: string) => void;
  /** Toggled true while a submit is in flight, so the parent can pause its
   *  tail / live-thinking pollers and yield the single serialized WS to the
   *  resume req/reply instead of contending with a big tail response. */
  onSubmitInFlight?: (inFlight: boolean) => void;
  /** Height currently blocked by this component. It floats above the transcript
   *  without taking layout height; the parent uses this to pad the scroll area's
   *  bottom so the last message isn't buried under the pill. */
  onHeight?: (px: number) => void;
}

export function ResumeComposer({
  session,
  client,
  mode = "resume",
  onOptimisticSend,
  onSubmitInFlight,
  onHeight,
}: ResumeProps) {
  const enqueueing = mode === "enqueue";
  // The session's source determines which model/effort list to use — unrecognized
  // sources fall back to Claude, which is the registry's own default. Before dsh
  // was added, this was a hardcoded Codex ternary, silently using Claude models
  // for dsh sessions.
  const tool = toolForAgentSource(session.agentSource);
  const pendingMessages = session.pendingMessages ?? [];
  // Per-session resume draft, keyed by sessionId — switching to another session
  // and back keeps the draft intact; cleared on successful send. Device-scoped:
  // session IDs are unique per machine only, so sessions with the same ID on
  // different machines will share a draft.
  const [prompt, setPrompt, clearPrompt] = useDeviceDraft(`resume:${session.id}`, "");
  // Resume model/effort start from the session's current values, not empty
  // strings — empty would display as "default" in the pill, obscuring which
  // model this follow-up will actually run on.
  const [model, setModel] = useState(session.model ?? "");
  const [effort, setEffort] = useState(session.effort ?? "");
  // Whether the user manually changed the selection. **Only send model/effort
  // if changed**: when untouched, leave empty so the desktop pulls the
  // authoritative value from launch-spec (it includes `[1m]` suffixes that the
  // snapshot's `session.model` lacks, parsed from transcript). This prevents an
  // unchanged follow-up from pinning the session to a degraded model spec.
  const [configTouched, setConfigTouched] = useState(false);
  const [permissionMode, setPermissionMode] = useState("");
  // Switching to another session: reset to that session's current config and
  // clear the "touched" flag.
  useEffect(() => {
    setModel(session.model ?? "");
    setEffort(session.effort ?? "");
    setConfigTouched(false);
  }, [session.id]);
  const [busy, setBusy] = useState(false);
  const [sent, setSent] = useState(false);
  // Optimistically hide a cancelled chip until the next sessions snapshot drops
  // it for real; keyed by index+content so a stale key never hides the wrong row
  // after the list re-indexes.
  const [cancelledKeys, setCancelledKeys] = useState<Set<string>>(new Set());
  // A fresh snapshot is authoritative, so drop the optimistic hides: without
  // this the set grows forever and a *new* follow-up that happens to land on
  // the same index with the same text would be hidden permanently.
  const pendingKey = pendingMessages.join(" ");
  useEffect(() => {
    setCancelledKeys((prev) => (prev.size === 0 ? prev : new Set()));
  }, [pendingKey]);
  const { attachments, uploading, pending, addFiles, remove, reset, previews } = useAttachments(
    client,
    `resume:${session.id}:attachments`,
  );
  const [pickerOpen, setPickerOpen] = useState(false);
  const fileRef = useRef<HTMLInputElement>(null);
  // Current config shown in the pill. dsh's model catalog comes from the host at runtime; if we
  // don't recognize an id, display it as-is — a real but unfamiliar id beats a wrong friendly name.
  const resumeCatalog = useModelCatalog(client);
  const modelLabel = useMemo(() => {
    if (tool === "dsh") return model;
    const table = modelChoicesFor(resumeCatalog, tool === "codex" ? "codex" : "claude", "");
    const hit = table.find(([v]) => v === model);
    return hit ? hit[1] : model;
  }, [tool, model, resumeCatalog]);
  const configChips = useMemo(
    () =>
      resumeConfigChips({
        tool,
        modelLabel,
        effortLabel: effort,
        permissionLabel: permissionMode ? t(PERMISSION_LABEL[permissionMode] ?? permissionMode) : "",
      }),
    [tool, modelLabel, effort, permissionMode],
  );
  const voice = useVoiceRecorder({
    value: prompt,
    onChange: setPrompt,
    onSend: () => void submit(),
  });
  const voiceTailRef = useFollowTail<HTMLTextAreaElement>(voice.showingPreview, voice.preview);
  useAutoGrow(voiceTailRef, voice.showingPreview ? voice.preview : prompt);
  // Actual measured height reported to parent: the pill floats without taking layout height, so the
  // transcript relies on this number to pad its bottom, or the last message stays buried under the pill.
  const boxRef = useRef<HTMLDivElement>(null);
  const [measureNonce, remeasure] = useReducer((n: number) => n + 1, 0);
  useLayoutEffect(() => {
    const el = boxRef.current;
    if (!el) return;
    // Report "distance from viewport bottom to component top", not just self
    // height: the pill also gets pushed up by the decision collapse bar
    // (--peek-inset), and that gap is also unavailable to the transcript.
    //
    // Use layout values (offsetHeight + computed bottom), not getBoundingClientRect:
    // rect includes transforms, so any in-flight transform animation won't
    // measure to the final value.
    onHeight?.(composerInset(el.offsetHeight, getComputedStyle(el).bottom));
  });
  // Height can change without re-rendering: attachment thumbnails load and grow
  // the component, textarea auto-grows via direct style writes, and the decision
  // collapse bar sets --peek-inset on documentElement, pushing the whole pill up.
  // Any of these require remeasuring, or the parent holds stale padding.
  useEffect(() => {
    const el = boxRef.current;
    if (!el || typeof ResizeObserver === "undefined") return;
    const ro = new ResizeObserver(() => remeasure());
    ro.observe(el);
    if (typeof MutationObserver === "undefined") return () => ro.disconnect();
    const mo = new MutationObserver(() => remeasure());
    mo.observe(document.documentElement, { attributes: true, attributeFilter: ["style"] });
    return () => {
      ro.disconnect();
      mo.disconnect();
    };
  }, []);
  void measureNonce;
  // On unmount, return padding to zero: when a session transitions from
  // "resumable" to "running", this component is swapped out, leaving stale height
  // in the parent — transcript would have a permanent white gap below the last
  // message uncovered.
  useEffect(() => () => onHeight?.(0), [onHeight]);

  // Chips still worth rendering — gates the "已排队" label too, so cancelling
  // the last one doesn't leave a header standing over an empty list.
  const visiblePending = pendingMessages
    .map((text, index) => ({ text, index }))
    .filter(({ text, index }) => !cancelledKeys.has(`${index}:${text}`));

  const cancelQueued = async (index: number, text: string) => {
    if (!client) return;
    const key = `${index}:${text}`;
    setCancelledKeys((prev) => new Set(prev).add(key));
    try {
      await client.request("cancel_pending_message", {
        sessionId: session.id,
        index,
      });
    } catch {
      // Failed — un-hide so the user sees it's still queued and can retry.
      setCancelledKeys((prev) => {
        const next = new Set(prev);
        next.delete(key);
        return next;
      });
    }
  };

  const submit = async () => {
    if (!client || busy || uploading) return;
    const text = withContextFiles(prompt.trim(), attachments);
    // Enqueue needs actual text (there's no "continue" fallback for a queued
    // follow-up); resume tolerates empty (= continue).
    if (enqueueing && !text) return;
    setBusy(true);
    // Submit in flight: pause parent's tail/thinking polling, give this
    // serialized WS to the resume req/reply so it's not blocked by a large tail
    // response. Reset on success/catch.
    onSubmitInFlight?.(true);
    // Plan A: optimistic success. Desktop responds with an early ack (well before
    // claude's cold-start produces the final reply), no need to wait 5–10s. On
    // ack, reset input and echo message; `settled` prevents re-trigger (reply
    // arrival also fires, idempotent).
    let settled = false;
    const succeed = () => {
      if (settled) return;
      settled = true;
      // Resume echoes user input optimistically to the message list; enqueue
      // hasn't sent yet, so keep the "queued" chip.
      if (!enqueueing && text) onOptimisticSend?.(text);
      clearPrompt();
      reset();
      setSent(true);
      setBusy(false);
      // Ack arrived, write delivered: resume parent's polling for the real
      // transcript (reply is small, no longer the bottleneck).
      onSubmitInFlight?.(false);
      window.setTimeout(() => setSent(false), 3000);
    };
    const method = enqueueing ? "enqueue_message" : "resume_session";
    // Fresh key per submit: relay delivery is best-effort; if the ack is lost,
    // this request may replay (or reach a second agent on the same machine), so
    // desktop uses it to deduplicate and not spawn a second claude turn.
    const idempotencyKey = randomId();
    const params = enqueueing
      ? { sessionId: session.id, workspacePath: session.workspacePath, text, idempotencyKey }
      : {
          sessionId: session.id,
          workspacePath: session.workspacePath,
          idempotencyKey,
          // Empty prompt = "continue" (relay side supplies the fallback).
          prompt: text || undefined,
          // The relay routes the resume by source (blank → claude); a Codex
          // thread resumed as claude would fail, so always send it.
          agentSource: session.agentSource ?? "",
          ...resumeConfigOverrides({ touched: configTouched, model, effort }),
          // Codex and dsh have no --permission-mode equivalent; send only to Claude.
          ...(tool === "claude" && permissionMode ? { permissionMode } : {}),
        };
    try {
      // 5th arg = onAck: fired once when desktop's early ack arrives.
      await client.request(method, params, undefined, succeed);
      succeed(); // Reply also succeeds, idempotent with onAck
    } catch (e) {
      // Regardless of failure, submit is no longer in flight: resume parent's polling.
      onSubmitInFlight?.(false);
      // Desktop explicit rejection (path doesn't exist, invalid prompt, etc.): it
      // judged and declined, report the error honestly — even if already concluded
      // via ack, still alert (consistent with new session).
      if (isDesktopRejection(e)) {
        window.alert(e.message);
        setBusy(false);
        return;
      }
      // Already concluded via early ack: subsequent timeout/disconnect reject is
      // just the reply not arriving, ignore.
      if (settled) return;
      // Never ack, no reply — request may not have reached desktop at all
      // (relay is best-effort, no re-send); report timeout honestly.
      window.alert(e instanceof Error ? e.message : t("恢复会话失败"));
      setBusy(false);
    }
  };

  return (
    <div className={styles.resumeBox} ref={boxRef}>
      {visiblePending.length > 0 && (
        <div className={styles.queuedList}>
          <div className={styles.queuedLabel}>{t("已排队，本轮结束后自动发送")}</div>
          {visiblePending.map(({ text: m, index: i }) => (
            <div key={i} className={styles.queuedChip}>
              <span className={styles.queuedText}>{m}</span>
              <button
                type="button"
                className={styles.queuedCancel}
                onClick={() => cancelQueued(i, m)}
                aria-label={t("取消这条排队消息")}
              >
                ×
              </button>
            </div>
          ))}
        </div>
      )}
      {/* Thumbnails on their own row, taking height only when there are actual
          attachments — previously squeezed with 📎/🎤 on a permanent 44px
          attachRow, taking space even when empty. */}
      {(attachments.length > 0 || pending.length > 0) && !voice.active && (
        <div className={styles.resumeThumbs}>
          <AttachmentThumbs
            paths={attachments.map((a) => a.path)}
            pending={pending}
            client={client}
            previews={previews}
            onRemove={remove}
            compact
          />
        </div>
      )}
      {/* No config in enqueue mode: this message runs with the current turn's
          settings, so showing unchangeable pills would only mislead. */}
      {!enqueueing && !voice.active && (
        <div className={styles.resumeChips}>
          {configChips.map((label) => (
            <button
              key={label}
              type="button"
              className={styles.resumeChip}
              onClick={() => setPickerOpen(true)}
            >
              {label}
            </button>
          ))}
        </div>
      )}
      {voice.active ? (
        <VoiceBar rec={voice} />
      ) : (
        <div className={styles.pill}>
          <button
            className={styles.pillBtn}
            disabled={uploading}
            onClick={() => fileRef.current?.click()}
            aria-label={uploading ? t("上传中…") : t("附件")}
          >
            {uploading ? (
              <LoaderCircle size={19} className={styles.spin} />
            ) : (
              <Plus size={20} />
            )}
          </button>
          <textarea
            ref={voiceTailRef}
            className={styles.composerInput}
            /* One line in the pill fits only ~10 Chinese chars; a long placeholder
               expands the box to two lines at rest — exactly what we're eliminating.
               The mic is right there, no need for text explanation. "Empty = continue"
               behavior unchanged, just not written in the box. */
            placeholder={enqueueing ? t("排队一条追问…") : t("继续这个会话…")}
            rows={1}
            value={voice.showingPreview ? voice.preview : prompt}
            readOnly={voice.showingPreview}
            onChange={(e) => setPrompt(e.target.value)}
          />
          {/* When there's text, yield mic position to send: two buttons side-by-side
              squeeze the right edge into two 40px targets, but users only want one. */}
          {voice.available && !prompt.trim() && (
            <span className={styles.pillMic}>
              <VoiceMicButton rec={voice} />
            </span>
          )}
          <button
            className={styles.sendBtn}
            data-success={sent || undefined}
            disabled={busy || uploading || !client || (enqueueing && !prompt.trim())}
            onClick={() => void submit()}
            aria-label={
              busy
                ? enqueueing
                  ? t("排队中…")
                  : t("发送中…")
                : enqueueing
                  ? t("排队")
                  : t("继续会话")
            }
          >
            {busy ? (
              <LoaderCircle size={17} className={styles.spin} />
            ) : sent ? (
              <Check size={17} />
            ) : (
              <Send size={16} />
            )}
          </button>
          <input
            ref={fileRef}
            type="file"
            multiple
            hidden
            onChange={(e) => {
              void addFiles(e.target.files);
              e.target.value = "";
            }}
          />
        </div>
      )}
      {pickerOpen && (
        <div className={styles.resumePicker}>
          <div className={styles.pickerBackdrop} onClick={() => setPickerOpen(false)} />
          <div className={styles.pickerSheet} role="dialog" aria-label={t("运行配置")}>
            <div className={styles.pickerGrabber} />
            <div className={styles.pickerHead}>
              <span />
              <strong>{t("运行配置")}</strong>
              <button onClick={() => setPickerOpen(false)}>{t("完成")}</button>
            </div>
            <div className={styles.pickerBody}>
              <OptionSelects
                tool={tool}
                client={client}
                model={model}
                effort={effort}
                permissionMode={permissionMode}
                permissionDefaultLabel="沿用权限"
                onChange={(p) => {
                  if (p.model !== undefined) {
                    setModel(p.model);
                    setConfigTouched(true);
                  }
                  if (p.effort !== undefined) {
                    setEffort(p.effort);
                    setConfigTouched(true);
                  }
                  if (p.permissionMode !== undefined) setPermissionMode(p.permissionMode);
                }}
              />
            </div>
          </div>
        </div>
      )}
    </div>
  );
}
