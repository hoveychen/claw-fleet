// ── SSH host book — types mirroring Rust structs ─────────────────────────────

/** One SSH host — `claw_fleet_core::remote_host::SshHost`.
 *
 *  A host in the book is an rca executor: the agent stays on this machine and
 *  only the workspace's file I/O is routed to the host (`rcaPath` is the rca
 *  installed there). */
export interface SshHost {
  id: string;
  label: string;
  host: string;
  port: number;
  username: string;
  identityFile: string | null;
  jumpHost: string | null;
  sshProfile: string | null;
  /** rca capability: absolute path of the rca installed on this host.
   *  Absent = not (yet) an rca executor. */
  rcaPath?: string | null;
}

/** The ssh argument fragment for a host — mirrors
 *  `claw_fleet_core::remote_host::ssh_target_for`. Only used for display and
 *  for the health probe's `sshTarget` argument; the launch path resolves this
 *  on the backend, from the same fields. */
export function sshTargetOf(h: SshHost): string {
  const profile = h.sshProfile?.trim();
  if (profile) return profile;
  const host = h.host.trim();
  const user = h.username.trim();
  if (!host || !user) return "";
  const parts: string[] = [];
  if (h.port !== 22) parts.push(`-p ${h.port}`);
  const key = h.identityFile?.trim();
  if (key) parts.push(`-i ${key}`);
  const jump = h.jumpHost?.trim();
  if (jump) parts.push(`-J ${jump}`);
  parts.push(`${user}@${host}`);
  return parts.join(" ");
}

/** What a health probe learned — `claw_fleet_core::remote_host::HostHealth`. */
export interface HostHealth {
  sshOk: boolean;
  home?: string | null;
  rcaPath?: string | null;
  rcaVersion?: string | null;
  stdioOk: boolean;
  error?: string | null;
}
