import { useState, type ReactNode } from "react";
import { invoke } from "@tauri-apps/api/core";
import { useTranslation } from "react-i18next";
import { ContextMenu, type ContextMenuAnchor } from "../components/ContextMenu";
import { resolvePathRef, type PathRef } from "./pathRef";
import styles from "./markdown.module.css";

/**
 * Clickable file paths inside agent prose.
 *
 * Only inline-code spans are considered (see pathRef.ts for why). A span that
 * parses as a path renders as a chip: click opens it in the 文件 page, right
 * click offers "reveal in Finder" — the latter only for a local connection,
 * since a remote workspace's files are not on this machine.
 */

/** "Reveal in Finder" vs "Show in File Explorer" — App.tsx stamps the platform
 *  onto <html> at boot, which is also what the module-CSS selectors key off. */
function revealKey(): string {
  return document.documentElement.getAttribute("data-platform") === "windows"
    ? "paths.reveal_in_explorer"
    : "paths.reveal_in_finder";
}

export interface PathLinkContext {
  /** Workspace root that relative paths resolve against. */
  workspaceRoot: string;
  /** False for a remote connection: Finder cannot reach those files. */
  isLocal: boolean;
  /** Open in the 文件 page. `absPath` is already resolved. */
  openInFiles: (absPath: string, line: number | null) => void;
  /**
   * Paths a previous click could not resolve to any file. Undefined in the
   * decision-float window, which hands the click to the main window and never
   * hears back.
   */
  unresolved?: string[];
}

export function PathChip({
  pathRef,
  ctx,
  children,
}: {
  pathRef: PathRef;
  ctx: PathLinkContext;
  children: ReactNode;
}) {
  const { t } = useTranslation();
  const [menu, setMenu] = useState<ContextMenuAnchor | null>(null);
  const [revealFailed, setRevealFailed] = useState(false);

  // resolvePathRef returns null only for `~` with no home dir. We pass none:
  // a `~` path lies outside the workspace anyway (so the 文件 page can't show
  // it), and reveal_path expands `~` host-side. Keeping it as-written is right.
  const absPath = resolvePathRef(pathRef.path, ctx.workspaceRoot, null) ?? pathRef.path;

  // Two ways a chip learns it is broken: the right-click reveal rejected it, or
  // a previous left click reached the 仓库 page and found nothing there.
  const failed = revealFailed || (ctx.unresolved?.includes(absPath) ?? false);

  // The join above is a *guess*: agents write paths relative to whatever
  // directory they had in mind, which is often a subdirectory of the workspace,
  // so `absPath` may name a file that does not exist. Asserting it in the
  // tooltip ("打开 /Users/…/public/app-icon.png") stated that guess as fact —
  // and the path in the tooltip was the wrong one. Show what was written and
  // where we will look for it, which is all we actually know before the click.
  const hint = failed
    ? t("paths.not_found_hint", { path: absPath })
    : pathRef.path === absPath
      ? t("paths.open_hint", { path: absPath })
      : t("paths.open_hint_relative", { path: pathRef.path, root: ctx.workspaceRoot });

  const reveal = () => {
    invoke("reveal_path", { path: absPath }).catch(() => {
      // Most often: the path the agent named no longer exists (or never did).
      setRevealFailed(true);
      setTimeout(() => setRevealFailed(false), 2000);
    });
  };

  return (
    <>
      <code
        className={`${styles.path_chip} ${failed ? styles.path_chip_failed : ""}`}
        role="button"
        tabIndex={0}
        title={hint}
        onClick={() => ctx.openInFiles(absPath, pathRef.line)}
        onKeyDown={(e) => {
          if (e.key === "Enter" || e.key === " ") {
            e.preventDefault();
            ctx.openInFiles(absPath, pathRef.line);
          }
        }}
        onContextMenu={(e) => {
          if (!ctx.isLocal) return; // fall through to the app-wide menu
          e.preventDefault();
          setMenu({ x: e.clientX, y: e.clientY });
        }}
      >
        {children}
      </code>
      {menu && (
        <ContextMenu
          anchor={menu}
          onClose={() => setMenu(null)}
          items={[
            {
              id: "open",
              label: t("paths.open_in_files"),
              onSelect: () => ctx.openInFiles(absPath, pathRef.line),
            },
            { id: "reveal", label: t(revealKey()), onSelect: reveal },
          ]}
        />
      )}
    </>
  );
}
