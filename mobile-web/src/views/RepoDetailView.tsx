// 单个仓库详情：main 分支的推送状态(未推 / 落后 / 脏文件)、每个 worktree 的
// 「漏活」明细(未合并回 main 的提交数、脏文件、最后提交时间)、最近提交列表，
// 以及 push / pull 按钮(带二次确认，执行后显示 git 输出并刷新)。数据经 relay
// 的 repo_detail / repo_push / repo_pull 打到 git_ops.rs。

import { useCallback, useEffect, useState } from "react";
import { ChevronDown, ChevronLeft, ChevronRight } from "lucide-react";
import { dateLocale, t } from "../i18n";
import type { RelayClient } from "../relay";
import type { DirtyFile, RepoDetail, RepoSummary, WorktreeHealth } from "../types";
import { fetchRepoDetail, pullRepo, pushRepo } from "../repo";
import styles from "./RepoDetailView.module.css";

interface Props {
  repo: RepoSummary;
  client: RelayClient | null;
  onBack: () => void;
}

export function RepoDetailView({ repo, client, onBack }: Props) {
  const [detail, setDetail] = useState<RepoDetail | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState<null | "push" | "pull">(null);
  const [opResult, setOpResult] = useState<{ ok: boolean; output: string } | null>(null);
  // Whether the main checkout's "脏 N" badge is expanded into its file list.
  const [showMainFiles, setShowMainFiles] = useState(false);

  const refresh = useCallback(async () => {
    if (!client) return;
    setError(null);
    try {
      setDetail(await fetchRepoDetail(client, repo.root));
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    }
  }, [client, repo.root]);

  useEffect(() => {
    void refresh();
  }, [refresh]);

  const runOp = useCallback(
    async (op: "push" | "pull") => {
      if (!client || busy) return;
      const prompt = op === "push" ? t("确认 push 到远端？") : t("确认 pull（--ff-only）？");
      if (!window.confirm(prompt)) return;
      setBusy(op);
      setOpResult(null);
      try {
        const res = op === "push" ? await pushRepo(client, repo.root) : await pullRepo(client, repo.root);
        setOpResult(res);
        await refresh();
      } catch (e) {
        setOpResult({ ok: false, output: e instanceof Error ? e.message : String(e) });
      } finally {
        setBusy(null);
      }
    },
    [client, busy, repo.root, refresh],
  );

  return (
    <div className={styles.page}>
      <div className={styles.header}>
        <button className={styles.backButton} onClick={onBack} aria-label={t("返回")}>
          <ChevronLeft size={22} />
          {t("仓库")}
        </button>
        <div className={styles.headerText}>
          <div className={styles.headerTitle}>{repo.label}</div>
          <div className={styles.headerSub}>{repo.root}</div>
        </div>
      </div>

      <div className={styles.body}>
        {error && <div className={styles.hint}>{t("仓库加载失败：{0}", error)}</div>}
        {!error && detail === null && <div className={styles.hint}>{t("加载中…")}</div>}

        {!error && detail && (
          <>
            {/* ── 分支 / 推送状态 ── */}
            <div className={styles.section}>
              <div className={styles.sectionLabel}>{t("当前分支")}</div>
              <div className={styles.card}>
                <div className={styles.row}>
                  <span className={styles.rowLabel}>{detail.branch ?? t("(游离 HEAD)")}</span>
                  <span className={styles.rowValue}>
                    {detail.upstream ?? t("无上游")}
                  </span>
                </div>
                <div className={styles.divider} />
                {/* 远端地址：只读。URL 比这一行宽得多，所以让它自己换行而不是
                    把右侧挤没（rowValue 是 flex-shrink: 0 的）。 */}
                <div className={styles.row}>
                  <span className={styles.rowLabel}>{t("远端地址")}</span>
                  <span className={styles.remoteValue}>
                    {detail.remoteUrl ?? t("未配置")}
                  </span>
                </div>
                <div className={styles.divider} />
                <div className={styles.row}>
                  <span className={styles.rowLabel}>{t("未推 / 落后")}</span>
                  <span className={styles.statChips}>
                    <StatChip
                      value={detail.unpushed}
                      label={t("未推 {0}", detail.unpushed ?? 0)}
                      tone="warn"
                    />
                    <StatChip
                      value={detail.behind}
                      label={t("落后 {0}", detail.behind ?? 0)}
                      tone="info"
                    />
                    {detail.dirtyCount > 0 && (
                      <button
                        type="button"
                        className={`${styles.chip} ${styles.chipToggle}`}
                        data-tone="info"
                        aria-expanded={showMainFiles}
                        onClick={() => setShowMainFiles((v) => !v)}
                      >
                        {t("脏 {0}", detail.dirtyCount)}
                        {showMainFiles ? <ChevronDown size={11} /> : <ChevronRight size={11} />}
                      </button>
                    )}
                    {(detail.unpushed ?? 0) === 0 &&
                      (detail.behind ?? 0) === 0 &&
                      detail.dirtyCount === 0 && (
                        <span className={styles.chip} data-tone="ok">
                          {t("干净")}
                        </span>
                      )}
                  </span>
                </div>
              </div>
              {showMainFiles && detail.dirtyCount > 0 && (
                <DirtyFileList files={detail.dirtyFiles} />
              )}
              <div className={styles.opRow}>
                <button
                  className={styles.opButton}
                  disabled={busy !== null || !client}
                  onClick={() => void runOp("pull")}
                >
                  {busy === "pull" ? t("拉取中…") : t("Pull")}
                </button>
                <button
                  className={styles.opButton}
                  data-primary="true"
                  disabled={busy !== null || !client}
                  onClick={() => void runOp("push")}
                >
                  {busy === "push" ? t("推送中…") : t("Push")}
                </button>
              </div>
              {opResult && (
                <pre className={styles.opOutput} data-ok={opResult.ok}>
                  {opResult.output || (opResult.ok ? t("完成") : t("失败"))}
                </pre>
              )}
            </div>

            {/* ── worktrees ── */}
            <div className={styles.section}>
              <div className={styles.sectionLabel}>
                {t("worktree（{0}）", detail.worktrees.length)}
              </div>
              {detail.worktrees.length === 0 ? (
                <div className={styles.emptyNote}>{t("没有 worktree。")}</div>
              ) : (
                <div className={styles.card}>
                  {detail.worktrees.map((w, i) => (
                    <div key={w.path}>
                      {i > 0 && <div className={styles.divider} />}
                      <WorktreeRow wt={w} />
                    </div>
                  ))}
                </div>
              )}
            </div>

            {/* ── 最近提交 ── */}
            <div className={styles.section}>
              <div className={styles.sectionLabel}>{t("最近提交")}</div>
              {detail.commits.length === 0 ? (
                <div className={styles.emptyNote}>{t("没有提交。")}</div>
              ) : (
                <div className={styles.card}>
                  {detail.commits.map((c, i) => (
                    <div key={c.hash + i}>
                      {i > 0 && <div className={styles.divider} />}
                      <div className={styles.commit}>
                        <code className={styles.commitHash}>{c.hash}</code>
                        <span className={styles.commitBody}>
                          <span className={styles.commitSummary}>{c.summary}</span>
                          <span className={styles.commitMeta}>
                            {c.author} · {fmtTime(c.time)}
                          </span>
                        </span>
                      </div>
                    </div>
                  ))}
                </div>
              )}
            </div>
          </>
        )}
      </div>
    </div>
  );
}

