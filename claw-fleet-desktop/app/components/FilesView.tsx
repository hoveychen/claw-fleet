import { invoke } from "@tauri-apps/api/core";
import { lazy, Suspense, useCallback, useEffect, useMemo, useRef, useState, type ReactNode } from "react";
import { useTranslation } from "react-i18next";
import {
  Check,
  ChevronDown,
  ChevronRight,
  CloudDownload,
  CloudUpload,
  Copy,
  Ellipsis,
  Eye,
  EyeOff,
  ExternalLink,
  FolderSearch,
  GitBranch,
  HardDrive,
  RefreshCw,
} from "lucide-react";
import { writeText } from "@tauri-apps/plugin-clipboard-manager";
import { CopyButton } from "./CopyButton";
import { ConfirmDialog } from "./ConfirmDialog";
import { ContextMenu, type ContextMenuAnchor, type ContextMenuItem } from "./ContextMenu";
import { FileKindIcon, fileKind, officeModeOf } from "./fileKind";
import { EmptyState } from "./EmptyState";
import { timeAgo } from "./timeAgo";
import type { OfficeMode } from "../officePreview";
import { canRevealPath } from "../canReveal";
import {
  useUIStore,
  type FileNavRequest,
} from "../store";
import { FilePreview, FileTree, formatSize } from "./ExplorerPane";
import type { ExplorerEntry, ExplorerFileContent, RevealRequest } from "./ExplorerPane";
import { planRevealFallback } from "./revealFallback";
import { useResizableWidth } from "../hooks/useResizableWidth";
import { ResizeHandle } from "./ResizeHandle";
import styles from "./MemoryView.module.css";
import skillStyles from "./SkillsView.module.css";
import fileStyles from "./FilesView.module.css";
import { Presence } from "./Presence";
import { Skeleton, SkeletonCard, SkeletonList, Spinner } from "./loading";
import { useDelayedFlag } from "../hooks/useDelayedFlag";

// Lazy for the same reason as on the Library page: the Office renderers are
// large and most visits to this page never open a document.
const OfficePreview = lazy(() => import("./OfficePreview"));

// ── Types (mirror claw-fleet-core/src/file_explorer.rs) ─────────────────────

interface ExplorerRoot {
  label: string;
  path: string;
  branch: string | null;
  isWorktree: boolean;
}

// mirror claw-fleet-core/src/git_ops.rs
interface DirtyFile {
  path: string;
  status: string;
}

interface GitStatus {
  isGit: boolean;
  branch: string | null;
  upstream: string | null;
  /** Remote *URL* (`git@host:owner/repo.git`) — not the `origin/main` ref name
   *  `upstream` carries. Null when the repo has no matching remote. */
  remoteUrl: string | null;
  ahead: number | null;
  behind: number | null;
  dirtyCount: number;
  dirtyFiles: DirtyFile[];
}

interface GitOpResult {
  ok: boolean;
  output: string;
}

// ── Helpers ──────────────────────────────────────────────────────────────────
//
// Tree rendering and its size/markdown helpers live in ExplorerPane, shared with
// the session scratchpad. Root-picking stays here: it is the workspace
// explorer's own concern — the scratchpad has a single root and never needs it.

/** The root that owns `absPath` — longest matching prefix, so a worktree root
 *  wins over the main checkout it lives inside. */
function pickRoot(roots: ExplorerRoot[], absPath: string): ExplorerRoot | null {
  let best: ExplorerRoot | null = null;
  for (const root of roots) {
    const prefix = root.path.endsWith("/") ? root.path : `${root.path}/`;
    if (absPath.startsWith(prefix) && (!best || root.path.length > best.path.length)) {
      best = root;
    }
  }
  return best;
}

/** Last path segment, separator-agnostic. Used to name a workspace card when
 *  the path was collapsed to a repo root (or added by hand) and no session's
 *  `workspaceName` applies. */
function basename(p: string): string {
  const normalized = p.replace(/\\/g, "/").replace(/\/+$/, "");
  const slash = normalized.lastIndexOf("/");
  return slash >= 0 ? normalized.slice(slash + 1) : normalized;
}

// ── Explorer for one workspace ───────────────────────────────────────────────

/** Embedded in the Projects page's code tab. `name` heads the breadcrumb;
 *  `extraActions` lets the page add its own buttons to the toolbar. */
