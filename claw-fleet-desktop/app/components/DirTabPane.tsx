import { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { invoke } from "@tauri-apps/api/core";
import { FileText, Folder, Image as ImageIcon } from "lucide-react";

import type { AuxDoc } from "../detailAux";
import { formatBytes } from "../formatBytes";
import { isRenderableImage } from "../userAttachments";
import { AuxDocBar, AuxPane, type AuxFact } from "./AuxDocBar";
import { buildFileMenu, type AuxCardTail } from "./auxDocMenu";
import { useIngestOpen } from "./blocks/ingestOpenContext";
import { SkeletonList } from "./loading";
import styles from "./TabPanes.module.css";
import dirStyles from "./DirTabPane.module.css";

/** Mirrors core `file_explorer::ExternalDirListing`. */
export interface ExternalDirEntry {
  name: string;
  path: string;
  isDir: boolean;
  sizeBytes: number;
  modifiedMs: number;
}

export interface ExternalDirListing {
  path: string;
  entries: ExternalDirEntry[];
  truncated: boolean;
}

/**
 * A folder an agent handed over as material (`reviewDocs` kind `dir`), as an
 * auxiliary-rail reader: its top level, folders first. A row opens in the rail
 * the way a path clicked in prose does — a file as a file card (images render
 * as images there), a sub-folder as another folder card.
 */
export function DirTabPane({ doc, tail }: { doc: AuxDoc; tail: AuxCardTail }) {
  const { t } = useTranslation();
  const ingest = useIngestOpen();
  const [listing, setListing] = useState<ExternalDirListing | null>(null);
  const [error, setError] = useState<string | null>(null);
  const path = doc.ref;

  useEffect(() => {
    let live = true;
    setListing(null);
    setError(null);
    invoke<ExternalDirListing>("list_external_dir", { path })
      .then((l) => live && setListing(l))
      .catch((e) => live && setError(String(e)));
    return () => {
      live = false;
    };
  }, [path]);

  const build = buildFileMenu({ doc, tail, t, fail: setError });

  const facts: AuxFact[] = [{ text: path }];
  if (listing) {
    facts.push({
      text: t("detail.aux_dir_count", "{{count}} 项", { count: listing.entries.length }),
      strong: true,
    });
  } else if (!error) {
    facts.push({ text: "", pending: 40 });
  }

  return (
    <AuxPane menuItems={build.menu} className={styles.pane}>
      <AuxDocBar
        kind="dir"
        icon={<Folder size={14} strokeWidth={1.8} />}
        title={doc.label}
        titleHint={path}
        facts={facts}
        actions={build.actions}
        menuItems={build.menu}
        onCollapse={tail.onToggle}
        onClose={tail.onClose}
      />
      {error && <p className={styles.error_line}>{error}</p>}
      <div className={styles.body}>
        {!listing && !error && <SkeletonList rows={6} />}
        {listing && listing.entries.length === 0 && (
          <p className={dirStyles.empty}>{t("detail.aux_dir_empty", "空文件夹")}</p>
        )}
        {listing && listing.entries.length > 0 && (
          <ul className={dirStyles.list} data-testid="dir-listing">
            {listing.entries.map((e) => (
              <li key={e.path}>
                <button
                  type="button"
                  className={dirStyles.row}
                  title={e.path}
                  disabled={!ingest}
                  onClick={() => ingest?.open(e.isDir ? "dir" : "file", e.path, e.name)}
                >
                  <span className={dirStyles.icon} data-dir={e.isDir || undefined}>
                    {e.isDir ? (
                      <Folder size={14} strokeWidth={1.8} />
                    ) : isRenderableImage(e.name) ? (
                      <ImageIcon size={14} strokeWidth={1.8} />
                    ) : (
                      <FileText size={14} strokeWidth={1.8} />
                    )}
                  </span>
                  <span className={dirStyles.name}>{e.name}</span>
                  {!e.isDir && <span className={dirStyles.meta}>{formatBytes(e.sizeBytes)}</span>}
                </button>
              </li>
            ))}
          </ul>
        )}
        {listing?.truncated && (
          <p className={dirStyles.empty}>
            {t("detail.aux_dir_truncated", "只列出前 {{count}} 项", { count: listing.entries.length })}
          </p>
        )}
      </div>
    </AuxPane>
  );
}