function StatChip({
  value,
  label,
  tone,
}: {
  value: number | null;
  label: string;
  tone: "warn" | "info";
}) {
  // A null count (no upstream) or a zero count is muted; a real count lights up.
  const active = (value ?? 0) > 0;
  return (
    <span className={styles.chip} data-tone={active ? tone : "muted"}>
      {label}
    </span>
  );
}

function WorktreeRow({ wt }: { wt: WorktreeHealth }) {
  const pending = wt.unmerged > 0 || wt.dirtyCount > 0;
  const [showFiles, setShowFiles] = useState(false);
  return (
    <div className={styles.worktree}>
      <span className={styles.wtDot} data-pending={pending} />
      <span className={styles.wtBody}>
        <span className={styles.wtBranch}>{wt.branch ?? t("(游离 HEAD)")}</span>
        <span className={styles.wtMeta}>
          {wt.unmerged > 0 && (
            <span className={styles.chip} data-tone="warn">
              {t("未合并 {0}", wt.unmerged)}
            </span>
          )}
          {wt.dirtyCount > 0 && (
            <button
              type="button"
              className={`${styles.chip} ${styles.chipToggle}`}
              data-tone="info"
              aria-expanded={showFiles}
              onClick={() => setShowFiles((v) => !v)}
            >
              {t("脏 {0}", wt.dirtyCount)}
              {showFiles ? <ChevronDown size={11} /> : <ChevronRight size={11} />}
            </button>
          )}
          {!pending && (
            <span className={styles.chip} data-tone="ok">
              {t("已合并")}
            </span>
          )}
          {wt.lastCommitTime != null && (
            <span className={styles.wtTime}>{fmtTime(wt.lastCommitTime)}</span>
          )}
        </span>
        {showFiles && wt.dirtyCount > 0 && <DirtyFileList files={wt.dirtyFiles} />}
      </span>
    </div>
  );
}

/** Read-only list of uncommitted files (path + one-char status code). Mobile
 *  has no file tree, so — unlike the desktop 文件 tab — entries are display-only. */
function DirtyFileList({ files }: { files: DirtyFile[] }) {
  return (
    <ul className={styles.dirtyFiles}>
      {files.map((f) => (
        <li key={f.path} className={styles.dirtyFile}>
          <span className={styles.dirtyStatus} data-status={f.status}>
            {f.status}
          </span>
          <span className={styles.dirtyPath}>{f.path}</span>
        </li>
      ))}
    </ul>
  );
}

/** Unix seconds → localized short date-time. */
function fmtTime(sec: number): string {
  if (!sec) return "";
  return new Date(sec * 1000).toLocaleDateString(dateLocale(), {
    year: "2-digit",
    month: "2-digit",
    day: "2-digit",
    hour: "2-digit",
    minute: "2-digit",
  });
}