export function WorkspaceExplorer({
  workspace,
  name,
  nav,
  extraActions,
}: {
  workspace: string;
  name: string;
  nav: FileNavRequest | null;
  /** Rendered at the start of the toolbar's action group. */
  extraActions?: ReactNode;
}) {
  const { t } = useTranslation();
  const clearFileNav = useUIStore((s) => s.clearFileNav);
  const markPathUnresolved = useUIStore((s) => s.markPathUnresolved);
  const { showIgnored, activeFilePath } = useUIStore(
    (s) => s.mainViewState.files,
  );
  const updateMainViewState = useUIStore((s) => s.updateMainViewState);
  const setShowIgnored = (value: boolean) =>
    updateMainViewState("files", { showIgnored: value });
  const [roots, setRoots] = useState<ExplorerRoot[] | null>(null);
  const [rootsError, setRootsError] = useState<string | null>(null);
  const [activeRoot, setActiveRoot] = useState<ExplorerRoot | null>(null);
  const {
    width: treeWidth,
    isDragging: treeDragging,
    onMouseDown: onTreeMouseDown,
  } = useResizableWidth("files-tree-width", { min: 140, max: 480, initial: 220 });

  const [activeFile, setActiveFile] = useState<ExplorerEntry | null>(null);
  // A clicked path that no root of this workspace owns. Held next to
  // `activeFile` rather than inside it: it has no root and no relative path, so
  // it can't take part in tree selection — it only replaces the preview pane.
  const [externalPath, setExternalPath] = useState<string | null>(null);
  // A clicked path the tree could not reveal and the suffix search could not
  // rescue: either nothing matches it, or several do. Occupies the preview pane
  // the same way `externalPath` does — the point is that the click says
  // *something* back.
  const [revealMiss, setRevealMiss] = useState<{
    relPath: string;
    candidates: string[];
  } | null>(null);
  // Every reading of the clicked path that the chip stat'ed and found empty.
  // Carried alongside the miss surfaces rather than inside them because both
  // of them want it and neither owns it: it is a property of the click, and
  // without it the page can only name the one guess it happened to open.
  const [navTried, setNavTried] = useState<string[]>([]);
  // The suffix search behind a failed reveal, while it runs. A clicked path
  // otherwise gets no feedback until the backend walk comes back.
  const [revealSearch, setRevealSearch] = useState<string | null>(null);
  const showRevealSearch = useDelayedFlag(revealSearch !== null);

  useEffect(() => {
    invoke<ExplorerRoot[]>("list_explorer_roots", { workspace })
      .then((r) => {
        setRoots(r);
        const savedRoot = useUIStore.getState().mainViewState.files.activeRootPath;
        const nextRoot = r.find((root) => root.path === savedRoot) ?? r[0] ?? null;
        setActiveRoot(nextRoot);
        updateMainViewState("files", {
          activeRootPath: nextRoot?.path ?? null,
          activeFilePath: nextRoot?.path === savedRoot
            ? useUIStore.getState().mainViewState.files.activeFilePath
            : null,
        });
      })
      .catch((e) => {
        setRoots([]);
        setRootsError(String(e));
      });
  }, [workspace, updateMainViewState]);

  const loadDir = useCallback(
    async (rel: string): Promise<ExplorerEntry[] | null> => {
      if (!activeRoot) return null;
      try {
        return await invoke<ExplorerEntry[]>("list_explorer_dir", {
          workspace,
          root: activeRoot.path,
          relPath: rel,
          showIgnored,
        });
      } catch {
        return null;
      }
    },
    [workspace, activeRoot, showIgnored],
  );

  // Switching root (or toggling ignored) invalidates the selection; FileTree
  // reloads itself off the new `loadDir` identity.
  useEffect(() => {
    setActiveFile(null);
  }, [activeRoot, loadDir]);

  const readFile = useCallback(
    (relPath: string) =>
      invoke<ExplorerFileContent>("read_explorer_file", {
        workspace,
        root: activeRoot?.path ?? "",
        relPath,
      }),
    [workspace, activeRoot],
  );

  // ── Reveal a path clicked in agent prose ───────────────────────────────────
  //
  // Pick the root that owns the path and reduce it to a root-relative one; the
  // tree does the actual expanding, since it is what holds the expansion state.
  // Switching roots re-runs this effect, so a path in a root we are not showing
  // resolves on the second pass.
  const [reveal, setReveal] = useState<RevealRequest | null>(null);
  useEffect(() => {
    if (!nav || !roots || !activeRoot) return;

    setNavTried(nav.tried ?? []);
    const owner = pickRoot(roots, nav.absPath);
    if (!owner) {
      // Outside every root of this workspace — an agent naming /tmp/report.md,
      // a file in another repo, something under ~. There is no tree node to
      // reveal, so preview the single file on its own instead of (as this used
      // to) dropping the click on the floor with no feedback at all.
      setExternalPath(nav.absPath);
      clearFileNav();
      return;
    }
    setExternalPath(null);
    setRevealMiss(null);
    if (owner.path !== activeRoot.path) {
      setActiveRoot(owner); // this effect retries once the new root is active
      updateMainViewState("files", { activeRootPath: owner.path, activeFilePath: null });
      return;
    }

    const prefix = owner.path.endsWith("/") ? owner.path : `${owner.path}/`;
    const rel = nav.absPath.slice(prefix.length).replace(/\/$/, "");
    setReveal({ relPath: rel, nonce: nav.nonce, reportMiss: true });
  }, [nav, roots, activeRoot, clearFileNav, updateMainViewState]);

  // Reveal a file the user clicked in the source-control panel. Its path is
  // already relative to the active root (git reports work-dir-relative paths,
  // and the root IS the repo work dir), so no root-picking is needed. Clicks
  // get their own strictly-decreasing (negative) nonce space so they never
  // collide with the non-negative `fileNav` nonces the effect above uses.
  const clickNonce = useRef(0);
  const revealFile = useCallback((relPath: string) => {
    clickNonce.current -= 1;
    setReveal({ relPath, nonce: clickNonce.current });
  }, []);

  // ── Nothing to reveal: rescue the click or explain it ──────────────────────
  //
  // Agents name paths relative to whatever directory they had in mind, which is
  // often a subdirectory of the workspace — a card saying `public/app-icon.png`
  // about `claw-fleet-desktop/public/app-icon.png`. The chip's join produces a
  // path that does not exist, and until this existed the tree failed silently:
  // the 仓库 page opened and nothing else happened. Ask the backend where that
  // tail actually lives, and follow it only when the answer is unambiguous.
  const handleRevealFailed = useCallback(
    async (relPath: string) => {
      if (!activeRoot) return;
      let candidates: string[] = [];
      setRevealSearch(relPath);
      try {
        candidates = await invoke<string[]>("find_explorer_path", {
          workspace,
          root: activeRoot.path,
          relSuffix: relPath,
        });
      } catch {
        candidates = []; // treat a failed search as "found nothing"
      } finally {
        // A newer search may have started meanwhile; only clear our own.
        setRevealSearch((cur) => (cur === relPath ? null : cur));
      }
      const plan = planRevealFallback(relPath, candidates);
      const prefix = activeRoot.path.endsWith("/") ? activeRoot.path : `${activeRoot.path}/`;
      switch (plan.kind) {
        case "retry":
          clickNonce.current -= 1;
          setReveal({ relPath: plan.relPath, nonce: clickNonce.current, reportMiss: true });
          break;
        case "preview":
          // The path was right; the tree is just filtering it (gitignored with
          // "Show Ignored" off). Show it on its own rather than arguing with
          // the filter.
          setExternalPath(prefix + plan.relPath);
          break;
        case "ambiguous":
          setRevealMiss({ relPath, candidates: plan.candidates });
          break;
        case "missing":
          setRevealMiss({ relPath, candidates: [] });
          // Receipt for the chip that sent us here, so it can mark itself
          // broken. Only for "missing": the other three outcomes all end in
          // the user seeing something real.
          markPathUnresolved(prefix + relPath);
          break;
      }
    },
    [workspace, activeRoot, markPathUnresolved],
  );

  // Rehydrate the selected file object from its stable root-relative path after
  // this explorer remounts. FileTree owns directory loading, so its reveal path
  // is the single source of truth for reconstructing the entry.
  //
  // `externalPath` blocks it: revealing ends in `onPick` → `selectFile`, which
  // drops the out-of-tree preview. Without the guard, an external path clicked
  // while some tree file was already selected flashed up and was immediately
  // replaced by that file. Clearing the preview re-runs this and restores the
  // selection, which is what "close" should do anyway.
  useEffect(() => {
    if (!activeRoot || !activeFilePath || nav || externalPath) return;
    clickNonce.current -= 1;
    setReveal({ relPath: activeFilePath, nonce: clickNonce.current });
  }, [activeRoot, activeFilePath, nav, externalPath, loadDir]);

  const selectRoot = (root: ExplorerRoot) => {
    setActiveRoot(root);
    updateMainViewState("files", { activeRootPath: root.path, activeFilePath: null });
  };

  // Memoised because FileTree's reveal effect lists it as a dependency: a fresh
  // closure every render re-runs that effect continuously, which is wasted work
  // — and used to be worse than wasted, since the effect's old `stale` guard
  // then swallowed its own async callbacks (see the nonce guard in
  // ExplorerPane). The identity is genuinely stable: every call here is either
  // a setState setter or the zustand action, none of which change.
  const selectFile = useCallback(
    (entry: ExplorerEntry) => {
      setActiveFile(entry);
      setExternalPath(null); // picking in the tree leaves the out-of-tree preview
      setRevealMiss(null);
      updateMainViewState("files", { activeFilePath: entry.relativePath });
    },
    [updateMainViewState],
  );

  // Back to the root: no file selected, so the preview shows the recent changes.
  const clearSelection = () => {
    setActiveFile(null);
    setExternalPath(null);
    setRevealMiss(null);
    updateMainViewState("files", { activeFilePath: null });
  };

  const git = useGitStatus(workspace, activeRoot?.path ?? null);
  const changed = useMemo(
    () => new Map((git.status?.dirtyFiles ?? []).map((f) => [f.path, f.status])),
    [git.status],
  );

  const canReveal = canRevealPath();
  const revealKey =
    document.documentElement.getAttribute("data-platform") === "windows"
      ? "paths.reveal_in_explorer"
      : "paths.reveal_in_finder";
  const absOf = (rel: string) => (activeRoot ? (rel ? `${activeRoot.path}/${rel}` : activeRoot.path) : rel);
  const pathActions = (abs: string, dividerBefore = false): ContextMenuItem[] => [
    ...(canReveal
      ? [{
          id: "reveal",
          label: t(revealKey),
          icon: <FolderSearch size={13} strokeWidth={1.7} />,
          dividerBefore,
          onSelect: () => void invoke("reveal_path", { path: abs }).catch(() => {}),
        }]
      : []),
    {
      id: "copy",
      label: t("paths.copy_path"),
      icon: <Copy size={13} strokeWidth={1.7} />,
      dividerBefore: dividerBefore && !canReveal,
      onSelect: () => void writeText(abs).catch(() => {}),
    },
  ];

  // One anchored menu at a time: "⋯", a tree row's right-click, or the root
  // crumb's worktree switcher.
  const [menu, setMenu] = useState<{ anchor: ContextMenuAnchor; items: ContextMenuItem[] } | null>(null);
  const openMenuAt = (el: HTMLElement, items: ContextMenuItem[]) => {
    const r = el.getBoundingClientRect();
    setMenu({ anchor: { x: r.left, y: r.bottom + 4 }, items });
  };

  const openMoreMenu = (el: HTMLElement) =>
    openMenuAt(el, [
      {
        id: "hidden",
        label: t("files.show_hidden"),
        icon: showIgnored ? <Eye size={13} strokeWidth={1.7} /> : <EyeOff size={13} strokeWidth={1.7} />,
        active: showIgnored,
        onSelect: () => setShowIgnored(!showIgnored),
      },
      ...pathActions(activeFile ? absOf(activeFile.relativePath) : absOf(""), true),
    ]);

  const onEntryContextMenu = (entry: ExplorerEntry, e: React.MouseEvent) =>
    setMenu({
      anchor: { x: e.clientX, y: e.clientY },
      items: [
        ...(entry.isDir
          ? []
          : [{ id: "open", label: t("files.open_here"), onSelect: () => selectFile(entry) }]),
        ...pathActions(absOf(entry.relativePath), !entry.isDir),
      ],
    });

  // Breadcrumb: the root, then every folder down to the selected file.
  const crumbs = activeFile ? activeFile.relativePath.split("/") : [];
  const revealDir = (depth: number) => {
    clickNonce.current -= 1;
    setReveal({ relPath: crumbs.slice(0, depth + 1).join("/"), nonce: clickNonce.current });
  };
  const multiRoot = (roots?.length ?? 0) > 1;

  return (
    <>
      <div className={fileStyles.toolbar}>
        <nav className={fileStyles.crumbs} aria-label={t("files.tree_label")}>
          {roots === null ? (
            <Skeleton inline width={120} height={16} radius="var(--radius-pill)" />
          ) : (
            <button
              type="button"
              className={`${fileStyles.crumb} ${fileStyles.crumb_root}`}
              title={activeRoot?.path}
              onClick={(e) =>
                multiRoot
                  ? openMenuAt(
                      e.currentTarget,
                      roots.map((root) => ({
                        id: root.path,
                        label: root.label,
                        sub: root.branch ?? undefined,
                        icon: root.isWorktree ? <GitBranch size={13} strokeWidth={1.6} /> : undefined,
                        active: root.path === activeRoot?.path,
                        onSelect: () => selectRoot(root),
                      })),
                    )
                  : clearSelection()
              }
            >
              <FileKindIcon kind="folder" size={14} className={fileStyles.kind_icon} />
              {multiRoot ? activeRoot?.label : name}
              {multiRoot && activeRoot?.branch && (
                <span className={fileStyles.root_branch}>{activeRoot.branch}</span>
              )}
              {multiRoot && <ChevronDown size={12} strokeWidth={2} />}
            </button>
          )}
          {crumbs.map((seg, i) => {
            const last = i === crumbs.length - 1;
            return (
              <span key={i} className={fileStyles.crumb_seg}>
                <ChevronRight size={12} strokeWidth={2} className={fileStyles.crumb_sep} />
                {last ? (
                  <span className={`${fileStyles.crumb} ${fileStyles.crumb_current}`}>{seg}</span>
                ) : (
                  <button type="button" className={fileStyles.crumb} onClick={() => revealDir(i)}>
                    {seg}
                  </button>
                )}
              </span>
            );
          })}
        </nav>
        <div className={fileStyles.toolbar_actions}>
          {extraActions}
          {activeRoot && (
            <SyncStatus
              key={activeRoot.path}
              git={git}
              workspace={workspace}
              root={activeRoot.path}
              onRevealFile={revealFile}
            />
          )}
          <button
            type="button"
            className={fileStyles.icon_btn}
            onClick={(e) => openMoreMenu(e.currentTarget)}
            title={t("files.more")}
            aria-label={t("files.more")}
          >
            <Ellipsis size={15} strokeWidth={1.8} />
          </button>
        </div>
      </div>

          <div className={skillStyles.detail_split}>
            <aside
              className={`${skillStyles.tree_pane} ${fileStyles.tree_pane}`}
              style={{ width: treeWidth }}
            >
              {roots === null && <SkeletonList rows={10} meta={false} />}
              {rootsError && <p className={skillStyles.tree_empty}>{rootsError}</p>}
              {activeRoot && (
                <FileTree
                  loadDir={loadDir}
                  activeFile={activeFile}
                  onPick={selectFile}
                  reveal={reveal}
                  onRevealed={clearFileNav}
                  onRevealFailed={handleRevealFailed}
                  hideDotfiles={!showIgnored}
                  changed={changed}
                  meta="time"
                  onEntryContextMenu={onEntryContextMenu}
                />
              )}

              <ResizeHandle active={treeDragging} onMouseDown={onTreeMouseDown} />
            </aside>

            <div className={styles.detail_body}>
              {revealMiss ? (
                <RevealMissNotice
                  miss={revealMiss}
                  tried={navTried}
                  onPick={revealFile}
                  onClose={() => setRevealMiss(null)}
                />
              ) : externalPath ? (
                <ExternalFilePreview
                  path={externalPath}
                  tried={navTried}
                  onClose={() => setExternalPath(null)}
                />
              ) : showRevealSearch && revealSearch ? (
                <div className={fileStyles.reveal_searching} role="status">
                  <Spinner size={12} />
                  {t("files.reveal_searching", "正在仓库里查找 {{path}}…", { path: revealSearch })}
                </div>
              ) : activeFile && activeRoot ? (
                <WorkspaceFileView
                  file={activeFile}
                  workspace={workspace}
                  root={activeRoot.path}
                  readFile={readFile}
                />
              ) : (
                <RecentChanges git={git} onPick={revealFile} />
              )}
            </div>
          </div>

      <Presence when={Boolean(menu)}>
        {menu && <ContextMenu anchor={menu.anchor} items={menu.items} onClose={() => setMenu(null)} />}
      </Presence>
    </>
  );
}

