/** Collapse an in-repo worktree checkout to its repo root. Fleet develops plans
 *  inside `<repo-root>/.worktrees/<task-id>` (see the worktree workflow), which
 *  are transient — removed once the plan merges. A workspace picker must offer
 *  the durable repo root, never the task-id leaf, and a folder section must fold
 *  a plan's worktree back into the repository it belongs to. Mirrors the
 *  backend's `workspace_name` segment logic (session.rs), but returns the *path*
 *  prefix instead of the name. Paths without a `.worktrees` segment (including
 *  the unrelated `~/.fleet/worktrees/` task-workers, whose segment is
 *  `worktrees`) are returned unchanged.
 *
 *  Shared by the desktop launchpad and the mobile task page so the two group
 *  sessions into the same sections — a divergence here would show the same repo
 *  as one folder on the desktop and several on the phone. */
export function repoRootPath(path: string): string {
  const normalized = path.replace(/\\/g, "/");
  const idx = normalized.split("/").indexOf(".worktrees");
  if (idx <= 0) return path;
  // Rejoin the segments before `.worktrees`, preserving the original separators
  // by slicing the raw string at the segment boundary.
  const before = normalized.split("/").slice(0, idx).join("/");
  return before || path;
}
