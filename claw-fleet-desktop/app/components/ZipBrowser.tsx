/**
 * Browse a .zip artifact as if it were a folder.
 *
 * Mounted by the 产出 page's preview stage in place of the "this format can't
 * be previewed here" placeholder. Everything it knows about the archive comes
 * from `shared-ts/zipDir.ts`, which reads through `Range` requests — listing a
 * 2 GB artifact costs a couple of KB, and opening one member costs that member.
 *
 * Previewing a member is deliberately *not* implemented here: the page passes
 * `renderPreview`, and what it passes is the same dispatch that renders a
 * loose artifact. A member and an artifact of the same type must look the
 * same, and the way to guarantee that is to have one renderer, not two that
 * agree today.
 */
import {
  ChevronRight,
  Download,
  File as FileIcon,
  FileSpreadsheet,
  FileText,
  FileType,
  Film,
  Folder,
  Image as ImageIcon,
  Lock,
  Music,
  Package,
} from "lucide-react";
import { useCallback, useEffect, useMemo, useRef, useState, type ReactNode } from "react";
import { useTranslation } from "react-i18next";

import { formatBytes } from "../formatBytes";
import {
  buildZipTree,
  rangeReader,
  readZipEntries,
  readZipEntryBytes,
  ZipError,
  zipDirAt,
  zipEntryKind,
  zipEntryMime,
  type ByteReader,
  type ZipDir,
  type ZipEntry,
} from "../../../shared-ts/zipDir";
import styles from "./ZipBrowser.module.css";

/** What the page needs to render one member with its artifact-level renderer. */
export interface ZipMemberPreview {
  /** A `blob:` URL for the member's decompressed bytes. */
  url: string;
  mime: string;
  /** The store's coarse bucket, so the page can reuse its existing branches. */
  kind: string;
  title: string;
}

/**
 * Above this, a member is not inflated on click.
 *
 * Not a performance hedge — inflating happens in memory, and a 500 MB member
 * inside a 600 MB archive would be held twice (compressed slice + inflated
 * result) with a `blob:` on top. The export button reaches the same bytes
 * without any of that, so the honest move is to offer it instead.
 */
const MAX_INLINE_BYTES = 50 * 1024 * 1024;

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

interface Loaded {
  reader: ByteReader;
  entries: ZipEntry[];
  root: ZipDir;
}