// ── One selected file: header + the right viewer for its type ────────────────

interface ExplorerBytes {
  base64: string;
  mime: string;
  sizeBytes: number;
}

/**
 * A file the way a file manager shows it: what it is, how big, when it last
 * changed, and a way to open it in the app that owns it. Office documents and
 * PDFs render in place from their raw bytes; text, images and HTML go through
 * the shared `FilePreview`; anything else gets an "open it" card instead of a
 * dead-end "binary file" line.
 */
function WorkspaceFileView({
  file,
  workspace,
  root,
  readFile,
}: {
  file: ExplorerEntry;
  workspace: string;
  root: string;
  readFile: (relPath: string) => Promise<ExplorerFileContent>;
}) {
  const { t } = useTranslation();
  const kind = fileKind(file.name);
  const office = officeModeOf(file.name);
  const abs = `${root}/${file.relativePath}`;
  const canOpen = canRevealPath();
  const [openFailed, setOpenFailed] = useState(false);

  const openDefault = async () => {
    try {
      await invoke("open_explorer_file", { workspace, root, relPath: file.relativePath });
      setOpenFailed(false);
    } catch {
      setOpenFailed(true);
      setTimeout(() => setOpenFailed(false), 2000);
    }
  };

  const meta = [
    t(`files.kind.${kind}`),
    file.sizeBytes > 0 ? formatSize(file.sizeBytes) : null,
    file.modifiedMs > 0 ? t("files.modified", { when: timeAgo(file.modifiedMs, t) }) : null,
  ].filter(Boolean);

  const noPreview = (
    <EmptyState
      icon={<FileKindIcon kind={kind} size={28} />}
      title={t("files.no_preview_title")}
      subtitle={canOpen ? t("files.no_preview_sub") : undefined}
      action={canOpen ? { label: t("files.open_default"), onClick: () => void openDefault() } : undefined}
    />
  );

  return (
    <div className={fileStyles.file_view}>
      <div className={fileStyles.file_head}>
        <FileKindIcon kind={kind} size={26} className={fileStyles.kind_icon} />
        <div className={fileStyles.file_head_text}>
          <span className={fileStyles.file_head_name}>{file.name}</span>
          <span className={fileStyles.file_head_meta}>{meta.join(" · ")}</span>
        </div>
        <div className={fileStyles.file_head_actions}>
          {canOpen && (
            <button type="button" className={fileStyles.head_btn} onClick={() => void openDefault()}>
              <ExternalLink size={13} strokeWidth={1.7} />
              {openFailed ? t("files.open_failed") : t("files.open_default")}
            </button>
          )}
          {canOpen && (
            <button
              type="button"
              className={fileStyles.icon_btn}
              onClick={() => void invoke("reveal_path", { path: abs }).catch(() => {})}
              title={t(revealKeyFor())}
              aria-label={t(revealKeyFor())}
            >
              <FolderSearch size={14} strokeWidth={1.7} />
            </button>
          )}
          <CopyButton text={abs} label={t("paths.copy_path")} />
        </div>
      </div>
      <div className={fileStyles.file_body}>
        {office || kind === "pdf" ? (
          <BytesPreview
            key={file.relativePath}
            workspace={workspace}
            root={root}
            file={file}
            mode={office ?? "pdf"}
            fallback={noPreview}
          />
        ) : (
          <FilePreview file={file} load={readFile} renderBinary={() => noPreview} />
        )}
      </div>
    </div>
  );
}

