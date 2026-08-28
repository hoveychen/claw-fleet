import { invoke } from "@tauri-apps/api/core";
import { save } from "@tauri-apps/plugin-dialog";
import { revealItemInDir } from "@tauri-apps/plugin-opener";
import { openPath } from "@tauri-apps/plugin-opener";
import {
  Archive,
  FileSpreadsheet,
  FileText,
  FileType,
  Film,
  Image as ImageIcon,
  Music,
  Package,
  Presentation,
  Star,
  TriangleAlert,
} from "lucide-react";
import { lazy, Suspense, useCallback, useEffect, useMemo, useState } from "react";
import { useTranslation } from "react-i18next";

import { artifactBlobUrl } from "../artifactAssets";
import { isWebBuild } from "../hostEnv";
import { officeMode } from "../officePreview";
import { downloadArtifact } from "../mock/liveProxy";
import { PageShell } from "./PageShell";
import { EmptyState } from "./EmptyState";
import { TextBlock } from "./blocks/TextBlock";
import styles from "./ArtifactsView.module.css";

/** Mirrors `claw_fleet_core::artifacts::Artifact`. */
export interface Artifact {
  id: string;
  name: string;
  title: string;
  note: string;
  mime: string;
  kind: string;
  sizeBytes: number;
  createdMs: number;
  workspacePath: string;
  workspaceName: string;
  sessionId: string | null;
  sourcePath: string;
  starred: boolean;
  hardlinked: boolean;
  drifted: boolean;
}

interface StoreUsage {
  count: number;
  totalBytes: number;
  hardlinkedBytes: number;
}

type SortKey = "recent" | "size" | "name";

const KIND_ICON: Record<string, typeof FileText> = {
  image: ImageIcon,
  video: Film,
  audio: Music,
  pdf: FileType,
  doc: FileText,
  sheet: FileSpreadsheet,
  slides: Presentation,
  archive: Archive,
  text: FileText,
};

/**
 * The three Office renderers, kept out of the main bundle.
 *
 * Together they are ~1.6 MB of JavaScript (pptx-preview bundles echarts for a
 * deck's native charts), which no session should download to look at a session
 * list. `lazy` defers the module, and the module defers each library again —
 * see OfficePreview's own docs.
 */
const OfficePreview = lazy(() => import("./OfficePreview"));

/**
 * Which renderer a `text`-kind artifact wants.
 *
 * The store buckets every `text/*` file into one `text` kind, which is right
 * for icons but wrong for the stage: a markdown spec and an html report are
 * both deliverables people hand over (the routing rule is audience, not
 * format), and showing either as raw source is the same failure as an .xlsx
 * opening blank in the wiki. Sniffing happens here, on the mime the store
 * already derived, so no component re-parses an extension.
 */
export function textPreviewMode(mime: string): "markdown" | "html" | "plain" {
  const base = mime.split(";")[0].trim().toLowerCase();
  if (base === "text/markdown") return "markdown";
  if (base === "text/html") return "html";
  return "plain";
}

export function formatBytes(n: number): string {
  if (n < 1024) return `${n} B`;
  const units = ["KB", "MB", "GB", "TB"];
  let v = n / 1024;
  let i = 0;
  while (v >= 1024 && i < units.length - 1) {
    v /= 1024;
    i += 1;
  }
  // One decimal below 10 so "1.4 MB" doesn't round to a useless "1 MB", none
  // above it where the extra digit is noise.
  return `${v < 10 ? v.toFixed(1) : Math.round(v)} ${units[i]}`;
}

/**
 * Order artifacts for the grid.
 *
 * Its own exported function because the ordering is the part worth testing:
 * "newest first" has to survive same-millisecond ids (two `fleet artifact add`
 * calls in one script), and the name sort has to be locale-aware or a CJK
 * title lands in a random position.
 */
