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
    user_home_dir()
}

/// Where the *agent's* own files live — its credentials (`~/.claude.json`),
/// its config dir, and the transcripts it writes.
///
/// Defaults to [`real_home_dir`], so nothing changes unless you ask it to.
/// `FLEET_AGENT_HOME` overrides it, and is the answer to a specific bind:
/// `FLEET_HOME` means "put *Fleet's* state elsewhere", but it also relocates
/// the agent's home, and `claude` then looks for `$FLEET_HOME/.claude.json`,
/// finds nothing, and replies "Not logged in · Please run /login".
///
/// That is not hypothetical — it blocked the ACP mobile acceptance run, where
/// isolating Fleet's state was mandatory (an un-isolated `fleet serve`
/// competes for the relay-agent role) and cost the agent its login.
///
/// The one rule: **spawning and scanning must agree**. Pointing only the
/// child's `HOME` at the real home would let it log in and then write its
/// transcript somewhere the scanner never looks, so every agent-file lookup
/// goes through here.
pub fn agent_home_dir() -> Option<PathBuf> {
    if let Some(dir) = std::env::var_os("FLEET_AGENT_HOME") {
        let path = PathBuf::from(dir);
        if !path.as_os_str().is_empty() {
            return Some(path);
        }
    }
    real_home_dir()
}