function revealKeyFor(): string {
  return document.documentElement.getAttribute("data-platform") === "windows"
    ? "paths.reveal_in_explorer"
    : "paths.reveal_in_finder";
}

/** Office and PDF render from the file's bytes, handed over as a blob URL —
 *  the same viewers the Library page uses for deliverables. */
function BytesPreview({
  workspace,
  root,
  file,
  mode,
  fallback,
}: {
  workspace: string;
  root: string;
  file: ExplorerEntry;
  mode: OfficeMode | "pdf";
  fallback: ReactNode;
}) {
  const [url, setUrl] = useState<string | null>(null);
  const [failed, setFailed] = useState(false);

  useEffect(() => {
    let alive = true;
    let made: string | null = null;
    invoke<ExplorerBytes>("read_explorer_bytes", { workspace, root, relPath: file.relativePath })
      .then((b) => {
        if (!alive) return;
        if (!b.base64) {
          setFailed(true);
          return;
        }
        const raw = Uint8Array.from(atob(b.base64), (c) => c.charCodeAt(0));
        made = URL.createObjectURL(new Blob([raw], { type: b.mime }));
        setUrl(made);
      })
      .catch(() => alive && setFailed(true));
    return () => {
      alive = false;
      if (made) URL.revokeObjectURL(made);
    };
  }, [workspace, root, file.relativePath]);

  if (failed) return <>{fallback}</>;
  if (!url) return <SkeletonCard height="60vh" />;
  if (mode === "pdf") {
    return <iframe className={fileStyles.pdf_frame} src={url} title={file.name} />;
  }
  return (
    <div className={fileStyles.office_stage}>
      <Suspense fallback={<SkeletonCard height="60vh" />}>
        <OfficePreview mode={mode} url={url} title={file.name} />
      </Suspense>
    </div>
  );
}

