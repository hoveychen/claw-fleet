import { repoRootPath } from "../../shared-ts/repoPath";

/** A registered project, as `list_projects` / `GET /projects` returns it. */
export interface RegisteredProject {
  path: string;
  name: string;
  createdAtMs: number;
  /** Set once the user archived it: still registered, but listed apart. */
  archivedAtMs?: number;
}

/** One row of the project page's list. */
export interface ProjectRow {
  path: string;
  name: string;
  /** Latest activity of any session in it (ms), else its registration time. */
  lastActiveMs: number;
  /** Top-level tasks of it whose process is alive right now. */
  runningTasks: number;
  /** Listed only because the user added (or cloned) it by hand: no session
   *  and no registration. The one kind of row the user can remove. */
  handAdded?: boolean;
}

/** The slice of a scanned session this list needs. */
export interface ProjectSessionView {
  workspacePath: string;
  workspaceName: string;
  isSubagent: boolean;
  procAlive: boolean;
  lastActivityMs: number;
}

const TEMP_ROOTS = ["/tmp", "/private/tmp", "/var/folders", "/private/var/folders"];

/** Paths that are never a project: temp dirs and Fleet's chat workspace.
 *  Mirrors `project_registry::project_path_of`, minus the is-a-directory
 *  check the frontend cannot make. */
function isProjectPath(path: string, chatWorkspace: string | null): boolean {
  if (!path) return false;
  if (chatWorkspace && path === chatWorkspace) return false;
  return !TEMP_ROOTS.some((root) => path === root || path.startsWith(`${root}/`));
}

function nameOf(path: string): string {
  const parts = path.replace(/\\/g, "/").split("/").filter(Boolean);
  return parts[parts.length - 1] ?? path;
}

/**
 * The project list: every registered project plus the repo root of every
 * scanned session (the registry learns a hand-run session's directory only
 * when a client lists projects, so a session newer than that list would
 * otherwise be missing). Archived projects are left out, sessions in them
 * included. Most recently active first.
 */
export function mergeProjects(
  registered: readonly RegisteredProject[],
  sessions: readonly ProjectSessionView[],
  chatWorkspace: string | null,
): ProjectRow[] {
  const rows = new Map<string, ProjectRow>();
  const archived = archivedPaths(registered);
  for (const p of registered) {
    const path = p.path.replace(/\/+$/, "");
    if (archived.has(path) || !isProjectPath(path, chatWorkspace)) continue;
    rows.set(path, { path, name: p.name || nameOf(path), lastActiveMs: p.createdAtMs, runningTasks: 0 });
  }
  for (const s of sessions) {
    const path = repoRootPath(s.workspacePath).replace(/\/+$/, "");
    if (archived.has(path) || !isProjectPath(path, chatWorkspace)) continue;
    let row = rows.get(path);
    if (!row) {
      // A worktree session's workspaceName is the worktree leaf; the root's
      // own directory name is what the project is called.
      const name = path === s.workspacePath ? s.workspaceName || nameOf(path) : nameOf(path);
      row = { path, name, lastActiveMs: 0, runningTasks: 0 };
      rows.set(path, row);
    }
    row.lastActiveMs = Math.max(row.lastActiveMs, s.lastActivityMs);
    if (!s.isSubagent && s.procAlive) row.runningTasks += 1;
  }
  return [...rows.values()].sort((a, b) => b.lastActiveMs - a.lastActiveMs || a.name.localeCompare(b.name));
}

/**
 * `rows` plus the directories the user added or cloned by hand
 * (`list_browse_paths`) that no session or registration already covers. Those
 * go first, newest first, as they come back from the backend: a directory the
 * user just added has no activity yet and would otherwise land at the bottom.
 */
export function withBrowsePaths(
  rows: readonly ProjectRow[],
  browsePaths: readonly string[],
  archived: ReadonlySet<string> = new Set(),
): ProjectRow[] {
  const known = new Set([...rows.map((r) => r.path), ...archived]);
  const extras: ProjectRow[] = [];
  for (const raw of browsePaths) {
    const path = raw.replace(/\/+$/, "");
    if (!path || known.has(path)) continue;
    known.add(path);
    extras.push({ path, name: nameOf(path), lastActiveMs: 0, runningTasks: 0, handAdded: true });
  }
  return [...extras, ...rows];
}

/** The paths of the archived projects. */
export function archivedPaths(registered: readonly RegisteredProject[]): Set<string> {
  return new Set(registered.filter((p) => p.archivedAtMs).map((p) => p.path.replace(/\/+$/, "")));
}

/** The archived projects, most recently archived first. */
export function archivedProjects(registered: readonly RegisteredProject[]): RegisteredProject[] {
  return registered
    .filter((p) => p.archivedAtMs)
    .map((p) => ({ ...p, path: p.path.replace(/\/+$/, ""), name: p.name || nameOf(p.path) }))
    .sort((a, b) => (b.archivedAtMs ?? 0) - (a.archivedAtMs ?? 0));
}

/** The last two segments of a path, for a row's subtitle. */
export function pathTail(path: string): string {
  const parts = path.replace(/\\/g, "/").split("/").filter(Boolean);
  return parts.length <= 2 ? path : `…/${parts.slice(-2).join("/")}`;
}
