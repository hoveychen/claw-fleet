// 把一份 .zip 产出当文件夹浏览（手机版）。
//
// 与桌面端同一个解析层（shared-ts/zipDir.ts），但取字节的方式不同，这一点值得
// 说清楚：桌面端能对产出发 Range 请求，所以能只读压缩包尾部的中央目录、按需
// 取某一个成员，2GB 也浏览得动；relay 传字节只有「单帧 base64」一种形状，上限
// 16 MiB（mobile_relay::MAX_ARTIFACT_FRAME_BYTES），所以手机上是整包已经取到
// 内存里之后，再用 bufferReader 在本地解析。超过上限的压缩包和别的超大产出一
// 样，停在「到桌面端导出」那张卡上——不是这里新加的限制。
//
// 成员的预览交给 PreviewBody，也就是散装产出用的同一个分派。

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

/** 成员还没读出来时喂给 PreviewBody 的空形态——它会直接落到 fallback。 */
const EMPTY_SOURCE: PreviewSource = {
  kind: "none",
  title: "",
  blobUrl: null,
  blob: null,
  text: null,
};

/** 成员的时间戳，短到能塞进一行。zip 存的是本地时区的 DOS 时间、精度 2 秒，
 *  所以只到分钟；有的写入方压根不写时间戳，那就没有。 */
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

/** 一行文件。手机上宽度不够摆「名字 | 时间 | 大小」三列，所以时间和大小挪到
 *  名字下面那行——这是手机的读法，不是把桌面端那排硬塞进来。 */
function FileRow({
  entry,
  label,
  onOpen,
}: {
  entry: ZipEntry;
  /** 目录浏览时是文件名，搜索命中时是完整路径。 */
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
  /** 整个压缩包，已经过 relay 取到内存里。 */
  bytes: Uint8Array;
  /** 把某一个成员交给系统分享面板 / 下载。 */
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

  /** 搜的是整个包而不是当前这层——理由与桌面端同一条：值得搜的正是那个懒得
   *  走过去的深层文件。命中只给文件，并显示完整路径。 */
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
        // 每一类只做出它自己那一种形态：markdown/html/text 从解出来的字符串
        // 读，image/pdf 才要 blob URL，Office 三件套要 Blob 本身。多做的那份
        // 在手机上就是白占内存。
        if (kind === "image" || kind === "pdf") {
          url = URL.createObjectURL(new Blob([out as BlobPart], { type: mime }));
        }
        setMemberSource({
          kind,
          title: open.name,
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

  // ── 打开了一个成员 ─────────────────────────────────────────────────────────
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

  // ── 目录列表 ───────────────────────────────────────────────────────────────
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