export function sortArtifacts(list: Artifact[], key: SortKey): Artifact[] {
  const out = [...list];
  switch (key) {
    case "size":
      return out.sort((a, b) => b.sizeBytes - a.sizeBytes || a.id.localeCompare(b.id));
    case "name":
      return out.sort((a, b) => a.title.localeCompare(b.title, undefined, { numeric: true }));
    case "recent":
    default:
      // Ids are timestamps with a collision suffix, so they break a createdMs
      // tie in the same direction the store's own listing does.
      return out.sort((a, b) => b.createdMs - a.createdMs || b.id.localeCompare(a.id));
  }
}

/** Apply the sub-bar's filters. Exported for the same reason as the sort. */
export function filterArtifacts(
  list: Artifact[],
  opts: { query: string; workspace: string; starredOnly: boolean },
): Artifact[] {
  const q = opts.query.trim().toLowerCase();
  return list.filter((a) => {
    if (opts.starredOnly && !a.starred) return false;
    if (opts.workspace && a.workspacePath !== opts.workspace) return false;
    if (!q) return true;
    // Note and filename included on purpose: the title is often the filename,
    // and what the user remembers is as likely to be "the one about Q3".
    return (
      a.title.toLowerCase().includes(q) ||
      a.name.toLowerCase().includes(q) ||
      a.note.toLowerCase().includes(q)
    );
  });
}

export function ArtifactsView() {
  const { t } = useTranslation();
  const [items, setItems] = useState<Artifact[] | null>(null);
  const [usage, setUsage] = useState<StoreUsage | null>(null);
  const [query, setQuery] = useState("");
  const [workspace, setWorkspace] = useState("");
  const [starredOnly, setStarredOnly] = useState(false);
  const [sortKey, setSortKey] = useState<SortKey>("recent");
  const [selectedId, setSelectedId] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);

  const load = useCallback(async () => {
    // `?? []` rather than the raw result: the mock's `default:` branch answers
    // null, and a null here would blank the page on the first filter() — the
    // exact failure MOCK_WIKI_DOCS exists to prevent for the wiki.
    const list = (await invoke<Artifact[]>("list_artifacts").catch(() => [])) ?? [];
    setItems(list);
    setUsage((await invoke<StoreUsage>("artifact_usage").catch(() => null)) ?? null);
  }, []);

  useEffect(() => {
    void load();
  }, [load]);

  const workspaces = useMemo(() => {
    const seen = new Map<string, string>();
    for (const a of items ?? []) seen.set(a.workspacePath, a.workspaceName);
    return [...seen.entries()].sort((a, b) => a[1].localeCompare(b[1]));
  }, [items]);

  const shown = useMemo(
    () => sortArtifacts(filterArtifacts(items ?? [], { query, workspace, starredOnly }), sortKey),
    [items, query, workspace, starredOnly, sortKey],
  );

  const selected = useMemo(
    () => (items ?? []).find((a) => a.id === selectedId) ?? null,
    [items, selectedId],
  );

  const patch = useCallback(
    async (id: string, fields: { title?: string; note?: string; starred?: boolean }) => {
      try {
        const updated = await invoke<Artifact>("update_artifact", { id, ...fields });
        setItems((prev) => (prev ?? []).map((a) => (a.id === id ? updated : a)));
        setError(null);
      } catch (e) {
        setError(String(e));
      }
    },
    [],
  );

  const subBar = (
    <div className={styles.filters}>
      <button
        className={`${styles.chip} ${!starredOnly ? styles.chip_on : ""}`}
        onClick={() => setStarredOnly(false)}
      >
        {t("artifacts.filter_all", "全部")}
      </button>
      <button
        className={`${styles.chip} ${starredOnly ? styles.chip_on : ""}`}
        onClick={() => setStarredOnly(true)}
      >
        {t("artifacts.filter_starred", "已收藏")}
      </button>
      <select
        className={styles.select}
        value={workspace}
        onChange={(e) => setWorkspace(e.target.value)}
      >
        <option value="">{t("artifacts.all_workspaces", "全部工作区")}</option>
        {workspaces.map(([path, name]) => (
          <option key={path} value={path}>
            {name}
          </option>
        ))}
      </select>
      <select
        className={styles.select}
        value={sortKey}
        onChange={(e) => setSortKey(e.target.value as SortKey)}
        aria-label={t("artifacts.sort_by", "排序方式")}
      >
        <option value="recent">{t("artifacts.sort_recent", "最近加入")}</option>
        <option value="size">{t("artifacts.sort_size", "大小")}</option>
        <option value="name">{t("artifacts.sort_name", "名称")}</option>
      </select>
      {usage && usage.count > 0 && (
        <span className={styles.usage}>
          {t("artifacts.usage", "{{count}} 份 · 共 {{size}}", {
            count: usage.count,
            size: formatBytes(usage.totalBytes),
          })}
        </span>
      )}
    </div>
  );

  return (
    <PageShell
      view="artifacts"
      title={t("artifacts.panel_title", "产出")}
      count={items?.length ?? null}
      search={{
        value: query,
        onChange: setQuery,
        placeholder: t("artifacts.search_placeholder", "搜索产出…"),
      }}
      subBar={selected ? undefined : subBar}
    >
      {error && <div className={styles.error_line}>{error}</div>}
      {selected ? (
        <ArtifactDetail
          artifact={selected}
          onBack={() => setSelectedId(null)}
          onPatch={patch}
          onDeleted={async () => {
            setSelectedId(null);
            await load();
          }}
          onError={setError}
        />
      ) : items === null ? (
        <EmptyState icon={<Package size={30} strokeWidth={1.1} />} title={t("artifacts.loading", "加载中…")} />
      ) : shown.length === 0 ? (
        <EmptyState
          icon={<Package size={30} strokeWidth={1.1} />}
          title={t("artifacts.empty_title", "还没有产出")}
          subtitle={t(
            "artifacts.empty_subtitle",
            "Agent 用 `fleet artifact add <path>` 把交付物存进来。",
          )}
        />
      ) : (
        <div className={styles.grid}>
          {shown.map((a) => (
            <ArtifactCard
              key={a.id}
              artifact={a}
              onOpen={() => setSelectedId(a.id)}
              onToggleStar={() => patch(a.id, { starred: !a.starred })}
            />
          ))}
        </div>
      )}
    </PageShell>
  );
}

