use super::*;

// ── Helpers ──────────────────────────────────────────────────────────────────

/// Returns the real user home directory, robust to a redirected `$HOME`.
///
/// Origin: the desktop app used to ship with the macOS App Sandbox enabled,
/// where `dirs::home_dir()` returns the container path
/// (`~/Library/Containers/<id>/Data/`). The sandbox was dropped from
/// entitlements.plist in 2026-07, but this stays the canonical home lookup:
/// `getpwuid` reads the passwd database, so `~/.claude/`, `~/.fleet/`,
/// `~/.ssh/` etc. resolve correctly even under a polluted/overridden `$HOME`.
///
/// For integration tests, setting `FLEET_HOME` overrides the detected home
/// so the test can operate on a temp dir without touching the real user's
/// `~/.fleet/`. Intended for tests only — production code never sets it.
pub fn real_home_dir() -> Option<PathBuf> {
    if let Some(dir) = std::env::var_os("FLEET_HOME") {
        let path = PathBuf::from(dir);
        if !path.as_os_str().is_empty() {
            return Some(path);
        }
    }
    #[cfg(target_os = "macos")]
    {
        use std::ffi::CStr;
        let pw = unsafe { libc::getpwuid(libc::getuid()) };
        if !pw.is_null() {
            let home = unsafe { CStr::from_ptr((*pw).pw_dir) };
            return Some(PathBuf::from(home.to_string_lossy().into_owned()));
        }
    }
    dirs::home_dir()
}

pub fn get_claude_dir() -> Option<PathBuf> {
    real_home_dir().map(|h| h.join(".claude"))
}

/// Fleet's own data directory: `~/.fleet/`.
pub fn get_fleet_dir() -> Option<PathBuf> {
    real_home_dir().map(|h| h.join(".fleet"))
}

/// Codex's config home: `$CODEX_HOME` if set (Codex honours it), else `~/.codex`.
///
/// This is where Codex keeps its data *and* discovers skills — Codex's own
/// skill-installer installs into `$CODEX_HOME/skills` (default `~/.codex/skills`).
/// Canonical lookup so projection (`skill_sync`) and discovery (`skills`,
/// `memory`) agree; `FLEET_HOME` is honoured transitively through
/// [`real_home_dir`] for test isolation.
pub fn get_codex_dir() -> Option<PathBuf> {
    if let Some(dir) = std::env::var_os("CODEX_HOME") {
        let path = PathBuf::from(dir);
        if !path.as_os_str().is_empty() {
            return Some(path);
        }
    }
    real_home_dir().map(|h| h.join(".codex"))
}

#[cfg(unix)]
pub fn is_process_alive(pid: u32) -> bool {
    let ret = unsafe { libc::kill(pid as libc::pid_t, 0) };
    if ret == 0 {
        return true;
    }
    // EPERM = process exists but we lack permission to signal it → alive
    // ESRCH = no such process → dead
    std::io::Error::last_os_error().raw_os_error() != Some(libc::ESRCH)
}

/// Read the process start time (seconds since the Unix epoch) for `pid`.
///
/// Returns `None` if no live process owns this pid right now. Used together
/// with [`is_process_alive`] to defend against PID reuse: an entry in a
/// lock file becomes "stale" if either the pid is dead OR the live process
/// at that pid was spawned at a different start time than the one
/// snapshotted on `acquire`. See
/// `~/.claude/projects/.../memory/project_mcp_injector_pid_reuse.md` for
/// the diagnosis that motivated this helper.
///
/// **Why not sysinfo?** A previous version used
/// `sysinfo::refresh_processes_specifics(Some(&[pid]), …)`, but on macOS
/// that path first calls `libc::proc_listallpids` to enumerate every pid
/// on the host *before* filtering — and the macOS App Sandbox returns 0
/// from `proc_listallpids`, so the refresh becomes a no-op, the process
/// isn't recorded, and `sysinfo.process(pid)` returns `None`. Combined
/// with `unwrap_or(0)` upstream, every holder in a sandboxed Fleet build
/// silently captured `start_time_secs = 0`, neutralising the PID-reuse
/// defence. The macOS branch below therefore calls `libc::proc_pidinfo`
/// directly for the target pid, which was permitted even inside the sandbox.
/// The desktop app has since dropped the App Sandbox (2026-07), but the
/// direct call stays — it skips the full-host pid enumeration sysinfo does.
#[cfg(target_os = "macos")]
pub fn process_start_time(pid: u32) -> Option<u64> {
    use std::mem;
    if pid == 0 {
        return None;
    }
    let mut info: libc::proc_bsdinfo = unsafe { mem::zeroed() };
    let size = mem::size_of::<libc::proc_bsdinfo>() as i32;
    let result = unsafe {
        libc::proc_pidinfo(
            pid as i32,
            libc::PROC_PIDTBSDINFO,
            0,
            &mut info as *mut libc::proc_bsdinfo as *mut std::ffi::c_void,
            size,
        )
    };
    if result != size {
        return None;
    }
    Some(info.pbi_start_tvsec as u64)
}

