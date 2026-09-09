import { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { ExternalLink } from "lucide-react";

import { formatBytes } from "../formatBytes";
import { useUIStore } from "../store";
import { loadArtifact } from "./blocks/ingestLookup";
import { ArtifactStage, type Artifact } from "./ArtifactsView";
import styles from "./TabPanes.module.css";

/**
 * One deliverable as a detail-column tab.
 *
 * Same contract as `WikiTabPane`, and for the same reason: the point of the
 * auxiliary column is to have the thing *beside* the transcript that produced
 * it rather than instead of it. So this is reader-only — the body is the very
 * `ArtifactStage` the 产出 page renders, and the header carries only what
 * reading needs plus a way over to that page, which owns the actions (rename,
 * move, version rollback, delete, export) and their dialogs.
 */
export function ArtifactTabPane({ id }: { id: string }) {
  const { t } = useTranslation();
  const requestArtifactNav = useUIStore((s) => s.requestArtifactNav);
  const [artifact, setArtifact] = useState<Artifact | null>(null);
  const [loaded, setLoaded] = useState(false);

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

  if (!artifact) {
    return (
      <div className={styles.pane}>
        <div className={styles.missing}>
          {/* Before the first fetch settles, "deleted" would be a lie. */}
          {loaded
            ? t("tabs.artifact_missing", "这份产出已被删除")
            : t("artifacts.loading", "Loading…")}
          <code className={styles.missing_key}>{id}</code>
        </div>
      </div>
    );
  }

  return (
    <div className={styles.pane}>
      <div className={styles.bar}>
        <div className={styles.bar_text}>
          <span className={styles.bar_label}>
            {t(`artifacts.kind.${artifact.kind}`, artifact.kind)} · {formatBytes(artifact.sizeBytes)}
          </span>
          <span className={styles.bar_main}>{artifact.title}</span>
        </div>
        <div className={styles.bar_actions}>
          <button
            type="button"
            className={styles.bar_btn}
            onClick={() => requestArtifactNav(artifact.id)}
            title={t("detail.ingest.open_artifact", "在产出页打开")}
          >
            <ExternalLink size={12} strokeWidth={1.7} />
            {t("detail.ingest.open_artifact_short", "产出")}
          </button>
        </div>
      </div>
      <div className={styles.body}>
        <ArtifactStage artifact={artifact} />
      </div>
    </div>
  );
}