// ── Empty preview: what changed lately ───────────────────────────────────────

/** Git's one-letter status, said the way a person would. */
function statusKey(status: string): string {
  switch (status) {
    case "A":
    case "?":
      return "files.scm.st_added";
    case "D":
      return "files.scm.st_deleted";
    case "R":
      return "files.scm.st_renamed";
    case "U":
      return "files.scm.st_conflict";
    default:
      return "files.scm.st_modified";
  }
}

/**
 * The preview pane before anything is picked. A blank "pick a file on the left"
 * wasted the largest area on the page; the files the agents just touched are
 * what someone opening a project's files most likely came to look at.
 */
function RecentChanges({ git, onPick }: { git: GitState; onPick: (relPath: string) => void }) {
  const { t } = useTranslation();
  const files = git.status?.isGit ? git.status.dirtyFiles : [];
  if (!git.loaded) return <SkeletonList rows={5} meta={false} />;
  if (files.length === 0) {
    return (
      <EmptyState
        icon={<FileKindIcon kind="other" size={28} />}
        title={t("files.empty_title")}
        subtitle={t("files.empty_sub")}
      />
    );
  }
  return (
    <div className={fileStyles.recent}>
      <div className={fileStyles.recent_title}>{t("files.recent_title")}</div>
      <p className={fileStyles.recent_hint}>{t("files.recent_hint")}</p>
      <ul className={fileStyles.recent_list}>
        {files.map((f) => {
          const name = basename(f.path);
          const dir = f.path.replace(/\/$/, "").slice(0, -name.length).replace(/\/$/, "");
          return (
            <li key={f.path}>
              <button
                type="button"
                className={fileStyles.recent_row}
                onClick={() => onPick(f.path.replace(/\/$/, ""))}
                title={f.path}
              >
                <FileKindIcon
                  kind={fileKind(name, f.path.endsWith("/"))}
                  size={16}
                  className={fileStyles.kind_icon}
                />
                <span className={fileStyles.recent_name}>{name}</span>
                {dir && <span className={fileStyles.recent_dir}>{dir}</span>}
                <span className={fileStyles.status_tag} data-status={f.status}>
                  {t(statusKey(f.status))}
                </span>
              </button>
            </li>
          );
        })}
      </ul>
    </div>
  );
}