/// Linux equivalent: read `/proc/<pid>/stat` field 22 (`starttime` in
/// clock ticks since boot) and convert to Unix epoch via `/proc/stat`'s
/// `btime` line. We do this directly rather than via sysinfo so the
/// macOS and Linux paths share the same "no global pid enumeration"
/// shape — handy if Linux ever gains an equivalent sandbox restriction.
#[cfg(target_os = "linux")]
pub fn process_start_time(pid: u32) -> Option<u64> {
    if pid == 0 {
        return None;
    }
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    // /proc/<pid>/stat format:
    //   pid (comm) state ppid pgrp ... starttime ...
    // The comm field can contain spaces and parens, so we slice off
    // everything up to and including the LAST ')' before parsing the
    // remaining whitespace-separated fields. After that slice, field
    // indexes are 1-based starting at `state`; starttime is field 22
    // overall, i.e. (22 - 2) = 20 fields after the ')'. Zero-based
    // index 19.
    let after_comm = stat.rfind(')')?;
    let rest = &stat[after_comm + 1..];
    let fields: Vec<&str> = rest.split_whitespace().collect();
    let ticks: u64 = fields.get(19)?.parse().ok()?;
    let btime = read_linux_btime()?;
    let clk_tck = unsafe { libc::sysconf(libc::_SC_CLK_TCK) };
    if clk_tck <= 0 {
        return None;
    }
    Some(btime + ticks / clk_tck as u64)
}

#[cfg(target_os = "linux")]
fn read_linux_btime() -> Option<u64> {
    let stat = std::fs::read_to_string("/proc/stat").ok()?;
    for line in stat.lines() {
        if let Some(rest) = line.strip_prefix("btime ") {
            return rest.trim().parse().ok();
        }
    }
    None
}

/// Windows fallback: there's no app-sandbox blocking pid enumeration in
/// our Windows distribution, so sysinfo's path remains the simplest
/// option. If Windows ever gains a similar concern, swap this to
/// `NtQueryInformationProcess(ProcessTimes)` directly.
#[cfg(windows)]
pub fn process_start_time(pid: u32) -> Option<u64> {
    use sysinfo::{Pid, ProcessRefreshKind, ProcessesToUpdate, System};
    if pid == 0 {
        return None;
    }
    let mut sys = System::new();
    let target = Pid::from_u32(pid);
    sys.refresh_processes_specifics(
        ProcessesToUpdate::Some(&[target]),
        true,
        ProcessRefreshKind::nothing(),
    );
    sys.process(target).map(|p| p.start_time())
}

/// A live-process holder of a Fleet lock file (mcp-lock / permissions-lock /
/// any future sibling). Pairs the holder's pid with its `start_time_secs`
/// so [`prune_dead_holders`] can defeat PID reuse: when the OS later
/// recycles `pid` to an unrelated process, the recycled process's
/// start_time differs from the snapshot, so the entry is correctly pruned.
///
/// `start_time_secs = 0` is reserved as the "legacy / unknown" marker — it
/// never matches a real live process (start_time is seconds-since-epoch),
/// so legacy lock files (written before this field existed) prune to
/// empty on first read. See [`deserialize_holders`] for the on-disk
/// migration path and `project_mcp_injector_pid_reuse` memory for the
/// original diagnosis.
#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Eq)]
pub struct HolderEntry {
    pub pid: u32,
    #[serde(default)]
    pub start_time_secs: u64,
}

