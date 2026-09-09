import { useState, type ReactNode } from "react";
import { invoke } from "@tauri-apps/api/core";
import { useTranslation } from "react-i18next";
import { ContextMenu, type ContextMenuAnchor } from "../components/ContextMenu";
import { canRevealPath } from "../canReveal";
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
  /**
   * Open in the 文件 page. `absPath` is already resolved.
   *
   * `tried` is passed only when *no* reading of the path existed, and lists
   * every one that was stat'ed — so the surface that ends up showing an error
   * can say where it looked instead of naming one guess.
   */
  openInFiles: (absPath: string, line: number | null, tried?: string[]) => void;
  /**
   * Paths a previous click could not resolve to any file. Undefined on a
   * surface that dispatches the click somewhere it never hears back from.
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

  // Candidates a click stat'ed and found nothing at. Empty until a click has
  // actually failed — see `open` for why this cannot be known before one.
  const [tried, setTried] = useState<string[]>([]);

  // resolvePathRef returns null only for `~` with no home dir. We pass none:
  // a `~` path lies outside the workspace anyway (so the 文件 page can't show
  // it), and reveal_path expands `~` host-side. Keeping it as-written is right.
  const absPath = resolvePathRef(pathRef.path, ctx.workspaceRoot, null) ?? pathRef.path;

  // Three ways a chip learns it is broken: the right-click reveal rejected it,
  // a previous left click reached the 仓库 page and found nothing there, or the
  // click's own resolution came back with no reading that exists.
  const failed = revealFailed || tried.length > 0 || (ctx.unresolved?.includes(absPath) ?? false);

  // The join here is a *guess*: agents write paths relative to whatever
  // directory they had in mind, which is often a subdirectory of the workspace
  // — or its parent — so `absPath` may name a file that does not exist.
  // Asserting it in the tooltip ("打开 /Users/…/public/app-icon.png") stated
  // that guess as fact. Before a click, say what was written and where we will
  // look; after a failed one, say every place we actually looked.
  const hint = tried.length
    ? t("paths.tried_hint", { paths: tried.join("\n") })
    : failed
      ? t("paths.not_found_hint", { path: absPath })
      : pathRef.path === absPath
        ? t("paths.open_hint", { path: absPath })
        : t("paths.open_hint_relative", { path: pathRef.path, root: ctx.workspaceRoot });

  /**
   * Ask the backend which reading of this path exists, then open that one.
   *
   * The chip cannot do this during render: resolving needs `stat`, the webview
   * has no filesystem, and a round trip per chip on a long transcript is not a
   * render-time cost anyone would accept. So the resolution happens exactly
   * once per click, and `absPath` — the plain workspace join — stays the
   * fallback, which keeps every path that already worked working even if the
   * command is unavailable (an older backend, a surface with no host).
   */
  const resolve = () =>
    invoke<{ resolved: string | null; tried: string[] }>("resolve_prose_path", {
      workspace: ctx.workspaceRoot,
      path: pathRef.path,
    })
      .then((r) => {
        setTried(r.resolved ? [] : r.tried);
        return r;
      })
      // An unavailable command must not turn a working chip into a dead one:
      // report the plain join as the resolution, exactly as before this existed.
      .catch(() => ({ resolved: absPath, tried: [] as string[] }));

  const open = () => {
    void resolve().then((r) =>
      ctx.openInFiles(r.resolved ?? absPath, pathRef.line, r.resolved ? undefined : r.tried),
    );
  };

  // Reveal resolves too — it is the same guess, and a "reveal in Finder" on a
  // parent-relative path failed for exactly the reason the preview did.
  const reveal = () => {
    void resolve().then((r) =>
      invoke("reveal_path", { path: r.resolved ?? absPath }).catch(() => {
        // Most often: the path the agent named no longer exists (or never did).
        setRevealFailed(true);
        setTimeout(() => setRevealFailed(false), 2000);
      }),
    );
  };

  return (
    <>
      <code
        className={`${styles.path_chip} ${failed ? styles.path_chip_failed : ""}`}
        role="button"
        tabIndex={0}
        title={hint}
        onClick={open}
        onKeyDown={(e) => {
          if (e.key === "Enter" || e.key === " ") {
            e.preventDefault();
            open();
          }
        }}
        onContextMenu={(e) => {
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
              onSelect: open,
            },
            // Reveal only where a file manager can actually open — see
            // canReveal.ts; in a tab the invoke resolves to null and the click
            // produces nothing at all, not even the failed-path flash.
            ...(canRevealPath()
              ? [{ id: "reveal", label: t(revealKey()), onSelect: reveal }]
              : []),
          ]}
        />
      )}
    </>
  );
}