export function ZipBrowser({
  url,
  size,
  renderPreview,
  onExportMember,
}: {
  /** The artifact blob's URL — the one that answers `Range`. */
  url: string;
  /** The artifact's `sizeBytes`. Known from the record, so the browser never
   *  spends a request discovering it. */
  size: number;
  renderPreview: (member: ZipMemberPreview) => ReactNode;
  /** Save one member to disk. Omitted in hosts that cannot write files. */
  onExportMember?: (name: string, bytes: Uint8Array) => void | Promise<void>;
}) {
  const { t } = useTranslation();
  const [loaded, setLoaded] = useState<Loaded | null>(null);
  const [listError, setListError] = useState<string | null>(null);
  const [cwd, setCwd] = useState("");
  const [open, setOpen] = useState<ZipEntry | null>(null);
  const [member, setMember] = useState<ZipMemberPreview | null>(null);
  const [memberError, setMemberError] = useState<string | null>(null);
  const [memberBytes, setMemberBytes] = useState<Uint8Array | null>(null);
  // Every blob: URL handed out has to be revoked, or the archive stays in
  // memory for the life of the window.
  const objectUrl = useRef<string | null>(null);

  const message = useCallback(
    (e: unknown): string => {
      if (e instanceof ZipError) {
        switch (e.code) {
          case "not-zip":
            return t("artifacts.zip.err_not_zip", "这个文件不是有效的 zip，或者已损坏。");
          case "encrypted":
            return t("artifacts.zip.err_encrypted", "这一项有密码保护，没法在这里打开。");
          case "unsupported-method":
            return t("artifacts.zip.err_method", "这一项用了不支持的压缩算法。");
          case "no-inflate":
            return t("artifacts.zip.err_no_inflate", "当前环境不支持解压，只能导出。");
          default:
            return t("artifacts.zip.err_read", "读取压缩包失败。");
        }
      }
      return e instanceof Error ? e.message : String(e);
    },
    [t],
  );

  // ── Listing ────────────────────────────────────────────────────────────────
  useEffect(() => {
    let alive = true;
    setLoaded(null);
    setListError(null);
    setCwd("");
    setOpen(null);
    const reader = rangeReader(url, size);
    readZipEntries(reader)
      .then((entries) => {
        if (alive) setLoaded({ reader, entries, root: buildZipTree(entries) });
      })
      .catch((e) => {
        if (alive) setListError(message(e));
      });
    return () => {
      alive = false;
    };
  }, [url, size, message]);

  // ── Opening one member ─────────────────────────────────────────────────────
  useEffect(() => {
    const revoke = () => {
      if (objectUrl.current) {
        URL.revokeObjectURL(objectUrl.current);
        objectUrl.current = null;
      }
    };
    if (!open || !loaded) {
      revoke();
      setMember(null);
      setMemberBytes(null);
      setMemberError(null);
      return;
    }
    let alive = true;
    setMember(null);
    setMemberBytes(null);
    setMemberError(null);
    readZipEntryBytes(loaded.reader, open)
      .then((bytes) => {
        if (!alive) return;
        setMemberBytes(bytes);
        if (open.size > MAX_INLINE_BYTES) return;
        const mime = zipEntryMime(open.name);
        revoke();
        const blobUrl = URL.createObjectURL(new Blob([bytes as BlobPart], { type: mime }));
        objectUrl.current = blobUrl;
        setMember({ url: blobUrl, mime, kind: zipEntryKind(mime, open.name), title: open.name });
      })
      .catch((e) => {
        if (alive) setMemberError(message(e));
      });
    return () => {
      alive = false;
    };
  }, [open, loaded, message]);

  useEffect(() => () => {
    if (objectUrl.current) URL.revokeObjectURL(objectUrl.current);
  }, []);

  const dir = useMemo(
    () => (loaded ? zipDirAt(loaded.root, cwd) : null),
    [loaded, cwd],
  );

  if (listError) {
    return (
      <div className={styles.centered}>
        <Package size={40} strokeWidth={1.1} />
        <div className={styles.centered_title}>{listError}</div>
      </div>
    );
  }
  if (!loaded || !dir) {
    return <div className={styles.centered}>{t("artifacts.loading", "加载中…")}</div>;
  }

  const crumbs = cwd ? cwd.split("/") : [];

  // ── One member, open ───────────────────────────────────────────────────────
  if (open) {
    const tooBig = open.size > MAX_INLINE_BYTES;
    return (
      <div className={styles.root}>
        <div className={styles.bar}>
          <button className={styles.crumb} onClick={() => setOpen(null)}>
            {t("artifacts.zip.back", "返回")}
          </button>
          <ChevronRight size={13} className={styles.sep} />
          <span className={styles.crumb_current}>{open.path}</span>
          <span className={styles.bar_spacer} />
          <span className={styles.bar_size}>{formatBytes(open.size)}</span>
          {onExportMember && memberBytes && (
            <button
              className={styles.bar_action}
              onClick={() => void onExportMember(open.name, memberBytes)}
            >
              <Download size={13} />
              {t("artifacts.zip.export_member", "导出这一项")}
            </button>
          )}
        </div>
        <div className={styles.body}>
          {memberError ? (
            <div className={styles.centered}>
              <Lock size={40} strokeWidth={1.1} />
              <div className={styles.centered_title}>{memberError}</div>
            </div>
          ) : tooBig ? (
            <div className={styles.centered}>
              <FileIcon size={40} strokeWidth={1.1} />
              <div className={styles.centered_title}>
                {t("artifacts.zip.too_big", "这一项太大了，不在这里展开。")}
              </div>
              <div className={styles.centered_hint}>
                {t("artifacts.zip.too_big_hint", "用上面的「导出这一项」把它存到本地再打开。")}
              </div>
            </div>
          ) : member ? (
            renderPreview(member)
          ) : (
            <div className={styles.centered}>{t("artifacts.loading", "加载中…")}</div>
          )}
        </div>
      </div>
    );
  }

  // ── Listing ────────────────────────────────────────────────────────────────
  return (
    <div className={styles.root}>
      <div className={styles.bar}>
        <button className={styles.crumb} onClick={() => setCwd("")} disabled={!cwd}>
          <Package size={13} />
          {t("artifacts.zip.root", "压缩包")}
        </button>
        {crumbs.map((segment, i) => (
          <span key={i} className={styles.crumb_group}>
            <ChevronRight size={13} className={styles.sep} />
            {i === crumbs.length - 1 ? (
              <span className={styles.crumb_current}>{segment}</span>
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
        <span className={styles.bar_spacer} />
        <span className={styles.bar_size}>
          {t("artifacts.zip.count", "{{count}} 项", {
            count: dir.dirs.length + dir.files.length,
          })}
        </span>
      </div>
      <div className={styles.body}>
        {dir.dirs.length === 0 && dir.files.length === 0 ? (
          <div className={styles.centered}>
            {t("artifacts.zip.empty", "这个文件夹是空的。")}
          </div>
        ) : (
          <ul className={styles.rows}>
            {dir.dirs.map((d) => (
              <li key={`d:${d.path}`}>
                <button className={styles.row} onClick={() => setCwd(d.path)}>
                  <Folder size={15} className={styles.row_icon} />
                  <span className={styles.row_name}>{d.name}</span>
                  <span className={styles.row_meta}>
                    {t("artifacts.zip.count", "{{count}} 项", {
                      count: d.dirs.length + d.files.length,
                    })}
                  </span>
                </button>
              </li>
            ))}
            {dir.files.map((f) => {
              const Icon = KIND_ICON[zipEntryKind(zipEntryMime(f.name), f.name)] ?? FileIcon;
              return (
                <li key={`f:${f.path}`}>
                  <button className={styles.row} onClick={() => setOpen(f)}>
                    {f.encrypted ? (
                      <Lock size={15} className={styles.row_icon} />
                    ) : (
                      <Icon size={15} className={styles.row_icon} />
                    )}
                    <span className={styles.row_name}>{f.name}</span>
                    <span className={styles.row_meta}>{formatBytes(f.size)}</span>
                  </button>
                </li>
              );
            })}
          </ul>
        )}
      </div>
    </div>
  );
}

export default ZipBrowser;