impl HolderEntry {
    /// Build a fresh entry for `pid`, snapshotting its current start_time.
    /// If the pid no longer resolves (process already gone), the
    /// snapshot falls back to 0 — and the next `prune_dead_holders` will
    /// drop the entry on the spot.
    pub fn capture(pid: u32) -> Self {
        Self {
            pid,
            start_time_secs: process_start_time(pid).unwrap_or(0),
        }
    }
}

/// Drop holder entries whose process is no longer alive **or** whose
/// recorded `start_time_secs` no longer matches the live process at
/// that pid. The start_time match is the PID-reuse defence.
pub fn prune_dead_holders(holders: &mut Vec<HolderEntry>) {
    holders.retain(|h| {
        is_process_alive(h.pid)
            && process_start_time(h.pid)
                .map(|t| t == h.start_time_secs)
                .unwrap_or(false)
    });
}

/// Custom serde deserializer that accepts both the legacy `[u32, ...]`
/// shape (written by older Fleet builds) and the new
/// `[{pid, start_time_secs}, ...]` shape. Legacy entries are mapped to
/// `HolderEntry { pid, start_time_secs: 0 }` so `prune_dead_holders`
/// drops them on first read after upgrade.
pub fn deserialize_holders<'de, D>(d: D) -> Result<Vec<HolderEntry>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Item {
        Legacy(u32),
        New(HolderEntry),
    }
    let items: Vec<Item> = Vec::deserialize(d)?;
    Ok(items
        .into_iter()
        .map(|i| match i {
            Item::Legacy(pid) => HolderEntry { pid, start_time_secs: 0 },
            Item::New(e) => e,
        })
        .collect())
}

#[cfg(windows)]
pub fn is_process_alive(pid: u32) -> bool {
    if pid == 0 {
        return false;
    }
    use std::ffi::c_void;
    type Handle = *mut c_void;
    const PROCESS_QUERY_LIMITED_INFORMATION: u32 = 0x1000;
    const STILL_ACTIVE: u32 = 259;
    const ERROR_INVALID_PARAMETER: u32 = 87;
    extern "system" {
        fn OpenProcess(
            dw_desired_access: u32,
            b_inherit_handle: i32,
            dw_process_id: u32,
        ) -> Handle;
        fn CloseHandle(h_object: Handle) -> i32;
        fn GetExitCodeProcess(h_process: Handle, lp_exit_code: *mut u32) -> i32;
        fn GetLastError() -> u32;
    }
    // SAFETY: standard Win32 calls. Handle is closed in every success path.
    unsafe {
        let h = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if h.is_null() {
            // Mirror Unix EPERM-as-alive: ERROR_INVALID_PARAMETER means
            // the pid doesn't refer to any process; any other error
            // (e.g. ERROR_ACCESS_DENIED) means the process exists but
            // we don't have rights to query it.
            return GetLastError() != ERROR_INVALID_PARAMETER;
        }
        let mut exit_code: u32 = 0;
        let got = GetExitCodeProcess(h, &mut exit_code) != 0;
        CloseHandle(h);
        got && exit_code == STILL_ACTIVE
    }
}

pub(crate) fn decode_workspace_path(encoded: &str) -> String {
    // "-Users-hoveychen-workspace-netferry" → "/Users/hoveychen/workspace/netferry"
    //
    // Dashes are ambiguous (path separator vs literal dash in a directory name).
    // We resolve this by greedily checking the filesystem: at each level, try the
    // longest remaining dash-joined segment first, and shorten until we find a real
    // directory.  Fall back to the naive one-dash-per-slash decode if nothing matches.
    let stripped = encoded.trim_start_matches('-');
    let parts: Vec<&str> = stripped.split('-').collect();
    if parts.is_empty() {
        return "/".to_string();
    }
    decode_workspace_path_with_parts(&parts)
}

