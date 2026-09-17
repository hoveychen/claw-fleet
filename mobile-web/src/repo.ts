// Repository "pending-work audit" client: lists all git repos in all session
// workspaces on the desktop, identifies unmerged-back worktrees (forgotten merge)
// and unpushed commits ahead of origin (forgotten push), and can trigger push/pull.
// All go through relay to claw-fleet-core/src/git_ops.rs (repo_list / repo_detail /
// repo_push / repo_pull).

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