// ── Unresolvable path notice ─────────────────────────────────────────────────

/**
 * A clicked path the tree could not reveal and the suffix search could not
 * rescue. Two shapes, one component: no match at all, or several.
 *
 * This exists because the alternative — what shipped before — was for the click
 * to open the Files page and then do nothing whatsoever, which reads as a broken
 * app rather than as a path the agent got wrong.
 */
function RevealMissNotice({
  miss,
  tried,
  onPick,
  onClose,
}: {
  miss: { relPath: string; candidates: string[] };
  /** Readings of the written path that were stat'ed and came up empty. */
  tried?: string[];
  onPick: (relPath: string) => void;
  onClose: () => void;
}) {
  const { t } = useTranslation();
  const ambiguous = miss.candidates.length > 0;
  return (
    <div className={fileStyles.external_wrap}>
      <div className={fileStyles.external_bar}>
        <div className={fileStyles.external_text}>
          <span className={fileStyles.external_title}>
            {t(ambiguous ? "files.reveal_ambiguous_title" : "files.reveal_miss_title")}
          </span>
          <span className={fileStyles.external_path}>
            {t(ambiguous ? "files.reveal_ambiguous_body" : "files.reveal_miss_body", {
              path: miss.relPath,
            })}
          </span>
        </div>
        <div className={fileStyles.external_actions}>
          <button className={fileStyles.external_btn} onClick={onClose}>
            {t("files.external_close")}
          </button>
        </div>
      </div>
      {ambiguous && (
        <ul className={fileStyles.reveal_candidates}>
          {miss.candidates.map((c) => (
            <li key={c}>
              <button className={fileStyles.reveal_candidate} onClick={() => onPick(c)}>
                {c}
              </button>
            </li>
          ))}
        </ul>
      )}
      <PathsTried paths={tried} />
    </div>
  );
}

/**
 * Where a click actually looked, when it found nothing.
 *
 * A path in agent prose has more than one reading (relative to the workspace,
 * relative to its parent), and until issue #106 the UI committed to one and
 * reported a bare failure — so the path on screen looked like a fact about the
 * file rather than one guess among several. Listing the readings is what makes
 * the failure diagnosable. Renders nothing when there is nothing to disclose.
 */
function PathsTried({ paths }: { paths?: string[] }) {
  const { t } = useTranslation();
  if (!paths?.length) return null;
  return (
    <div className={fileStyles.paths_tried}>
      <span className={fileStyles.paths_tried_label}>{t("files.paths_tried")}</span>
      <ul className={fileStyles.paths_tried_list}>
        {paths.map((p) => (
          <li key={p}>{p}</li>
        ))}
      </ul>
    </div>
  );
}

// ── Out-of-workspace file preview ────────────────────────────────────────────

/**
 * One file that belongs to no workspace, previewed on its own.
 *
 * Agents name such paths constantly — `/tmp/report.md` they just wrote, a file
 * in a sibling repo. The tree can't hold them (they have no root), so this
 * stands in for the whole tree+preview pair: a banner saying where the file
 * lives, plus the ordinary `FilePreview` fed by the ungated backend read.
 *
 * Exported because a detail-column file tab is the same thing minus the tree:
 * one absolute path, read-only. It passes its own `label`, since a file opened
 * from a session's prose is usually *inside* a workspace — the tab is just a
 * viewer, not the out-of-tree escape hatch this banner names by default.
 */
