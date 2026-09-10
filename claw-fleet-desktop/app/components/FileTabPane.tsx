import { useCallback, useMemo, useState } from "react";
import { useTranslation } from "react-i18next";
import { invoke } from "@tauri-apps/api/core";
import { FileText } from "lucide-react";

import { useUIStore } from "../store";
import type { AuxDoc } from "../detailAux";
import { AuxDocBar, AuxPane, type AuxFact } from "./AuxDocBar";
import { buildFileMenu, type AuxCardTail } from "./auxDocMenu";
import { FilePreview, type ExplorerEntry, type ExplorerFileContent } from "./ExplorerPane";
import { formatBytes } from "../formatBytes";
import styles from "./TabPanes.module.css";

/** Last path segment, tolerating either separator. */
function basename(p: string): string {
  const cut = Math.max(p.lastIndexOf("/"), p.lastIndexOf("\\"));
  return cut >= 0 ? p.slice(cut + 1) : p;
}

/** Everything before it, kept with its trailing separator so it reads as a
 *  directory rather than as a truncated path. */
function dirname(p: string): string {
  const cut = Math.max(p.lastIndexOf("/"), p.lastIndexOf("\\"));
  return cut >= 0 ? p.slice(0, cut + 1) : "";
}

/**
 * One file as an auxiliary-rail reader.
 *
 * This replaces the rail's use of `FilesView`'s `ExternalFilePreview`, which
 * was written for the 仓库 page's out-of-tree case and wore its assumptions
 * here: it built its `ExplorerEntry` with `sizeBytes: 0` and `modifiedMs: 0`
 * hard-coded, so the header could never print a size even though the read it
 * was about to perform returns one; and it drew its own 关闭 button beside the
 * rail card's ✕, which was the same action twice.
 *
 * The reader itself is still the shared `FilePreview`, so a file looks the same
 * here as on the 仓库 page. Only the chrome is the rail's own — the density line
 * is fed by the settled read (`onLoaded`), which is the only place an
 * out-of-tree file's size and length are known.
 */
export function FileTabPane({
  doc,
  tail,
  workspacePath,
}: {
  doc: AuxDoc;
  tail: AuxCardTail;
  /** The session's repo, needed to hand the path to the 仓库 page. Blank on a
   *  session with no workspace, which drops that one menu item. */
  workspacePath: string;
}) {
  const { t } = useTranslation();
  const requestFileNav = useUIStore((s) => s.requestFileNav);
  const [content, setContent] = useState<ExplorerFileContent | null>(null);
  const [error, setError] = useState<string | null>(null);
  const path = doc.ref;

  // `FilePreview` keys its read off `relativePath`; for a file addressed by an
  // absolute path that path IS the key, and the rest of the entry is display
  // data the reader does not consult.
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

  const build = buildFileMenu({
    doc,
    tail,
    t,
    fail: setError,
    content,
    onOpenInFiles: workspacePath
      ? () => requestFileNav({ workspacePath, absPath: path, line: null })
      : undefined,
  });

  // Facts from the read rather than from a listing: an absolute path clicked out
  // of agent prose has no directory entry behind it.
  const facts: AuxFact[] = [{ text: dirname(path) }];
  if (content) {
    facts.push({ text: formatBytes(content.sizeBytes), strong: true });
    if (content.kind === "text") {
      facts.push({
        text: t("detail.aux_lines", "{{count}} 行", {
          count: content.content.split("\n").length,
        }),
      });
      if (content.truncated) {
        facts.push({ text: t("files.truncated", "已截断") });
      }
    } else if (content.kind === "image") {
      facts.push({ text: content.mime });
    }
  }
  facts.push({ text: t("tabs.file_readonly", "只读预览") });

  return (
    <AuxPane menuItems={build.menu} className={styles.pane}>
      <AuxDocBar
        kind="file"
        icon={<FileText size={14} strokeWidth={1.8} />}
        title={entry.name}
        titleHint={path}
        facts={facts}
        actions={build.actions}
        menuItems={build.menu}
        onCollapse={tail.onToggle}
        onClose={tail.onClose}
      />
      {error && <p className={styles.error_line}>{error}</p>}
      <div className={styles.body}>
        <FilePreview file={entry} load={load} onLoaded={setContent} />
      </div>
    </AuxPane>
  );
}