/// Detect a Windows drive-letter prefix among dash-split path parts. Claude's
/// `sanitizePath` maps both `:` and `\` to `-`, so `C:\Users\foo` encodes as
/// `C--Users-foo`, which `split('-')` yields as `["C", "", "Users", "foo"]` —
/// a single ASCII-letter head followed by an empty part (the `--`). Returns the
/// drive letter and the remaining parts (empties inside the path are kept; the
/// fs walk disambiguates them exactly as it does on unix).
///
/// Call site is unix-gated: on unix the same `["x", "", …]` shape is a real
/// directory whose two chars collapsed (e.g. `/foo/_bar` → `foo--bar`), so this
/// must NOT fire there. `test` cfg lets the pure logic be unit-tested on macOS.
#[cfg(any(not(unix), test))]
pub(crate) fn windows_drive_split<'a>(parts: &'a [&'a str]) -> Option<(&'a str, &'a [&'a str])> {
    let first = parts.first()?;
    if first.len() == 1
        && first.as_bytes()[0].is_ascii_alphabetic()
        && parts.get(1) == Some(&"")
    {
        Some((first, &parts[2..]))
    } else {
        None
    }
}

pub fn decode_workspace_path_with_parts(parts: &[&str]) -> String {
    // Windows: a drive path "C:\Users\foo" encodes as "C--Users-foo" (both `:`
    // and `\` collapse to `-`) → ["C","","Users","foo"]. Start the walk at the
    // drive root ("C:") instead of the unix filesystem root. Unix-gated: the
    // same shape is a legitimate collapsed name there.
    #[cfg(not(unix))]
    if let Some((drive, rest)) = windows_drive_split(parts) {
        return decode_walk(format!("{drive}:"), rest);
    }
    decode_walk(String::new(), parts)
}

/// Greedy filesystem-guided decode: at each level try the longest remaining
/// dash-joined segment that names a real directory, shortening until one
/// matches; fall back to a single part when nothing exists on disk. `start` is
/// the already-decoded prefix — `""` for the unix root `/`, or `"C:"` for a
/// Windows drive root.
fn decode_walk(start: String, parts: &[&str]) -> String {
    let mut current = start; // built path so far (e.g. "/Users/hoveychen")
    let mut i = 0;
    while i < parts.len() {
        // Build a map of (encoded dir name) → (real dir name) for the current
        // level. Claude collapses `/`, `.` AND `_` all into `-`, so the real
        // on-disk directory name may differ from the dash-joined candidate by a
        // `.` or `_` — matching real entries by their re-encoded form recovers
        // names like `kol_dash` that a `-`→`/`-only decode cannot.
        let level_dirs = read_level_dirs(&current);

        // Try longest remaining segment first: join parts[i..] with '-', then parts[i..len-1], etc.
        let mut matched = false;
        // Try from longest (all remaining parts) down to single part
        for end in (i + 1..=parts.len()).rev() {
            let candidate_segment = parts[i..end].join("-");
            if let Some(real) = level_dirs.get(&candidate_segment) {
                current = format!("{}/{}", current, real);
                i = end;
                matched = true;
                break;
            }
        }
        if !matched {
            // Nothing exists on disk — use single part (original naive behavior)
            current = format!("{}/{}", current, parts[i]);
            i += 1;
        }
    }
    current
}

/// Re-encode a single directory entry name the way Claude Code encodes paths.
/// Claude's `sanitizePath` (claude-code-fork `src/utils/sessionStoragePortable.ts`)
/// maps EVERY non-alphanumeric character to `-` — not just `/`, `.`, `_`. An
/// entry name never contains `/`, but `.`, `_`, spaces, `+`, etc. are all
/// collapsed, so matching only `.`/`_` missed real names like `a b` or `a+b`.
pub(crate) fn encode_path_segment(name: &str) -> String {
    name.chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect()
}

