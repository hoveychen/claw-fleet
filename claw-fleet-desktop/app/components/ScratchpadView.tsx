import { invoke } from "@tauri-apps/api/core";
import { useCallback, useState } from "react";
import { useTranslation } from "react-i18next";

import { FilePreview, FileTree } from "./ExplorerPane";
import type { ExplorerEntry, ExplorerFileContent } from "./ExplorerPane";
import { useResizableWidth } from "../hooks/useResizableWidth";
import { ResizeHandle } from "./ResizeHandle";
import styles from "./MemoryView.module.css";
import skillStyles from "./SkillsView.module.css";

/// Read-only browser for the private temp directory Claude Code hands this
/// session (and instructs it, in the system prompt, to use instead of /tmp).
/// The root is derived backend-side from workspace + session id, so this works
/// for remote-probe sessions too — the tab only renders when the probe found
/// files there in the first place.
export function ScratchpadView({
  workspace,
  sessionId,
}: {
  workspace: string;
  sessionId: string;
}) {
  const { t } = useTranslation();
  const [activeFile, setActiveFile] = useState<ExplorerEntry | null>(null);
  const {
    width: treeWidth,
    isDragging: treeDragging,
    onMouseDown: onTreeMouseDown,
  } = useResizableWidth("scratchpad-tree-width", { min: 140, max: 480, initial: 220 });

  const loadDir = useCallback(
    async (relPath: string): Promise<ExplorerEntry[] | null> => {
      try {
        return await invoke<ExplorerEntry[]>("list_scratchpad_dir", {
          workspace,
          sessionId,
          relPath,
        });
      } catch {
        return null;
      }
    },
    [workspace, sessionId],
  );

  const readFile = useCallback(
    (relPath: string) =>
      invoke<ExplorerFileContent>("read_scratchpad_file", { workspace, sessionId, relPath }),
    [workspace, sessionId],
  );

  return (
    <div className={skillStyles.detail_split}>
      <aside className={skillStyles.tree_pane} style={{ width: treeWidth }}>
        <div className={skillStyles.tree_label}>{t("detail.scratchpad_tree_label")}</div>
        <FileTree loadDir={loadDir} activeFile={activeFile} onPick={setActiveFile} />

        <ResizeHandle active={treeDragging} onMouseDown={onTreeMouseDown} />
      </aside>

      <div className={styles.detail_body}>
        {activeFile ? (
          <FilePreview file={activeFile} load={readFile} />
        ) : (
          <p className={styles.empty}>{t("detail.scratchpad_hint")}</p>
        )}
      </div>
    </div>
  );
}
