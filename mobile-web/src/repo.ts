// 仓库「漏活检查」客户端：列出桌面端所有会话 workspace 中的 git 仓库，揪出
// 未合并回 main 的 worktree（忘记 merge）和领先 origin 的未推提交（忘记 push），
// 并能触发 push / pull。全部经 relay 打到 claw-fleet-core/src/git_ops.rs
// （repo_list / repo_detail / repo_push / repo_pull）。

import type { FleetTransport } from "./transport";
import type { GitOpResult, RepoDetail, RepoSummary } from "./types";

export type { RepoSummary, RepoDetail, WorktreeHealth, CommitInfo, GitOpResult } from "./types";

/** All git repos reachable from a known session workspace, needs-attention first. */
export function listRepos(client: FleetTransport): Promise<RepoSummary[]> {
  return client.request<RepoSummary[]>("repo_list");
}

/** Full detail for one repo: worktrees + recent commits + push state. */
export function fetchRepoDetail(client: FleetTransport, root: string): Promise<RepoDetail> {
  return client.request<RepoDetail>("repo_detail", { root });
}

/** `git push` on the repo's main checkout. Slow (network) — bump the timeout. */
export function pushRepo(client: FleetTransport, root: string): Promise<GitOpResult> {
  return client.request<GitOpResult>("repo_push", { root }, GIT_OP_TIMEOUT_MS);
}

/** `git pull --ff-only` on the repo's main checkout. */
export function pullRepo(client: FleetTransport, root: string): Promise<GitOpResult> {
  return client.request<GitOpResult>("repo_pull", { root }, GIT_OP_TIMEOUT_MS);
}

/** push/pull hit the network on the desktop host; give them room. */
const GIT_OP_TIMEOUT_MS = 60_000;