/// List the immediate sub-directories of `parent` (empty string = filesystem
/// root) keyed by their Claude-encoded name. A directory literally named with
/// `-` wins over a `.`/`_` collision so an exact path still round-trips.
///
/// TCC-safe: never reads a protected directory, and resolves an entry's type
/// from the readdir `d_type` (no per-entry `stat`) so listing `~` doesn't fire a
/// macOS permission dialog for `~/Documents`, `~/Downloads`, etc. Symlinks are
/// only followed when their target is not TCC-protected.
pub(crate) fn read_level_dirs(parent: &str) -> std::collections::HashMap<String, String> {
    let mut map: std::collections::HashMap<String, String> = std::collections::HashMap::new();
    // A bare "C:" is drive-relative on Windows (it means "cwd on C"), so the
    // drive ROOT needs the separator appended. Unix never builds a "C:"-shaped
    // prefix (its walk starts at "" → "/…"), so this branch is inert there.
    let drive_root;
    let dir = if parent.is_empty() {
        "/"
    } else if parent.len() == 2
        && parent.as_bytes()[1] == b':'
        && parent.as_bytes()[0].is_ascii_alphabetic()
    {
        drive_root = format!("{parent}\\");
        drive_root.as_str()
    } else {
        parent
    };
    let dir_path = std::path::Path::new(dir);
    if crate::tcc::is_tcc_protected(dir_path) {
        return map;
    }
    let Ok(entries) = std::fs::read_dir(dir_path) else {
        return map;
    };
    for entry in entries.flatten() {
        let Ok(name) = entry.file_name().into_string() else {
            continue;
        };
        let is_dir = match entry.file_type() {
            Ok(ft) if ft.is_dir() => true,
            Ok(ft) if ft.is_symlink() => {
                let target = entry.path();
                !crate::tcc::is_tcc_protected(&target) && target.is_dir()
            }
            _ => false,
        };
        if !is_dir {
            continue;
        }
        let key = encode_path_segment(&name);
        // A literal name (no `.`/`_` rewritten) is the unambiguous decoding, so
        // let it overwrite any earlier `.`/`_` collision; otherwise keep the
        // first seen.
        if name == key || !map.contains_key(&key) {
            map.insert(key, name);
        }
    }
    map
}

pub(crate) fn encode_workspace_path(path: &str) -> String {
    // Mirror Claude Code's `sanitizePath`: EVERY non-alphanumeric char → `-`
    // (claude-code-fork `src/utils/sessionStoragePortable.ts`). This is the key
    // Fleet matches against both the `~/.claude/projects/<dir>` name and CC's
    // scratchpad slug (`getProjectTempDir = join(tmp, sanitizePath(cwd))`).
    // On Windows a drive path `C:\Users\foo` → `C--Users-foo` (`:` and `\` both
    // collapse). Replacing only `/` broke every path containing `.`/`_`/space
    // (all platforms) and every Windows path (no `/` at all).
    //
    // Not reproduced here: CC truncates + hashes names over 200 chars
    // (`MAX_SANITIZED_LENGTH`), and the hash differs between its Bun and Node
    // runtimes — irreproducible without knowing which ran, so deep (>200-char)
    // paths fall through to the lossy fs-walk decode instead of a fast match.
    path.chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect()
}

/// Human-facing name for a workspace path. Shared with the memory module so
/// both derive worktree names identically.
pub(crate) fn workspace_name(path: &str) -> String {
    // The pure-chat workspace is a Fleet-owned directory, not a project — its
    // `chat` basename would read as a random folder in the session list.
    if crate::chat_workspace::is_chat_workspace(path) {
        return crate::chat_workspace::CHAT_WORKSPACE_NAME.to_string();
    }
    let segments: Vec<&str> = path.split(['/', '\\']).filter(|s| !s.is_empty()).collect();
    // Fleet develops plans inside `<repo-root>/.worktrees/<task-id>` (see the
    // worktree workflow). Such a checkout belongs to the repo, so name it after
    // the segment before `.worktrees` rather than the task-id leaf.
    if let Some(idx) = segments.iter().position(|s| *s == ".worktrees") {
        if idx > 0 {
            return segments[idx - 1].to_string();
        }
    }
    segments
        .last()
        .copied()
        .unwrap_or(path)
        .to_string()
}

#[cfg(test)]
mod workspace_name_win_tests {
    use super::workspace_name;

    #[test]
    fn windows_backslash_path_yields_basename() {
        // A real Windows path reaches here from lock files / browsed dirs.
        // Splitting on '/' only leaves the whole `C:\...\proj` as the name.
        assert_eq!(workspace_name("C:\\Users\\foo\\proj"), "proj");
    }

    #[test]
    fn windows_worktree_uses_repo_name() {
        assert_eq!(workspace_name("C:\\code\\repo\\.worktrees\\task-1"), "repo");
    }
}

