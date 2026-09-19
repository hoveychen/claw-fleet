// Browse a .zip artifact as a folder (mobile version).
//
// Uses the same parsing layer as the desktop (shared-ts/zipDir.ts), but retrieves
// bytes differently — worth clarifying: desktop can issue Range requests for artifacts,
// so it can read just the zip's central directory at the tail and fetch individual
// members on demand; 2GB zips are browsable. Relay has only one byte shape: single-frame
// base64, capped at 16 MiB (mobile_relay::MAX_ARTIFACT_FRAME_BYTES), so on mobile the
// entire package is already loaded into memory before bufferReader parses it locally.
// Zips exceeding the limit and other huge artifacts stop at the "export to desktop"
// card — not a new limitation added here.
//
// Member previews delegate to PreviewBody, the same dispatcher used for loose artifacts.

import { useEffect, useMemo, useState } from "react";
import {
  ChevronRight,
  FileSpreadsheet,
  FileText,
  FileType,
  Film,
  Folder,
  Image as ImageIcon,
  Lock,
  Music,
  Package,
  Search,
  Share2,
} from "lucide-react";

import { t } from "../i18n";
import { formatBytes, previewKindFor } from "../artifacts";
import {
  buildZipTree,
  bufferReader,
  readZipEntries,
  readZipEntryBytes,
  ZipError,
  zipDirAt,
  zipEntryKind,
  zipEntryMime,
  type ZipDir,
  type ZipEntry,
} from "../../../shared-ts/zipDir";
import { PreviewBody, type PreviewSource } from "./ArtifactPreviewBody";
import styles from "./ZipBrowser.module.css";
import artifactStyles from "./ArtifactsView.module.css";

const KIND_ICON: Record<string, typeof FileText> = {
  image: ImageIcon,
  video: Film,
  audio: Music,
  pdf: FileType,
  doc: FileText,
  sheet: FileSpreadsheet,
  slides: FileText,
  archive: Package,
  text: FileText,
};

/** Placeholder state fed to PreviewBody when member bytes haven't loaded yet — falls through directly to fallback. */
const EMPTY_SOURCE: PreviewSource = {
  kind: "none",
  title: "",
  blobUrl: null,
  blob: null,
  text: null,
};

/** Member timestamp, short enough to fit in one line. Zip stores DOS time in local
 *  timezone with 2-second precision, so display only goes to minute granularity;
 *  some writers don't record a timestamp at all. */
function rowTime(ms: number | null): string {
  if (ms === null) return "";
  return new Date(ms).toLocaleString(undefined, { dateStyle: "short", timeStyle: "short" });
}

function describe(e: unknown): string {
  if (e instanceof ZipError) {
    switch (e.code) {
      case "not-zip":
        return t("这个文件不是有效的 zip，或者已损坏。");
      case "encrypted":
        return t("这一项有密码保护，没法在这里打开。");
      case "unsupported-method":
        return t("这一项用了不支持的压缩算法。");
      case "no-inflate":
        return t("当前环境不支持解压。");
      default:
        return t("读取压缩包失败。");
    }
  }
  return e instanceof Error ? e.message : String(e);
}

/** One file row. Mobile width doesn't fit "name | time | size" in three columns,
 *  so time and size move to a second line under the name — this is mobile's layout,
 *  not a desktop row squeezed into mobile. */
function FileRow({
  entry,
  label,
  onOpen,
}: {
  entry: ZipEntry;
  /** Directory browse shows filename; search hits show full path. */
  label: string;
  onOpen: (e: ZipEntry) => void;
}) {
  const Icon = KIND_ICON[zipEntryKind(zipEntryMime(entry.name), entry.name)] ?? FileText;
  const sub = [rowTime(entry.modifiedMs), formatBytes(entry.size)].filter(Boolean).join(" · ");
  return (
    <button className={styles.row} onClick={() => onOpen(entry)}>
      {entry.encrypted ? (
        <Lock size={16} className={styles.rowIcon} />
      ) : (
        <Icon size={16} className={styles.rowIcon} />
      )}
      <span className={styles.rowText}>
        <span className={styles.rowName}>{label}</span>
        <span className={styles.rowSub}>{sub}</span>
      </span>
    </button>
  );
}