function ArtifactCard({
  artifact,
  onOpen,
  onToggleStar,
}: {
  artifact: Artifact;
  onOpen: () => void;
  onToggleStar: () => void;
}) {
  const { t } = useTranslation();
  const Icon = KIND_ICON[artifact.kind] ?? FileText;
  return (
    <div className={styles.card} onClick={onOpen} role="button" tabIndex={0}
      onKeyDown={(e) => {
        if (e.key === "Enter" || e.key === " ") {
          e.preventDefault();
          onOpen();
        }
      }}
    >
      <div className={styles.thumb}>
        {artifact.kind === "image" ? (
          <img src={artifactBlobUrl(artifact.id, artifact.name)} alt={artifact.title} />
        ) : (
          <Icon size={32} strokeWidth={1.2} className={styles.thumb_icon} />
        )}
        <span className={styles.kind_tag}>
          {t(`artifacts.kind.${artifact.kind}`, artifact.kind)}
        </span>
        <button
          className={`${styles.star_btn} ${artifact.starred ? styles.star_on : ""}`}
          title={t(artifact.starred ? "artifacts.unstar" : "artifacts.star", "收藏")}
          onClick={(e) => {
            e.stopPropagation();
            onToggleStar();
          }}
        >
          <Star size={13} strokeWidth={1.6} fill={artifact.starred ? "currentColor" : "none"} />
        </button>
      </div>
      <div className={styles.card_body}>
        <div className={styles.card_title} title={artifact.title}>
          {artifact.title}
        </div>
        <div className={styles.card_meta}>
          <span>{formatBytes(artifact.sizeBytes)}</span>
          <span className={styles.ws_chip} title={artifact.workspacePath}>
            {artifact.workspaceName}
          </span>
          {artifact.drifted && (
            <TriangleAlert
              size={11}
              className={styles.drift_flag}
              aria-label={t("artifacts.drifted", "源文件已被改写")}
            />
          )}
        </div>
      </div>
    </div>
  );
}