export function ExternalFilePreview({
  path,
  onClose,
  label,
  tried,
}: {
  path: string;
  onClose: () => void;
  /** Banner text. Defaults to the out-of-workspace notice. */
  label?: string;
  /** Readings of the written path that were stat'ed and came up empty — set
   *  when the click resolved to nothing and `path` is only the first guess. */
  tried?: string[];
}) {
  const { t } = useTranslation();
  // Same reason as the workspace menu above: no host shell to reveal into.
  const canReveal = canRevealPath();

  // FilePreview keys its read off `relativePath`; for an out-of-tree file the
  // absolute path IS the key, and the rest of the entry is only display data.
  const entry = useMemo<ExplorerEntry>(
    () => ({
      name: basename(path),
      relativePath: path,
      sizeBytes: 0,
      isDir: false,
      modifiedMs: 0,
      isIgnored: false,
      isSymlink: false,
    }),
    [path],
  );

  const load = useCallback(
    (absPath: string) => invoke<ExplorerFileContent>("read_external_file", { path: absPath }),
    [],
  );

  const revealKey =
    document.documentElement.getAttribute("data-platform") === "windows"
      ? "paths.reveal_in_explorer"
      : "paths.reveal_in_finder";

  // Same failure the path chips already surface (`markdown/pathLinks`): the
  // file can be gone by the time this is clicked. Unlike a context-menu item,
  // this button is still on screen when the promise settles, so a swallowed
  // rejection would be indistinguishable from a dead button.
  const [revealFailed, setRevealFailed] = useState(false);

  return (
    <div className={fileStyles.external_wrap}>
      <div className={fileStyles.external_bar}>
        <div className={fileStyles.external_text}>
          <span className={fileStyles.external_title}>
            {label ?? t("files.external_title")}
          </span>
          <span className={fileStyles.external_path}>{path}</span>
        </div>
        <div className={fileStyles.external_actions}>
          <CopyButton text={path} />
          {/* A remote workspace's files are on the probe host, not this Mac. */}
          {canReveal && (
            <button
              className={fileStyles.external_btn}
              onClick={async () => {
                try {
                  await invoke("reveal_path", { path });
                  setRevealFailed(false);
                } catch {
                  setRevealFailed(true);
                  setTimeout(() => setRevealFailed(false), 2000);
                }
              }}
            >
              {revealFailed ? t("paths.not_found") : t(revealKey)}
            </button>
          )}
          <button className={fileStyles.external_btn} onClick={onClose}>
            {t("files.external_close")}
          </button>
        </div>
      </div>
      <PathsTried paths={tried} />
      <div className={fileStyles.external_body}>
        <FilePreview file={entry} load={load} />
      </div>
    </div>
  );
}

// ── Source control: status hook + the sync pill ──────────────────────────────

interface GitState {
  status: GitStatus | null;
  /** False until the first git_status settles: before that a null `status`
   *  means "not asked yet", not "not a git repo". */
  loaded: boolean;
  refreshing: boolean;
  refresh: () => void;
}

/** Shared by the sync pill, the tree's change dots and the recent-changes list,
 *  so all three agree on one read of the repo. */
function useGitStatus(workspace: string, root: string | null): GitState {
  const [status, setStatus] = useState<GitStatus | null>(null);
  const [loaded, setLoaded] = useState(false);
  const [refreshing, setRefreshing] = useState(false);

  const refresh = useCallback(() => {
    if (!root) return;
    setRefreshing(true);
    invoke<GitStatus>("git_status", { workspace, root })
      .then(setStatus)
      .catch(() => setStatus(null))
      .finally(() => {
        setLoaded(true);
        setRefreshing(false);
      });
  }, [workspace, root]);

  useEffect(() => {
    setStatus(null);
    setLoaded(false);
    refresh();
  }, [refresh]);

  return { status, loaded, refreshing, refresh };
}

/**
 * Where this copy of the project stands against its cloud copy, in one pill.
 *
 * The old bar spelled it in git's own words — the remote URL, push/pull, and
 * git's raw stdout in a code block. The pill says only the thing that needs
 * attention; the panel behind it says what each state means and carries the
 * two actions, with git's output kept behind a "details" fold.
 */