export function ZipBrowser({
  bytes,
  onShareMember,
}: {
  /** The entire zip package, already fetched into memory via relay. */
  bytes: Uint8Array;
  /** Pass a member to the system share panel / download. */
  onShareMember?: (name: string, bytes: Uint8Array) => void;
}) {
  const [entries, setEntries] = useState<ZipEntry[] | null>(null);
  const [listError, setListError] = useState<string | null>(null);
  const [cwd, setCwd] = useState("");
  const [query, setQuery] = useState("");
  const [open, setOpen] = useState<ZipEntry | null>(null);
  const [memberBytes, setMemberBytes] = useState<Uint8Array | null>(null);
  const [memberError, setMemberError] = useState<string | null>(null);
  const [memberSource, setMemberSource] = useState<PreviewSource | null>(null);

  const reader = useMemo(() => bufferReader(bytes), [bytes]);

  useEffect(() => {
    let alive = true;
    setEntries(null);
    setListError(null);
    setCwd("");
    setQuery("");
    setOpen(null);
    readZipEntries(reader)
      .then((list) => {
        if (alive) setEntries(list);
      })
      .catch((e) => {
        if (alive) setListError(describe(e));
      });
    return () => {
      alive = false;
    };
  }, [reader]);

  const root: ZipDir | null = useMemo(
    () => (entries ? buildZipTree(entries) : null),
    [entries],
  );

  /** Search covers the entire package, not just the current level — same rationale
   *  as desktop: the files worth searching for are the deep ones you don't want to
   *  navigate to. Hits show only files with full paths. */
  const hits = useMemo(() => {
    const needle = query.trim().toLowerCase();
    if (!needle || !entries) return null;
    return entries
      .filter((e) => !e.isDir && e.path.toLowerCase().includes(needle))
      .sort((a, b) => a.path.localeCompare(b.path, undefined, { sensitivity: "base" }));
  }, [entries, query]);

  useEffect(() => {
    setMemberBytes(null);
    setMemberError(null);
    setMemberSource(null);
    if (!open) return;
    let alive = true;
    let url: string | null = null;
    readZipEntryBytes(reader, open)
      .then((out) => {
        if (!alive) return;
        setMemberBytes(out);
        const mime = zipEntryMime(open.name);
        const kind = previewKindFor(zipEntryKind(mime, open.name), mime);
        // Each kind produces only its own representation: markdown/html/text read
        // from the decoded string, image/pdf need blob URL, Office suite need Blob
        // itself. Preparing both wastes memory on mobile.
        // A media member needs no buffering step: unzipping already put the
        // whole clip in memory, so it goes straight to a blob URL like an image.
        if (kind === "image" || kind === "pdf" || kind === "media") {
          url = URL.createObjectURL(new Blob([out as BlobPart], { type: mime }));
        }
        setMemberSource({
          kind,
          title: open.name,
          mime,
          blobUrl: url,
          blob:
            kind === "docx" || kind === "xlsx" || kind === "pptx"
              ? new Blob([out as BlobPart], { type: mime })
              : null,
          text:
            kind === "markdown" || kind === "html" || kind === "text"
              ? new TextDecoder().decode(out)
              : null,
        });
      })
      .catch((e) => {
        if (alive) setMemberError(describe(e));
      });
    return () => {
      alive = false;
      // Revoked rather than left to GC, same as the artifact preview: a few
      // full-size images is real memory on a phone.
      if (url) URL.revokeObjectURL(url);
    };
  }, [open, reader]);

  if (listError) {
    return (
      <div className={artifactStyles.noPreview}>
        <Package size={34} strokeWidth={1.1} />
        <div className={artifactStyles.noPreviewTitle}>{listError}</div>
      </div>
    );
  }
  const dir = root ? zipDirAt(root, cwd) : null;
  if (!dir) {
    return (
      <div className={artifactStyles.noPreview}>
        <div className={artifactStyles.noPreviewHint}>{t("加载中…")}</div>
      </div>
    );
  }

  // ── Member opened ─────────────────────────────────────────────────────────────
  if (open) {
    return (
      <div className={styles.root}>
        <div className={styles.bar}>
          <button className={styles.crumb} onClick={() => setOpen(null)}>
            {t("返回")}
          </button>
          <ChevronRight size={13} className={styles.sep} />
          <span className={styles.crumbCurrent}>{open.name}</span>
          <span className={styles.barSpacer} />
          {onShareMember && memberBytes && (
            <button
              className={styles.barAction}
              onClick={() => onShareMember(open.name, memberBytes)}
            >
              <Share2 size={13} />
              {t("分享")}
            </button>
          )}
        </div>
        <div className={styles.body}>
          <PreviewBody
            src={memberSource ?? EMPTY_SOURCE}
            fallback={
              <div className={artifactStyles.noPreview}>
                <div className={artifactStyles.noPreviewTitle}>
                  {memberError ?? (memberSource ? t("这个格式手机上看不了") : t("加载中…"))}
                </div>
                {memberSource && !memberError && (
                  <div className={artifactStyles.noPreviewHint}>
                    {t("可以分享出去，或到桌面端用系统应用打开。")}
                  </div>
                )}
              </div>
            }
          />
        </div>
      </div>
    );
  }

  // ── Directory listing ───────────────────────────────────────────────────────
  const crumbs = cwd ? cwd.split("/") : [];
  return (
    <div className={styles.root}>
      <div className={styles.bar}>
        <button className={styles.crumb} onClick={() => setCwd("")} disabled={!cwd}>
          <Package size={13} />
          {t("压缩包")}
        </button>
        {crumbs.map((segment, i) => (
          <span key={i} className={styles.crumbGroup}>
            <ChevronRight size={13} className={styles.sep} />
            {i === crumbs.length - 1 ? (
              <span className={styles.crumbCurrent}>{segment}</span>
            ) : (
              <button
                className={styles.crumb}
                onClick={() => setCwd(crumbs.slice(0, i + 1).join("/"))}
              >
                {segment}
              </button>
            )}
          </span>
        ))}
      </div>
      <div className={styles.searchRow}>
        <Search size={14} className={styles.searchIcon} />
        <input
          className={styles.search}
          type="search"
          value={query}
          placeholder={t("在包里搜索…")}
          onChange={(e) => setQuery(e.target.value)}
        />
      </div>
      <div className={styles.body}>
        {hits ? (
          hits.length === 0 ? (
            <div className={artifactStyles.noPreview}>
              <div className={artifactStyles.noPreviewHint}>{t("包里没有匹配的文件。")}</div>
            </div>
          ) : (
            <ul className={styles.rows}>
              {hits.map((f) => (
                <li key={`h:${f.path}`}>
                  <FileRow entry={f} onOpen={setOpen} label={f.path} />
                </li>
              ))}
            </ul>
          )
        ) : dir.dirs.length === 0 && dir.files.length === 0 ? (
          <div className={artifactStyles.noPreview}>
            <div className={artifactStyles.noPreviewHint}>{t("这个文件夹是空的。")}</div>
          </div>
        ) : (
          <ul className={styles.rows}>
            {dir.dirs.map((d) => (
              <li key={`d:${d.path}`}>
                <button className={styles.row} onClick={() => setCwd(d.path)}>
                  <Folder size={16} className={styles.rowIcon} />
                  <span className={styles.rowText}>
                    <span className={styles.rowName}>{d.name}</span>
                    <span className={styles.rowSub}>
                      {t("{0} 项", d.dirs.length + d.files.length)}
                    </span>
                  </span>
                </button>
              </li>
            ))}
            {dir.files.map((f) => (
              <li key={`f:${f.path}`}>
                <FileRow entry={f} onOpen={setOpen} label={f.name} />
              </li>
            ))}
          </ul>
        )}
      </div>
    </div>
  );
}

export default ZipBrowser;