function ArtifactDetail({
  artifact,
  onBack,
  onPatch,
  onDeleted,
  onError,
}: {
  artifact: Artifact;
  onBack: () => void;
  onPatch: (id: string, fields: { title?: string; note?: string; starred?: boolean }) => void;
  onDeleted: () => void;
  onError: (msg: string | null) => void;
}) {
  const { t } = useTranslation();
  const [localPath, setLocalPath] = useState<string | null>(null);
  const [note, setNote] = useState(artifact.note);

  useEffect(() => setNote(artifact.note), [artifact.id, artifact.note]);

  // Null for a remote workspace — the two OS-level actions are hidden rather
  // than pointed at a path on the other machine.
  useEffect(() => {
    let current = artifact.id;
    invoke<string | null>("artifact_local_path", { id: artifact.id })
      .then((p) => {
        if (current === artifact.id) setLocalPath(p);
      })
      .catch(() => setLocalPath(null));
    return () => {
      current = "";
    };
  }, [artifact.id]);

  const doExport = async () => {
    try {
      // A tab cannot be given a destination path — `save()` answers null there
      // and the button would silently do nothing. Hand the browser a download
      // instead, the way the wiki's export does.
      if (isWebBuild()) {
        await downloadArtifact(artifact.id, artifact.name);
        onError(null);
        return;
      }
      const dest = await save({ defaultPath: artifact.name });
      if (!dest) return;
      await invoke("export_artifact", { id: artifact.id, dest });
      onError(null);
    } catch (e) {
      onError(t("artifacts.export_failed", "导出失败：{{error}}", { error: String(e) }));
    }
  };

  return (
    <div className={styles.detail}>
      <div className={styles.detail_bar}>
        <button className={styles.action} onClick={onBack}>
          ← {t("artifacts.panel_title", "产出")}
        </button>
        <span className={styles.detail_title} title={artifact.name}>
          {artifact.title}
        </span>
        <div className={styles.detail_actions}>
          <button className={styles.action} onClick={doExport}>
            {t("artifacts.export_short", "导出")}
          </button>
          {localPath && (
            <>
              <button className={styles.action} onClick={() => void openPath(localPath)}>
                {t("artifacts.open_with", "用系统应用打开")}
              </button>
              <button
                className={styles.action}
                onClick={() => void revealItemInDir(localPath)}
              >
                {t("artifacts.reveal", "在访达中显示")}
              </button>
            </>
          )}
          <button
            className={`${styles.action} ${styles.action_danger}`}
            onClick={async () => {
              if (
                !window.confirm(
                  t("artifacts.delete_confirm", "删除「{{title}}」？", {
                    title: artifact.title,
                  }),
                )
              ) {
                return;
              }
              try {
                await invoke("delete_artifact", { id: artifact.id });
                onDeleted();
              } catch (e) {
                onError(String(e));
              }
            }}
          >
            {t("artifacts.delete", "删除产出")}
          </button>
        </div>
      </div>

      <ArtifactStage artifact={artifact} />

      <div className={styles.detail_meta}>
        {artifact.drifted && (
          <div className={styles.drift_banner}>
            <TriangleAlert size={13} />
            <span>
              <b>{t("artifacts.drifted", "源文件已被改写")}</b> —{" "}
              {t("artifacts.drifted_hint", "入库时是硬链接，之后源文件被就地重写。")}
            </span>
          </div>
        )}
        <div className={styles.meta_row}>
          <span>{formatBytes(artifact.sizeBytes)}</span>
          <span>{artifact.mime}</span>
          <span title={artifact.workspacePath}>{artifact.workspaceName}</span>
          <span>{new Date(artifact.createdMs).toLocaleString()}</span>
        </div>
        <textarea
          className={styles.note_input}
          value={note}
          placeholder={t("artifacts.note_placeholder", "这份产出是什么、给谁的…")}
          onChange={(e) => setNote(e.target.value)}
          // Commit on blur, not on every keystroke: each save is a disk write
          // (and an HTTP round trip on a remote workspace).
          onBlur={() => {
            if (note !== artifact.note) onPatch(artifact.id, { note });
          }}
        />
      </div>
    </div>
  );
}