function SyncStatus({
  git,
  workspace,
  root,
  onRevealFile,
}: {
  git: GitState;
  workspace: string;
  root: string;
  onRevealFile: (relPath: string) => void;
}) {
  const { t } = useTranslation();
  const { status, loaded, refreshing, refresh } = git;
  const showRefreshing = useDelayedFlag(refreshing);
  const [open, setOpen] = useState(false);
  const [busy, setBusy] = useState<null | "push" | "pull">(null);
  const [confirm, setConfirm] = useState<null | "push" | "pull">(null);
  const [opMsg, setOpMsg] = useState<{ ok: boolean; op: "push" | "pull"; text: string } | null>(null);
  const wrapRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    if (!open) return;
    const onDown = (e: MouseEvent) => {
      // The confirm dialog is portalled outside the wrapper; a click in it must
      // not fold the panel it was opened from.
      if (confirm) return;
      if (!wrapRef.current?.contains(e.target as Node)) setOpen(false);
    };
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape" && !confirm) setOpen(false);
    };
    document.addEventListener("mousedown", onDown);
    document.addEventListener("keydown", onKey);
    return () => {
      document.removeEventListener("mousedown", onDown);
      document.removeEventListener("keydown", onKey);
    };
  }, [open, confirm]);

  const runOp = useCallback(
    async (op: "push" | "pull") => {
      setConfirm(null);
      setBusy(op);
      setOpMsg(null);
      try {
        const res = await invoke<GitOpResult>(op === "push" ? "git_push" : "git_pull", {
          workspace,
          root,
        });
        setOpMsg({ ok: res.ok, op, text: res.output });
        refresh();
      } catch (e) {
        setOpMsg({ ok: false, op, text: String(e) });
      } finally {
        setBusy(null);
      }
    },
    [workspace, root, refresh],
  );

  if (!loaded) {
    return <Skeleton inline width={84} height={22} radius="var(--radius-pill)" />;
  }
  // Plain folders have nothing to sync.
  if (!status || !status.isGit) return null;

  const ahead = status.ahead ?? 0;
  const behind = status.behind ?? 0;
  const dirty = status.dirtyCount;
  const localOnly = status.upstream == null;

  // The pill names the most pressing state; the panel lists all of them.
  let tone: "ok" | "warn" | "muted";
  let label: string;
  if (behind > 0) {
    tone = "warn";
    label = t("files.scm.pill_behind", { n: behind });
  } else if (ahead > 0) {
    tone = "warn";
    label = t("files.scm.pill_ahead", { n: ahead });
  } else if (dirty > 0) {
    tone = "warn";
    label = t("files.scm.pill_dirty", { n: dirty });
  } else if (localOnly) {
    tone = "muted";
    label = t("files.scm.local_only");
  } else {
    tone = "ok";
    label = t("files.scm.synced");
  }

  const confirmMsg =
    confirm === "push"
      ? t("files.scm.push_confirm", {
          upstream: status.upstream ?? t("files.scm.default_remote"),
        })
      : t("files.scm.pull_confirm", { branch: status.branch ?? "HEAD" });

  return (
    <div className={fileStyles.sync_wrap} ref={wrapRef}>
      <button
        type="button"
        className={fileStyles.sync_pill}
        data-tone={tone}
        onClick={() => setOpen((v) => !v)}
        aria-expanded={open}
      >
        {busy ? (
          <Spinner size={12} />
        ) : tone === "ok" ? (
          <Check size={13} strokeWidth={2} />
        ) : tone === "muted" ? (
          <HardDrive size={13} strokeWidth={1.7} />
        ) : (
          <span className={fileStyles.sync_dot} />
        )}
        {label}
        <ChevronDown size={12} strokeWidth={2} />
      </button>

      <Presence when={open}>
        {open && (
          <div className={fileStyles.sync_panel} role="dialog">
            <div className={fileStyles.sync_head}>
              <span className={fileStyles.sync_title}>{t("files.scm.panel_title")}</span>
              {status.branch && (
                <span className={fileStyles.sync_branch} title={status.upstream ?? undefined}>
                  <GitBranch size={11} strokeWidth={1.7} />
                  {status.branch}
                </span>
              )}
              <span className={fileStyles.scm_spacer} />
              <button
                type="button"
                className={fileStyles.icon_btn}
                disabled={busy !== null || refreshing}
                onClick={() => {
                  setOpMsg(null);
                  refresh();
                }}
                title={t("files.scm.refresh")}
                aria-label={t("files.scm.refresh")}
              >
                {showRefreshing ? <Spinner size={12} /> : <RefreshCw size={13} strokeWidth={1.7} />}
              </button>
            </div>

            {localOnly ? (
              <p className={fileStyles.sync_line}>{t("files.scm.local_only_hint")}</p>
            ) : ahead === 0 && behind === 0 && dirty === 0 ? (
              <p className={fileStyles.sync_line}>{t("files.scm.synced_hint")}</p>
            ) : null}
            {behind > 0 && (
              <p className={fileStyles.sync_line} data-tone="warn">
                {t("files.scm.behind_hint", { n: behind })}
              </p>
            )}
            {ahead > 0 && (
              <p className={fileStyles.sync_line} data-tone="warn">
                {t("files.scm.ahead_hint", { n: ahead })}
              </p>
            )}
            {dirty > 0 && (
              <>
                <p className={fileStyles.sync_line} data-tone="warn">
                  {t("files.scm.dirty_hint", { n: dirty })}
                </p>
                <ul className={fileStyles.scm_files}>
                  {status.dirtyFiles.map((f) => (
                    <li key={f.path}>
                      <button
                        type="button"
                        className={fileStyles.scm_file}
                        onClick={() => {
                          onRevealFile(f.path.replace(/\/$/, ""));
                          setOpen(false);
                        }}
                        title={t("files.scm.reveal_file")}
                      >
                        <span className={fileStyles.status_tag} data-status={f.status}>
                          {t(statusKey(f.status))}
                        </span>
                        <span className={fileStyles.scm_file_path}>{f.path}</span>
                      </button>
                    </li>
                  ))}
                </ul>
              </>
            )}

            {!localOnly && (
              <div className={fileStyles.sync_actions}>
                <button
                  type="button"
                  className={fileStyles.sync_btn}
                  disabled={busy !== null}
                  onClick={() => setConfirm("pull")}
                >
                  {busy === "pull" ? <Spinner size={12} /> : <CloudDownload size={13} strokeWidth={1.7} />}
                  {busy === "pull" ? t("files.scm.pulling") : t("files.scm.pull")}
                </button>
                <button
                  type="button"
                  className={`${fileStyles.sync_btn} ${ahead > 0 ? fileStyles.sync_btn_primary : ""}`}
                  disabled={busy !== null}
                  onClick={() => setConfirm("push")}
                >
                  {busy === "push" ? <Spinner size={12} /> : <CloudUpload size={13} strokeWidth={1.7} />}
                  {busy === "push" ? t("files.scm.pushing") : t("files.scm.push")}
                </button>
              </div>
            )}

            {opMsg && (
              <div className={fileStyles.sync_result} data-ok={opMsg.ok}>
                {opMsg.ok
                  ? t(opMsg.op === "push" ? "files.scm.push_ok" : "files.scm.pull_ok")
                  : t("files.scm.op_failed")}
                {opMsg.text && (
                  <details className={fileStyles.sync_details}>
                    <summary>{t("files.scm.details")}</summary>
                    <pre>{opMsg.text}</pre>
                  </details>
                )}
              </div>
            )}

            {status.remoteUrl && (
              <div className={fileStyles.sync_remote} title={status.remoteUrl}>
                <span className={fileStyles.sync_remote_label}>{t("files.scm.remote_label")}</span>
                <span className={fileStyles.scm_remote_url}>{status.remoteUrl}</span>
                <CopyButton text={status.remoteUrl} label={t("files.scm.copy_remote")} />
              </div>
            )}
          </div>
        )}
      </Presence>

      <Presence when={Boolean(confirm)}>{confirm && (
        <ConfirmDialog
          message={confirmMsg}
          onConfirm={() => void runOp(confirm)}
          onCancel={() => setConfirm(null)}
        />
      )}</Presence>
    </div>
  );
}
