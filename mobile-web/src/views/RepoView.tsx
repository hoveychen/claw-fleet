// 仓库列表：桌面端所有会话 workspace 里的 git 仓库，每个给一个「漏活」健康标
// —— 未推提交(忘记 push)、未合并/有脏改动的 worktree(忘记 merge)。有漏活的
// 仓库排在最前、标黄点。点开进 RepoDetailView 看逐个 worktree 明细并 push/pull。

import { useCallback, useEffect, useState } from "react";
import { ChevronLeft, ChevronRight, FolderGit2 } from "lucide-react";
import { EmptyState } from "./EmptyState";
import { t } from "../i18n";
import type { FleetTransport } from "../transport";
import type { RepoSummary } from "../types";
import { listRepos } from "../repo";
import styles from "./RepoView.module.css";

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
      <div className={styles.header}>
        <button className={styles.backButton} onClick={onBack} aria-label={t("返回")}>
          <ChevronLeft size={22} />
          {t("更多")}
        </button>
        <div className={styles.headerText}>
          <div className={styles.headerTitle}>{t("仓库")}</div>
        </div>
        <button className={styles.refresh} onClick={() => void refresh()} aria-label={t("刷新")}>
          ⟳
        </button>
      </div>

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
 *  single "干净" tag when nothing needs attention. */
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