/**
 * The preview surface. Which element renders is decided by `kind`, plus one
 * sub-split inside the `text` bucket (see `textPreviewMode`) so a markdown or
 * html deliverable is shown rendered instead of as source. Both come off the
 * mime the store already derived, so no component sniffs extensions itself.
 *
 * `<video>` and the PDF `<iframe>` both point at `fleet-artifact://`, which is
 * the protocol that honours `Range`; that is what makes seeking work rather
 * than re-downloading. The webview has no Office viewer of its own — an
 * `<iframe>` at a .docx renders a blank frame — so the OOXML three get one in
 * JavaScript, lazily (see `OfficePreview`). Everything left over (legacy .doc /
 * .xls / .ppt, ODF, archives) still gets the typed placeholder with 导出 / 打开
 * one click away in the bar above.
 */
function ArtifactStage({ artifact }: { artifact: Artifact }) {
  const { t } = useTranslation();
  const [text, setText] = useState<string | null>(null);
  const url = artifactBlobUrl(artifact.id, artifact.name);
  // html goes to the frame by URL, so only the two rendered-from-source modes
  // pull the bytes into React.
  const textMode = artifact.kind === "text" ? textPreviewMode(artifact.mime) : null;
  const needsBody = textMode === "markdown" || textMode === "plain";

  useEffect(() => {
    if (!needsBody) {
      setText(null);
      return;
    }
    let alive = true;
    fetch(url)
      .then((r) => r.text())
      .then((body) => {
        if (alive) setText(body);
      })
      .catch(() => {
        if (alive) setText(null);
      });
    return () => {
      alive = false;
    };
  }, [artifact.id, needsBody, url]);

  if (artifact.kind === "image") {
    return (
      <div className={styles.stage}>
        <img src={url} alt={artifact.title} />
      </div>
    );
  }
  if (artifact.kind === "video") {
    return (
      <div className={styles.stage}>
        <video src={url} controls preload="metadata" />
      </div>
    );
  }
  if (artifact.kind === "audio") {
    return (
      <div className={styles.stage}>
        <audio src={url} controls />
      </div>
    );
  }
  if (artifact.kind === "pdf") {
    return (
      <div className={styles.stage}>
        <iframe className={styles.doc_frame} src={url} title={artifact.title} />
      </div>
    );
  }
  if (textMode === "html") {
    // allow-scripts but NOT allow-same-origin: an agent-produced page may run
    // its own JS while staying a cross-origin document with no reach into
    // Tauri IPC. Same policy as the wiki's html frame.
    return (
      <div className={styles.stage}>
        <iframe
          className={styles.doc_frame}
          sandbox="allow-scripts"
          src={url}
          title={artifact.title}
        />
      </div>
    );
  }
  if (textMode === "markdown") {
    return (
      <div className={styles.stage}>
        <div className={styles.markdown_body}>
          {text === null ? t("artifacts.loading", "加载中…") : <TextBlock text={text} />}
        </div>
      </div>
    );
  }
  if (textMode === "plain") {
    return (
      <div className={styles.stage}>
        <pre className={styles.text_pre}>{text ?? t("artifacts.loading", "加载中…")}</pre>
      </div>
    );
  }
  const office = officeMode(artifact.mime);
  if (office) {
    return (
      <div className={`${styles.stage} ${styles.stage_office}`}>
        <Suspense fallback={<div className={styles.no_preview_hint}>{t("artifacts.loading", "加载中…")}</div>}>
          <OfficePreview mode={office} url={url} title={artifact.title} />
        </Suspense>
      </div>
    );
  }
  const Icon = KIND_ICON[artifact.kind] ?? FileText;
  return (
    <div className={styles.stage}>
      <div className={styles.no_preview}>
        <Icon size={40} strokeWidth={1.1} />
        <div className={styles.no_preview_title}>
          {t("artifacts.no_preview_title", "这个格式没法在这里预览")}
        </div>
        <div className={styles.no_preview_hint}>
          {/* docx/xlsx/pptx now render above; what lands here is the legacy
              binary Office formats, ODF, archives and unknown blobs. */}
          {t("artifacts.no_preview_hint", "这个格式只能导出，或者用系统应用打开。")}
        </div>
      </div>
    </div>
  );
}
