// Repository list: git repos from all session workspaces on desktop, each gets a
// "loose ends" health marker — unpushed commits (forgot to push), unmerged/dirty
// worktrees (forgot to merge). Repos with loose ends come first, yellow-marked. Click
// to RepoDetailView for per-worktree details and push/pull.

import { useCallback, useEffect, useState } from "react";
import { ChevronRight, FolderGit2, RefreshCw } from "lucide-react";
import { EmptyState } from "./EmptyState";
import { t } from "../i18n";
import type { FleetTransport } from "../transport";
import type { RepoSummary } from "../types";
import { listRepos } from "../repo";
import styles from "./RepoView.module.css";
import { AppHeader } from "./AppHeader";
import { HeaderAction } from "./HeaderAction";

interface Props {
  client: FleetTransport | null;
  onBack: () => void;
  onOpenRepo: (repo: RepoSummary) => void;
}

export function RepoView({ client, onBack, onOpenRepo }: Props) {
  const [repos, setRepos] = useState<RepoSummary[] | null>(null);
  const [error, setError] = useState<string | null>(null);

  const refresh = useCallback(async () => {
    if (!client) return;
    setError(null);
    try {
      setRepos(await listRepos(client));
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    }
  }, [client]);

  useEffect(() => {
    void refresh();
  }, [refresh]);

  const total = repos?.length ?? 0;

  return (
    <div className={styles.page}>
      <AppHeader
        onBack={onBack}
        title={t("仓库")}
        actions={
          <HeaderAction
            icon={<RefreshCw size={17} />}
            label={t("刷新")}
            onClick={() => void refresh()}
          />
        }
      />

      <div className={styles.body}>
        {error && <div className={styles.hint}>{t("仓库加载失败：{0}", error)}</div>}
        {!error && repos === null && <div className={styles.hint}>{t("加载中…")}</div>}
        {!error && repos !== null && total === 0 && (
          <EmptyState
            icon={FolderGit2}
            title={t("没有发现 git 仓库")}
            description={t("仓库来自桌面端各会话的工作目录，有会话在 git 仓库里工作时会出现在这里。")}
          />
        )}

        {!error &&
          repos !== null &&
          total > 0 &&
          repos.map((repo) => (
            <button
              key={repo.root}
              className={styles.repo}
              onClick={() => onOpenRepo(repo)}
            >
              <span className={styles.repoIcon} data-attention={repo.needsAttention}>
                <FolderGit2 size={18} />
              </span>
              <span className={styles.repoBody}>
                <span className={styles.repoTitle}>{repo.label}</span>
                <span className={styles.repoMeta}>
                  {repo.branch ?? t("(游离 HEAD)")}
                  <RepoBadges repo={repo} />
                </span>
              </span>
              <span className={styles.repoChevron}>
                <ChevronRight size={18} />
              </span>
            </button>
          ))}
      </div>
    </div>
  );
}

/** Loose-ends badges: unpushed commits, pending worktrees, dirty files. Shows a
 *  single "clean" tag when nothing needs attention. */
function RepoBadges({ repo }: { repo: RepoSummary }) {
  const badges: Array<{ text: string; tone: "warn" | "info" }> = [];
  if ((repo.unpushed ?? 0) > 0) {
    badges.push({ text: t("未推 {0}", repo.unpushed as number), tone: "warn" });
  }
  if (repo.pendingWorktrees > 0) {
    badges.push({ text: t("待合并 {0}", repo.pendingWorktrees), tone: "warn" });
  }
  if (repo.dirtyCount > 0) {
    badges.push({ text: t("脏 {0}", repo.dirtyCount), tone: "info" });
  }
  if (badges.length === 0) {
    if (repo.worktreeCount > 0) {
      badges.push({ text: t("worktree {0}", repo.worktreeCount), tone: "info" });
    }
    badges.push({ text: t("干净"), tone: "info" });
  }
  return (
    <>
      {badges.map((b, i) => (
        <span key={i} className={styles.badge} data-tone={b.tone}>
          {b.text}
        </span>
      ))}
    </>
  );
}
