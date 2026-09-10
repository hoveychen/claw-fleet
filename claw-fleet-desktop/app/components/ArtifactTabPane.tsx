import { useCallback, useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { invoke } from "@tauri-apps/api/core";
import { save } from "@tauri-apps/plugin-dialog";
import { Package } from "lucide-react";

import { formatBytes } from "../formatBytes";
import { isWebBuild } from "../hostEnv";
import { downloadArtifact } from "../mock/liveProxy";
import { useUIStore } from "../store";
import type { AuxDoc } from "../detailAux";
import { AuxDocBar, AuxPane } from "./AuxDocBar";
import { buildArtifactMenu, type AuxCardTail } from "./auxDocMenu";
import { loadArtifact } from "./blocks/ingestLookup";
import { ArtifactStage, type Artifact } from "./ArtifactsView";
import { timeAgo } from "./SessionRow";
import styles from "./TabPanes.module.css";

/**
 * One deliverable as an auxiliary-rail reader.
 *
 * The body is the very `ArtifactStage` the 产出 page renders, so a deliverable
 * looks the same wherever it is open. What changed is the header: it used to
 * name the kind and the size and offer a single 产出 button, while
 * `export_artifact`, `reveal_artifact`, `open_artifact_external` and
 * `delete_artifact` — all four already implemented — were unreachable from
 * here. They now live in the shared `AuxDocBar` and in the card's right-click
 * menu, which are one list (see `auxDocMenu`).
 *
 * The 产出 page still owns rename, move and version rollback, which need its
 * dialogs; 在产出页打开 is the way over to them.
 */
export function ArtifactTabPane({ doc, tail }: { doc: AuxDoc; tail: AuxCardTail }) {
  const { t } = useTranslation();
  const requestArtifactNav = useUIStore((s) => s.requestArtifactNav);
  const [artifact, setArtifact] = useState<Artifact | null>(null);
  const [loaded, setLoaded] = useState(false);
  const [exporting, setExporting] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const id = doc.ref;

  useEffect(() => {
    let alive = true;
    setLoaded(false);
    void loadArtifact(id).then((a) => {
      if (!alive) return;
      setArtifact(a);
      setLoaded(true);
    });
    return () => {
      alive = false;
    };
  }, [id]);

  const title = artifact?.title ?? doc.label;

  const doExport = useCallback(async () => {
    if (!artifact) return;
    setExporting(true);
    try {
      // A browser tab cannot be given a destination path — `save()` answers
      // null there and the action would silently do nothing. Hand the browser a
      // download instead, the same way 产出 and the wiki's export do.
      if (isWebBuild()) {
        await downloadArtifact(artifact.id, artifact.name);
        setError(null);
        return;
      }
      const dest = await save({ defaultPath: artifact.name });
      if (!dest) return;
      await invoke("export_artifact", { id: artifact.id, dest });
      setError(null);
    } catch (e) {
      setError(t("artifacts.export_failed", "导出失败：{{error}}", { error: String(e) }));
    } finally {
      setExporting(false);
    }
  }, [artifact, t]);

  const doDelete = useCallback(async () => {
    if (
      !window.confirm(t("artifacts.delete_confirm", "删除「{{title}}」？", { title }))
    ) {
      return;
    }
    try {
      await invoke("delete_artifact", { id });
      // The card reading a deliverable that no longer exists is the one state
      // this must not leave behind, so dismissing it is part of the delete.
      tail.onClose();
    } catch (e) {
      setError(t("artifacts.delete_failed", "删除失败：{{error}}", { error: String(e) }));
    }
  }, [id, t, tail, title]);

  const build = buildArtifactMenu({
    doc,
    tail,
    t,
    fail: setError,
    title,
    exporting,
    onExport: () => void doExport(),
    onOpenPage: () => requestArtifactNav(id),
    onDelete: () => void doDelete(),
  });

  if (!artifact) {
    return (
      <AuxPane menuItems={build.menu} className={styles.pane}>
        <div className={styles.missing}>
          {/* Before the first fetch settles, "deleted" would be a lie. */}
          {loaded
            ? t("tabs.artifact_missing", "这份产出已被删除")
            : t("artifacts.loading", "Loading…")}
          <code className={styles.missing_key}>{id}</code>
        </div>
      </AuxPane>
    );
  }

  // Directory first, because "which of the eleven posters is this" is the
  // question a rail card most often has to answer; the size and the version
  // follow, and the ingest time is last because it is the least distinguishing.
  const where = [artifact.workspaceName, artifact.path].filter((s) => s.length > 0).join(" / ");
  const versionCount = artifact.versions.length;

  return (
    <AuxPane menuItems={build.menu} className={styles.pane}>
      <AuxDocBar
        kind="artifact"
        icon={<Package size={14} strokeWidth={1.8} />}
        title={title}
        titleHint={artifact.name}
        facts={[
          { text: t(`artifacts.kind.${artifact.kind}`, artifact.kind) },
          { text: formatBytes(artifact.sizeBytes), strong: true },
          {
            text:
              versionCount > 1
                ? t("detail.aux_version_of", "{{version}} / 共 {{count}} 版", {
                    version: artifact.currentVersion,
                    count: versionCount,
                  })
                : artifact.currentVersion,
          },
          { text: timeAgo(artifact.createdMs, t) },
          { text: where },
        ]}
        actions={build.actions}
        menuItems={build.menu}
        onCollapse={tail.onToggle}
        onClose={tail.onClose}
      />
      {error && <p className={styles.error_line}>{error}</p>}
      <div className={styles.body}>
        <ArtifactStage artifact={artifact} />
      </div>
    </AuxPane>
  );
}
