import { useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { invoke } from "@tauri-apps/api/core";
import { writeText } from "@tauri-apps/plugin-clipboard-manager";
import { Copy, FileJson2, Folder, FolderOpen, MoreHorizontal } from "lucide-react";
import { ContextMenu, type ContextMenuAnchor, type ContextMenuItem } from "./ContextMenu";
import styles from "./SessionHeaderMenu.module.css";

/**
 * The detail header's overflow menu: everything about a session that is a
 * copy-when-you-need-it identifier rather than something to watch — the session
 * id, its transcript path, the workspace path.
 *
 * They used to sit in the header as two permanent rows; here they cost nothing
 * until asked for, and the menu doubles as the affordance that tells you where
 * they went.
 */
export function SessionHeaderMenu({
  sessionId,
  jsonlPath,
  workspacePath,
  isLocal,
}: {
  sessionId: string;
  jsonlPath: string;
  workspacePath: string;
  isLocal: boolean;
}) {
  const { t } = useTranslation();
  const btnRef = useRef<HTMLButtonElement>(null);
  const [anchor, setAnchor] = useState<ContextMenuAnchor | null>(null);

  // A failed clipboard write (Tauri ACL) must not read as success, but the menu
  // is gone by then — so surface it on the button itself.
  const [failed, setFailed] = useState(false);
  const copy = (text: string) => {
    writeText(text).catch(() => {
      setFailed(true);
      setTimeout(() => setFailed(false), 2000);
    });
  };

  const revealKey =
    document.documentElement.getAttribute("data-platform") === "windows"
      ? "paths.reveal_in_explorer"
      : "paths.reveal_in_finder";

  const items: ContextMenuItem[] = [
    {
      id: "copy-id",
      label: t("detail.copy_session_id"),
      sub: sessionId,
      icon: <Copy size={13} />,
      onSelect: () => copy(sessionId),
    },
    {
      id: "copy-transcript",
      label: t("detail.copy_transcript_path"),
      sub: jsonlPath,
      icon: <FileJson2 size={13} />,
      onSelect: () => copy(jsonlPath),
    },
    {
      id: "copy-workspace",
      label: t("detail.copy_workspace_path"),
      sub: workspacePath,
      icon: <Folder size={13} />,
      onSelect: () => copy(workspacePath),
    },
  ];

  // A remote workspace's files are not on this machine — nothing to reveal.
  if (isLocal) {
    items.push({
      id: "reveal",
      label: t(revealKey),
      icon: <FolderOpen size={13} />,
      onSelect: () => {
        invoke("reveal_path", { path: workspacePath }).catch(() => {
          setFailed(true);
          setTimeout(() => setFailed(false), 2000);
        });
      },
    });
  }

  return (
    <>
      <button
        ref={btnRef}
        type="button"
        className={`${styles.btn} ${anchor ? styles.btn_open : ""} ${failed ? styles.btn_failed : ""}`}
        title={t("detail.more")}
        aria-label={t("detail.more")}
        aria-haspopup="menu"
        aria-expanded={anchor != null}
        onClick={() => {
          if (anchor) {
            setAnchor(null);
            return;
          }
          const r = btnRef.current?.getBoundingClientRect();
          if (!r) return;
          // Hang it off the button, not the cursor. ContextMenu clamps to the
          // viewport, so a button near the right edge flips the menu inward.
          setAnchor({ x: r.left, y: r.bottom + 4 });
        }}
      >
        <MoreHorizontal size={15} />
      </button>
      {anchor && (
        <ContextMenu anchor={anchor} items={items} onClose={() => setAnchor(null)} />
      )}
    </>
  );
}