/// The user's home, ignoring `FLEET_HOME`.
///
/// Same detection as [`real_home_dir`] minus the test override: `getpwuid`
/// first, so it survives a polluted or overridden `$HOME`.
///
/// This exists because `FLEET_HOME` means "put *Fleet's own state* somewhere
/// else" — the fifty-odd `~/.fleet/…` lookups — and an agent's credentials are
/// not Fleet's state. Pinning a spawned agent's `HOME` to `FLEET_HOME` sends
/// `claude` looking for `$FLEET_HOME/.claude.json`, finds nothing, and the
/// agent answers "Not logged in · Please run /login". That surfaced during the
/// ACP acceptance run, where isolating Fleet's state was mandatory (an
/// un-isolated `fleet serve` competes for the relay-agent role) and cost the
/// agent its login.
pub fn user_home_dir() -> Option<PathBuf> {
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

/// Claude Code's config directory: `$CLAUDE_CONFIG_DIR` if set (Claude Code
/// honours it — every `~/.claude` path relocates under it), else `~/.claude`.
///
/// Mirrors [`get_codex_dir`]'s `$CODEX_HOME` handling. `FLEET_HOME` is honoured
/// transitively through [`real_home_dir`] for test isolation. The env value is
/// used verbatim (`PathBuf::from`) — Claude Code does not expand a leading `~`
/// (verified: a literal `~/x` value is treated as a relative path), so neither
/// do we; real users set an absolute path.
pub fn get_claude_dir() -> Option<PathBuf> {
    if let Some(dir) = std::env::var_os("CLAUDE_CONFIG_DIR") {
        let path = PathBuf::from(dir);
        if !path.as_os_str().is_empty() {
            return Some(path);
        }
    }
    agent_home_dir().map(|h| h.join(".claude"))
}

/// Path to Claude Code's `.claude.json` (MCP server config + project history).
///
/// **Asymmetric** with [`get_claude_dir`]: `.claude.json` sits at the *root* of
/// the config location, not inside a `.claude/` subdir.
/// - `$CLAUDE_CONFIG_DIR` set → `$CLAUDE_CONFIG_DIR/.claude.json`
/// - unset → `$HOME/.claude.json` (home root, *outside* `~/.claude/`)
///
/// Verified by isolating `HOME` + `CLAUDE_CONFIG_DIR`: with the var set, Claude
/// Code writes `<var>/.claude.json`; unset, it writes `~/.claude.json`. Cross-
/// checked against Claude Code docs ("every ~/.claude path lives under that
/// directory instead") and Fleet's pre-existing `~/.claude.json` usage.
pub fn get_claude_config_json() -> Option<PathBuf> {
    if let Some(dir) = std::env::var_os("CLAUDE_CONFIG_DIR") {
        let path = PathBuf::from(dir);
        if !path.as_os_str().is_empty() {
            return Some(path.join(".claude.json"));
        }
    }
    agent_home_dir().map(|h| h.join(".claude.json"))
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
    agent_home_dir().map(|h| h.join(".codex"))
}

/// dsh's config home: `$DSH_HOME` if set (dsh honours it), else `~/.dsh`.
///
/// The third agent's analogue of [`get_codex_dir`], and canonical for the same
/// reason: `@deepseek-ai/dsh-home-paths` resolves this exact pair, so every
/// Fleet surface that writes into or reads out of dsh's home — the guidance
/// AGENTS.md ([`crate::dsh_guidance`]) and the skill roots ([`crate::skills`]) —
/// has to agree with it, or Fleet writes somewhere dsh never looks.
pub fn get_dsh_dir() -> Option<PathBuf> {
    if let Some(dir) = std::env::var_os("DSH_HOME") {
        let path = PathBuf::from(dir);
        if !path.as_os_str().is_empty() {
            return Some(path);
        }
    }
    real_home_dir().map(|h| h.join(".dsh"))
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

/// The workspace a `~/.claude/projects/<slug>/` directory really belongs to.
///
/// `decoded` is [`decode_workspace_path`]'s best effort. That decode is
/// filesystem-guided, so it lands on the real path whenever every level of the
/// path can be listed — but the slug encoding is lossy (`-` is both a path
/// separator and a literal character in a directory name), so the moment a
/// level *cannot* be listed the walk falls back to one-dash-per-slash and
/// shreds every hyphenated directory name in the path. That happens for a
/// workspace under a TCC-protected folder (`~/Documents`, `~/Desktop`, …, where
/// [`read_level_dirs`] deliberately returns nothing rather than fire a
/// permission dialog) and for any `read_dir` denial — an unmounted volume, a
/// sandbox without folder access.
///
/// The transcripts in the directory carry the authoritative answer: Claude Code
/// stamps the session's real `cwd` on its records. So when the decode names
/// something that isn't a directory, believe the transcript instead.
pub(crate) fn heal_workspace_path(project_dir: &std::path::Path, decoded: String) -> String {
    // A TCC-protected decode can't be checked with `is_dir` — that stat is the
    // permission dialog we spend `read_level_dirs` avoiding. It is also exactly
    // where the decode is known to be unreliable, so skip the check and let the
    // transcript speak.
    if !crate::tcc::is_tcc_protected(std::path::Path::new(&decoded))
        && std::path::Path::new(&decoded).is_dir()
    {
        return decoded;
    }
    newest_transcript_cwd(project_dir).unwrap_or(decoded)
}

/// The `cwd` recorded by the most recently written transcript in a project
/// directory, if any.
///
/// Newest wins because a session can be resumed from a checkout that has since
/// moved, which leaves older transcripts in the same directory pointing at a
/// path that no longer exists.
fn newest_transcript_cwd(project_dir: &std::path::Path) -> Option<String> {
    let mut transcripts: Vec<(std::time::SystemTime, std::path::PathBuf)> =
        std::fs::read_dir(project_dir)
            .ok()?
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.extension().is_some_and(|x| x == "jsonl"))
            .filter_map(|p| {
                let mtime = std::fs::metadata(&p).ok()?.modified().ok()?;
                Some((mtime, p))
            })
            .collect();
    transcripts.sort_by(|a, b| b.0.cmp(&a.0));
    transcripts
        .into_iter()
        .find_map(|(_, p)| super::parse::session_cwd_from_jsonl(&p))
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
        // Follow symlinks only when their target isn't TCC-protected — resolve
        // the target with `read_link` (not `canonicalize`, which stats through)
        // so listing `~` never stats into `~/Documents` et al.
        if !crate::tcc::readdir_is_followable_dir(entry.file_type(), &entry.path(), &|p| {
            crate::tcc::is_tcc_protected(p)
        }) {
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

/// Do `a` and `b` name the same workspace directory?
///
/// A workspace path reaches a comparison from two spellings that never agree
/// byte-for-byte on Windows: the decode side (`decode_workspace_path` /
/// `decode_walk`) always joins with `/` (e.g. `C:/code/proj`), while a live
/// process cwd from sysinfo comes back with native `\` separators
/// (`C:\code\proj`). A plain `==` therefore never matches a Fleet-spawned
/// session's process on Windows — liveness reads dead and workspace kills
/// find nothing. Separators are folded on every platform (both comparands are
/// directory paths from the OS or our own decode, so a literal `\` inside a
/// unix file name colliding with a real `/` boundary is not a practical
/// concern); case is folded only where the filesystem is case-insensitive
/// (Windows), keeping unix comparisons exact.
pub fn same_workspace_path(a: &str, b: &str) -> bool {
    workspace_path_key_with(cfg!(windows), a) == workspace_path_key_with(cfg!(windows), b)
}

/// Pure core of [`same_workspace_path`] — the `windows` flag stands in for
/// `cfg!(windows)` so the Windows folding rules are unit-testable on unix.
fn workspace_path_key_with(windows: bool, p: &str) -> String {
    let mut s = p.replace('\\', "/");
    while s.len() > 1 && s.ends_with('/') {
        s.pop();
    }
    if windows {
        s = s.to_lowercase();
    }
    s
}

#[cfg(test)]
mod heal_workspace_path_tests {
    use super::heal_workspace_path;

    /// Build a project dir holding one transcript whose records carry `cwd`.
    /// The first lines deliberately lack `cwd` — Claude Code opens a transcript
    /// with `queue-operation` records, so a "read the first line" shortcut would
    /// miss it.
    fn project_dir_with_cwd(tag: &str, cwd: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "fleet-heal-ws-{}-{}-{}",
            tag,
            std::process::id(),
            cwd.len()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("00000000-0000-0000-0000-000000000000.jsonl"),
            format!(
                "{}\n{}\n",
                r#"{"type":"queue-operation","operation":"enqueue"}"#,
                serde_json::json!({ "type": "user", "cwd": cwd })
            ),
        )
        .unwrap();
        dir
    }

    /// The bug from issue #105: when a level of the path can't be listed the
    /// greedy decode shreds every hyphenated directory name
    /// (`~/w/my-org/sub-project` → `~/w/my/org/sub/project`). The transcript's
    /// own `cwd` is authoritative and must win over that wreckage.
    #[test]
    fn nonexistent_decode_is_replaced_by_the_transcript_cwd() {
        let real = std::env::temp_dir().join(format!("fleet-heal-real-{}", std::process::id()));
        let hyphenated = real.join("my-org").join("sub-project");
        std::fs::create_dir_all(&hyphenated).unwrap();
        let cwd = hyphenated.to_string_lossy().into_owned();

        let dir = project_dir_with_cwd("mangled", &cwd);
        // What the naive one-dash-per-slash fallback produces for that path.
        let mangled = cwd.replace('-', "/");
        assert!(
            !std::path::Path::new(&mangled).is_dir(),
            "fixture is pointless unless the mangled path is really absent"
        );

        assert_eq!(heal_workspace_path(&dir, mangled), cwd);

        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::remove_dir_all(&real);
    }

    /// A decode that names a real directory is already right — the transcript
    /// must not be allowed to override it (a session can be resumed from a
    /// moved/copied checkout, so its recorded `cwd` may be stale).
    #[test]
    fn existing_decode_is_kept_even_when_the_transcript_disagrees() {
        let real = std::env::temp_dir().join(format!("fleet-heal-keep-{}", std::process::id()));
        std::fs::create_dir_all(&real).unwrap();
        let dir = project_dir_with_cwd("keep", "/somewhere/else/entirely");

        let decoded = real.to_string_lossy().into_owned();
        assert_eq!(heal_workspace_path(&dir, decoded.clone()), decoded);

        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::remove_dir_all(&real);
    }

    /// The reported scenario end to end (issue #105): a workspace with
    /// hyphenated directory names living under a TCC-protected folder.
    /// `read_level_dirs` refuses to list anything below `~/Documents` (listing it
    /// is what fires the macOS permission dialog), so the greedy walk has no
    /// filesystem to steer by and shreds `my-org/sub-project` into
    /// `my/org/sub/project`. The transcript has to rescue it.
    #[cfg(target_os = "macos")]
    #[test]
    fn tcc_protected_workspace_decodes_wrong_and_is_healed() {
        let _guard = crate::session::fleet_home_lock();
        let home = std::env::temp_dir().join(format!("fleet-heal-tcc-{}", std::process::id()));
        let workspace = home.join("Documents").join("my-org").join("sub-project");
        std::fs::create_dir_all(&workspace).unwrap();
        // SAFETY: every test touching FLEET_HOME serialises on the lock above.
        unsafe { std::env::set_var("FLEET_HOME", &home) };

        let real = workspace.to_string_lossy().into_owned();
        let decoded = super::decode_workspace_path(&super::encode_workspace_path(&real));
        assert_ne!(
            decoded, real,
            "fixture is pointless unless the decode really is broken under Documents"
        );

        let dir = project_dir_with_cwd("tcc", &real);
        assert_eq!(heal_workspace_path(&dir, decoded), real);

        unsafe { std::env::remove_var("FLEET_HOME") };
        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::remove_dir_all(&home);
    }

    /// No transcript carries a usable `cwd` → nothing better is known, so the
    /// decode stands rather than collapsing to an empty path.
    #[test]
    fn missing_transcript_cwd_leaves_the_decode_alone() {
        let dir = std::env::temp_dir().join(format!("fleet-heal-empty-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("a.jsonl"), "{\"type\":\"user\"}\n").unwrap();

        let decoded = "/no/such/place".to_string();
        assert_eq!(heal_workspace_path(&dir, decoded.clone()), decoded);

        let _ = std::fs::remove_dir_all(&dir);
    }
}

#[cfg(test)]
mod same_workspace_path_tests {
    use super::{same_workspace_path, workspace_path_key_with};

    #[test]
    fn separator_spellings_match_on_all_platforms() {
        assert!(same_workspace_path("C:\\code\\proj", "C:/code/proj"));
        assert!(same_workspace_path("/Users/foo/proj", "/Users/foo/proj"));
        assert!(!same_workspace_path("/Users/foo/proj", "/Users/foo/other"));
    }

    #[test]
    fn trailing_separator_is_ignored_but_root_survives() {
        assert!(same_workspace_path("/Users/foo/proj/", "/Users/foo/proj"));
        assert_eq!(workspace_path_key_with(false, "/"), "/");
    }

    #[test]
    fn windows_key_folds_case_unix_key_does_not() {
        assert_eq!(
            workspace_path_key_with(true, "C:\\Code\\Proj"),
            workspace_path_key_with(true, "c:/code/proj")
        );
        assert_ne!(
            workspace_path_key_with(false, "/Users/Foo"),
            workspace_path_key_with(false, "/users/foo")
        );
    }
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

#[cfg(test)]
mod claude_config_dir_tests {
    use super::{get_claude_config_json, get_claude_dir, real_home_dir};
    use std::path::PathBuf;

    /// Restores `CLAUDE_CONFIG_DIR` to its captured value on drop, so a panic
    /// mid-assert can't leak the test's override into other tests.
    struct CfgGuard(Option<std::ffi::OsString>);
    impl Drop for CfgGuard {
        fn drop(&mut self) {
            unsafe {
                match &self.0 {
                    Some(v) => std::env::set_var("CLAUDE_CONFIG_DIR", v),
                    None => std::env::remove_var("CLAUDE_CONFIG_DIR"),
                }
            }
        }
    }

    // One serial test (not several parallel #[test]s) because it mutates the
    // process-global `CLAUDE_CONFIG_DIR`. Only that single new var is touched —
    // nothing else reads it — so cross-test interference is effectively nil.
    #[test]
    fn resolves_config_dir_and_json_with_and_without_env() {
        let _g = CfgGuard(std::env::var_os("CLAUDE_CONFIG_DIR"));

        // Set → used verbatim; .claude.json sits at the config-dir root.
        unsafe { std::env::set_var("CLAUDE_CONFIG_DIR", "/tmp/fleet-cfg-honor-test") };
        assert_eq!(
            get_claude_dir(),
            Some(PathBuf::from("/tmp/fleet-cfg-honor-test"))
        );
        assert_eq!(
            get_claude_config_json(),
            Some(PathBuf::from("/tmp/fleet-cfg-honor-test/.claude.json"))
        );

        // Empty value is ignored (treated as unset).
        unsafe { std::env::set_var("CLAUDE_CONFIG_DIR", "") };
        assert_eq!(get_claude_dir(), real_home_dir().map(|h| h.join(".claude")));

        // Unset → fall back to ~/.claude, and .claude.json to the home ROOT
        // (outside .claude/) — the verified asymmetry.
        unsafe { std::env::remove_var("CLAUDE_CONFIG_DIR") };
        assert_eq!(get_claude_dir(), real_home_dir().map(|h| h.join(".claude")));
        assert_eq!(
            get_claude_config_json(),
            real_home_dir().map(|h| h.join(".claude.json"))
        );
    }
}

