//! Drift-guard for the Backend fan-out surface.
//!
//! `CLAUDE.md` documents that adding one feature really touches ~8 layers, but
//! only two of them (`impl Backend for LocalBackend` / `for RemoteBackend`) are
//! protected by the compiler — a missing method there fails to build. The other
//! layers are hand-maintained parallel dispatch tables with no single source of
//! truth, so they drift silently: a `Backend` method can gain a real Local impl
//! while RemoteBackend keeps the default stub (404 / no-op), or the HTTP server
//! can drop a route the remote client still calls, or the mobile client can call
//! a relay method the Rust dispatcher no longer handles. None of those break the
//! build; the feature just quietly doesn't work over the affected transport.
//!
//! This test turns three of those silent drifts into CI failures. It parses the
//! current source text (not a frozen snapshot), so it stays correct as the trait
//! evolves and only fires on genuine divergence.
//!
//! Checks:
//!   A. Remote impl completeness — every default-bodied `Backend` method that
//!      LocalBackend overrides must also be overridden by RemoteBackend, else the
//!      remote transport silently serves the default. (The exact drift that hid
//!      `apply_mcp_injector`.)
//!   B. Remote → server endpoint coverage — every HTTP path the RemoteBackend
//!      client calls must be served by `hooks_server::serve`, else remote 404s.
//!   C. mobile-web → relay method coverage — every `client.request("m", …)` the
//!      mobile web app issues must be handled by the `mobile_relay` dispatcher,
//!      else the mobile transport errors at runtime.
//!
//! Known drifts that pre-date this guard are listed in the per-check allow-lists
//! with a pointer to where they get fixed. Shrinking those lists to empty is the
//! acceptance criterion for the route-table unification (arch-extensibility P2).

use regex::Regex;
use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

// ── Source locations (relative to claw-fleet-core/) ─────────────────────────
const BACKEND_RS: &str = "src/backend.rs";
const HOOKS_SERVER_RS: &str = "src/hooks_server.rs";
const MOBILE_RELAY_RS: &str = "src/mobile_relay.rs";
const LOCAL_BACKEND_RS: &str = "../claw-fleet-desktop/src/local_backend.rs";
const REMOTE_RS: &str = "../claw-fleet-desktop/src/remote.rs";
const MOBILE_WEB_SRC: &str = "../mobile-web/src";

// ── Known, tracked drifts (fixed in arch-extensibility P2 once remote.rs /
//    hooks_server.rs are safe to edit; both edits collide with in-flight
//    worktrees at the time this guard was added, so they are deferred here
//    rather than fixed silently). ──────────────────────────────────────────

/// Default-bodied methods LocalBackend overrides but RemoteBackend does not.
/// `apply_mcp_injector`: RemoteBackend never proxies it, so a remote session
/// silently runs the no-op default. Fix = add the HTTP call in remote.rs.
const CHECK_A_KNOWN_DRIFT: &[&str] = &["apply_mcp_injector"];

/// Endpoints the remote client calls that `hooks_server` no longer serves.
/// Empty: the former `/auto_resume_config` drift was fixed by
/// fix-remote-autoresume-config (RemoteBackend now reads/writes the local
/// AutoResumeConfig directly instead of calling the dropped probe endpoint), so
/// the guard's stale-detection required removing it from this list.
const CHECK_B_KNOWN_DRIFT: &[&str] = &[];

/// mobile-web request methods not handled by the relay dispatcher. Empty — the
/// mobile surface is currently fully covered; keep it that way.
const CHECK_C_KNOWN_DRIFT: &[&str] = &[];

fn manifest_path(rel: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join(rel)
}

fn read(rel: &str) -> String {
    let p = manifest_path(rel);
    fs::read_to_string(&p)
        .unwrap_or_else(|e| panic!("drift-guard: cannot read {}: {e}", p.display()))
}

// ── Rust source scanner ─────────────────────────────────────────────────────
// A minimal state machine that walks a brace-delimited block and reports the
// `fn NAME` declarations at brace-depth 1, together with whether the signature
// ends in `;` (required/abstract) or `{` (default body). It skips string/char
// literals and line/block comments so a `fn` inside a doc comment or string is
// never mistaken for a real declaration.

#[derive(Debug, PartialEq)]
enum Term {
    Required, // signature ended with `;`
    Default,  // signature ended with `{`
}

/// Returns the byte range `[start, end]` (inclusive of both braces) of the block
/// that opens at the first `{` at or after `from`, balanced over braces while
/// skipping strings/comments.
fn block_after(src: &str, from: usize) -> (usize, usize) {
    let b = src.as_bytes();
    let mut i = from;
    while i < b.len() && b[i] != b'{' {
        i += 1;
    }
    let start = i;
    let mut depth = 0usize;
    let mut sc = Scanner::new();
    while i < b.len() {
        if let Some(c) = sc.step(b, &mut i) {
            if c == b'{' {
                depth += 1;
            } else if c == b'}' {
                depth -= 1;
                if depth == 0 {
                    return (start, i - 1);
                }
            }
        }
    }
    panic!("drift-guard: unbalanced braces starting at byte {start}");
}

/// Tracks string/char/comment state so structural bytes inside them are ignored.
struct Scanner {
    in_line_comment: bool,
    in_block_comment: bool,
    in_str: bool,
    in_char: bool,
    in_raw_str: bool,
    escaped: bool,
}

impl Scanner {
    fn new() -> Self {
        Scanner {
            in_line_comment: false,
            in_block_comment: false,
            in_str: false,
            in_char: false,
            in_raw_str: false,
            escaped: false,
        }
    }

    /// Advances `*i` by one byte. Returns `Some(byte)` when that byte is
    /// structural code (outside any string/comment), else `None`.
    fn step(&mut self, b: &[u8], i: &mut usize) -> Option<u8> {
        let c = b[*i];
        let next = b.get(*i + 1).copied().unwrap_or(0);

        if self.in_line_comment {
            *i += 1;
            if c == b'\n' {
                self.in_line_comment = false;
            }
            return None;
        }
        if self.in_block_comment {
            if c == b'*' && next == b'/' {
                self.in_block_comment = false;
                *i += 2;
            } else {
                *i += 1;
            }
            return None;
        }
        if self.in_str || self.in_raw_str {
            if self.in_str && !self.escaped && c == b'\\' {
                self.escaped = true;
                *i += 1;
                return None;
            }
            if !self.escaped && c == b'"' {
                self.in_str = false;
                self.in_raw_str = false;
            }
            self.escaped = false;
            *i += 1;
            return None;
        }
        if self.in_char {
            if !self.escaped && c == b'\\' {
                self.escaped = true;
                *i += 1;
                return None;
            }
            if !self.escaped && c == b'\'' {
                self.in_char = false;
            }
            self.escaped = false;
            *i += 1;
            return None;
        }

        // Not currently inside a string/comment: detect openers.
        if c == b'/' && next == b'/' {
            self.in_line_comment = true;
            *i += 2;
            return None;
        }
        if c == b'/' && next == b'*' {
            self.in_block_comment = true;
            *i += 2;
            return None;
        }
        if c == b'r' && (next == b'"' || next == b'#') {
            // raw string r"..." or r#"..."# — consume up to the opening quote.
            let mut j = *i + 1;
            while j < b.len() && b[j] == b'#' {
                j += 1;
            }
            if j < b.len() && b[j] == b'"' {
                self.in_raw_str = true;
                *i = j + 1;
                return None;
            }
        }
        if c == b'"' {
            self.in_str = true;
            *i += 1;
            return None;
        }
        if c == b'\'' {
            // Could be a char literal or a lifetime (`'a`). Treat `'x'` and
            // `'\x'` as char literals; a lifetime is `'` + ident + non-quote.
            let after = next;
            let after2 = b.get(*i + 2).copied().unwrap_or(0);
            let is_char = (after == b'\\') || (after2 == b'\'');
            if is_char {
                self.in_char = true;
                *i += 1;
                return None;
            }
            // lifetime: fall through, treat the quote as an ordinary byte.
        }
        *i += 1;
        Some(c)
    }
}

/// Given a `{...}` block slice, return `(fn_name, terminator)` for every
/// function declared at depth 1.
fn top_level_fns(block: &str) -> Vec<(String, Term)> {
    let b = block.as_bytes();
    let fn_re = Regex::new(r"fn\s+([a-z_][a-z0-9_]*)\s*(?:<[^>]*>)?\s*\(").unwrap();
    let mut out = Vec::new();
    let mut depth = 0usize;
    let mut sc = Scanner::new();
    let mut i = 0usize;
    while i < b.len() {
        // Try to recognise a `fn NAME(` starting exactly here, at depth 1,
        // as real code (Scanner not mid-string/comment).
        if depth == 1
            && !sc.in_line_comment
            && !sc.in_block_comment
            && !sc.in_str
            && !sc.in_char
            && !sc.in_raw_str
            && b[i] == b'f'
            && block[i..].starts_with("fn")
            && (i == 0 || !is_word(b[i - 1]))
        {
            if let Some(m) = fn_re.find(&block[i..]) {
                if m.start() == 0 {
                    let caps = fn_re.captures(&block[i..]).unwrap();
                    let name = caps[1].to_string();
                    // Find matching close paren for the `(` at m.end()-1.
                    let paren_open = i + m.end() - 1;
                    let term = terminator_after_params(block, paren_open);
                    out.push((name, term));
                    // Advance past the `fn NAME(` (no braces in it, so `depth`
                    // is unaffected) and let the scanner resume from the params.
                    i += m.end();
                    continue;
                }
            }
        }
        match sc.step(b, &mut i) {
            Some(b'{') => depth += 1,
            Some(b'}') => depth -= 1,
            _ => {}
        }
    }
    out
}

fn is_word(c: u8) -> bool {
    c == b'_' || c.is_ascii_alphanumeric()
}

/// From the `(` of a fn's parameter list, balance parens then scan for the first
/// `;` or `{` (the signature terminator). Return-types / where-clauses contain
/// neither, so the first hit is authoritative.
fn terminator_after_params(src: &str, paren_open: usize) -> Term {
    let b = src.as_bytes();
    let mut i = paren_open;
    let mut pd = 0i32;
    let mut sc = Scanner::new();
    // balance the parameter parens
    while i < b.len() {
        let c = b[i];
        if let Some(code) = sc.step(b, &mut i) {
            if code == b'(' {
                pd += 1;
            } else if code == b')' {
                pd -= 1;
                if pd == 0 {
                    break;
                }
            }
        }
        let _ = c;
    }
    // scan for `;` or `{`
    while i < b.len() {
        if let Some(code) = sc.step(b, &mut i) {
            if code == b';' {
                return Term::Required;
            }
            if code == b'{' {
                return Term::Default;
            }
        }
    }
    panic!("drift-guard: no terminator after fn params at byte {paren_open}");
}

// ── Extractors built on the scanner ─────────────────────────────────────────

/// All `Backend` trait methods and whether each has a default body.
fn trait_methods() -> Vec<(String, Term)> {
    let src = read(BACKEND_RS);
    let head = src
        .find("pub trait Backend")
        .expect("drift-guard: `pub trait Backend` not found");
    let (start, end) = block_after(&src, head);
    top_level_fns(&src[start..=end])
}

/// Method names overridden inside `impl … Backend for {ty}`.
fn impl_methods(src_rel: &str, ty: &str) -> HashSet<String> {
    let src = read(src_rel);
    let re = Regex::new(&format!(
        r"impl\s+(?:[A-Za-z_][A-Za-z0-9_]*::)*Backend\s+for\s+{ty}\s*\{{"
    ))
    .unwrap();
    let m = re
        .find(&src)
        .unwrap_or_else(|| panic!("drift-guard: `impl Backend for {ty}` not found in {src_rel}"));
    let (start, end) = block_after(&src, m.start());
    top_level_fns(&src[start..=end])
        .into_iter()
        .map(|(n, _)| n)
        .collect()
}

/// Normalise an endpoint/path literal: drop the query string and any `{param}`
/// tail, strip a trailing slash.
fn norm_path(p: &str) -> String {
    let p = p.split('?').next().unwrap_or(p);
    let p = p.split('{').next().unwrap_or(p);
    let p = p.trim_end_matches('/');
    if p.is_empty() {
        "/".to_string()
    } else {
        p.to_string()
    }
}

/// HTTP path prefixes the RemoteBackend client calls (literal + `format!`).
fn remote_endpoints() -> HashSet<String> {
    let src = read(REMOTE_RS);
    let calls = "get|get_bytes|get_ok|post_ok|post_json_ok|post_json|get_value|post_bytes";
    let lit = Regex::new(&format!(
        r#"\.(?:{calls})(?:::<[^>]*>)?\(\s*"(/[^"]*)""#
    ))
    .unwrap();
    let fmt = Regex::new(&format!(
        r#"\.(?:{calls})(?:::<[^>]*>)?\(\s*&?\s*format!\(\s*"(/[^"{{?]*)"#
    ))
    .unwrap();
    let mut out = HashSet::new();
    for c in lit.captures_iter(&src) {
        out.insert(norm_path(&c[1]));
    }
    for c in fmt.captures_iter(&src) {
        out.insert(norm_path(&c[1]));
    }
    out
}

/// Exact route literals + `starts_with` prefixes served by `hooks_server`.
fn hooks_server_routes() -> (HashSet<String>, Vec<String>) {
    let src = read(HOOKS_SERVER_RS);
    // A route literal is a "/..." string immediately followed (after optional
    // whitespace) by `=>`, an `if`-guard, or a `|` alternation.
    let arm = Regex::new(r#""(/[^"]*)"\s*(?:=>|if\b|\|)"#).unwrap();
    let mut routes = HashSet::new();
    for c in arm.captures_iter(&src) {
        routes.insert(norm_path(&c[1]));
    }
    let pre = Regex::new(r#"starts_with\(\s*"(/[^"]*)""#).unwrap();
    let prefixes: Vec<String> = pre.captures_iter(&src).map(|c| c[1].to_string()).collect();
    (routes, prefixes)
}

/// Method arms handled by the `mobile_relay` dispatcher.
fn mobile_relay_methods() -> HashSet<String> {
    let src = read(MOBILE_RELAY_RS);
    let arm = Regex::new(r#""([a-z_][a-z0-9_]*)"\s*=>"#).unwrap();
    let mut out: HashSet<String> = arm.captures_iter(&src).map(|c| c[1].to_string()).collect();
    // Multi-pattern arms: "a" | "b" => …
    let alt = Regex::new(r#""([a-z_][a-z0-9_]*)""#).unwrap();
    for line in src.lines() {
        if line.contains('|') && line.contains("=>") {
            for c in alt.captures_iter(line) {
                out.insert(c[1].to_string());
            }
        }
    }
    out
}

/// Method-name literals mobile-web passes to the relay client (`client.request`).
fn mobile_web_request_methods() -> HashSet<String> {
    let re = Regex::new(r#"client\.request(?:<[^>]*>)?\(\s*["'`]([a-z_][a-z0-9_]*)"#).unwrap();
    let mut out = HashSet::new();
    collect_ts(&manifest_path(MOBILE_WEB_SRC), &re, &mut out);
    out
}

fn collect_ts(dir: &Path, re: &Regex, out: &mut HashSet<String>) {
    let entries = match fs::read_dir(dir) {
        Ok(e) => e,
        Err(e) => panic!("drift-guard: cannot read mobile-web dir {}: {e}", dir.display()),
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            if path.file_name().and_then(|n| n.to_str()) == Some("node_modules") {
                continue;
            }
            collect_ts(&path, re, out);
            continue;
        }
        let name = match path.file_name().and_then(|n| n.to_str()) {
            Some(n) => n,
            None => continue,
        };
        let is_ts = name.ends_with(".ts") || name.ends_with(".tsx");
        if !is_ts || name.contains(".test.") {
            continue;
        }
        let src = fs::read_to_string(&path).unwrap_or_default();
        for c in re.captures_iter(&src) {
            out.insert(c[1].to_string());
        }
    }
}

fn sorted(set: &HashSet<String>) -> Vec<String> {
    let mut v: Vec<String> = set.iter().cloned().collect();
    v.sort();
    v
}

// ── The three drift checks ──────────────────────────────────────────────────

#[test]
fn check_a_remote_impl_covers_local_default_methods() {
    let methods = trait_methods();
    // Sanity: the parse must find a plausible trait, not silently zero out.
    let default_count = methods.iter().filter(|(_, t)| *t == Term::Default).count();
    assert!(
        methods.len() > 80 && default_count > 5,
        "drift-guard: trait parse looks wrong ({} methods, {} default) — parser or file moved",
        methods.len(),
        default_count
    );

    let local = impl_methods(LOCAL_BACKEND_RS, "LocalBackend");
    let remote = impl_methods(REMOTE_RS, "RemoteBackend");
    let known: HashSet<&str> = CHECK_A_KNOWN_DRIFT.iter().copied().collect();

    let mut new_drift = Vec::new();
    let mut fixed = Vec::new();
    for (name, term) in &methods {
        if *term != Term::Default {
            continue;
        }
        let local_overrides = local.contains(name);
        let remote_overrides = remote.contains(name);
        let is_drift = local_overrides && !remote_overrides;
        let listed = known.contains(name.as_str());
        if is_drift && !listed {
            new_drift.push(name.clone());
        }
        if !is_drift && listed {
            fixed.push(name.clone());
        }
    }

    assert!(
        fixed.is_empty(),
        "drift-guard: these methods are in CHECK_A_KNOWN_DRIFT but no longer drift — \
         RemoteBackend now covers them (or the method was removed). Delete them from the \
         allow-list: {fixed:?}"
    );
    assert!(
        new_drift.is_empty(),
        "drift-guard (Check A): these default-bodied Backend methods have a LocalBackend \
         override but NO RemoteBackend override, so remote sessions silently get the default \
         stub. Add a RemoteBackend impl (see CLAUDE.md step 4), or if the default is truly \
         correct for remote, add the name to CHECK_A_KNOWN_DRIFT with a justification: \
         {new_drift:?}"
    );
}

#[test]
fn check_b_remote_endpoints_are_served() {
    let endpoints = remote_endpoints();
    let (routes, prefixes) = hooks_server_routes();
    assert!(
        endpoints.len() > 80 && routes.len() > 80,
        "drift-guard: endpoint/route parse looks wrong ({} endpoints, {} routes)",
        endpoints.len(),
        routes.len()
    );
    let known: HashSet<&str> = CHECK_B_KNOWN_DRIFT.iter().copied().collect();

    let covered = |e: &str| routes.contains(e) || prefixes.iter().any(|p| e.starts_with(p));

    let mut new_drift = Vec::new();
    for e in sorted(&endpoints) {
        if !covered(&e) && !known.contains(e.as_str()) {
            new_drift.push(e);
        }
    }
    let stale: Vec<&&str> = CHECK_B_KNOWN_DRIFT
        .iter()
        .filter(|e| endpoints.contains(**e) && covered(e) || !endpoints.contains(**e))
        .collect();

    assert!(
        stale.is_empty(),
        "drift-guard: these paths are in CHECK_B_KNOWN_DRIFT but are now served (or no longer \
         called by remote.rs). Remove them from the allow-list: {stale:?}"
    );
    assert!(
        new_drift.is_empty(),
        "drift-guard (Check B): RemoteBackend calls these HTTP paths but `hooks_server::serve` \
         does not route them, so remote sessions 404 silently. Add the route to hooks_server \
         (see CLAUDE.md step 3), fix the remote path, or allow-list with justification in \
         CHECK_B_KNOWN_DRIFT: {new_drift:?}"
    );
}

#[test]
fn check_c_mobile_web_methods_are_handled_by_relay() {
    let called = mobile_web_request_methods();
    let handled = mobile_relay_methods();
    assert!(
        called.len() > 15 && handled.len() > 15,
        "drift-guard: mobile parse looks wrong ({} called, {} handled)",
        called.len(),
        handled.len()
    );
    let known: HashSet<&str> = CHECK_C_KNOWN_DRIFT.iter().copied().collect();

    let mut new_drift = Vec::new();
    for m in sorted(&called) {
        if !handled.contains(&m) && !known.contains(m.as_str()) {
            new_drift.push(m);
        }
    }
    let stale: Vec<&&str> = CHECK_C_KNOWN_DRIFT
        .iter()
        .filter(|m| handled.contains(**m) || !called.contains(**m))
        .collect();

    assert!(
        stale.is_empty(),
        "drift-guard: these methods are in CHECK_C_KNOWN_DRIFT but are now handled (or no longer \
         called by mobile-web). Remove them from the allow-list: {stale:?}"
    );
    assert!(
        new_drift.is_empty(),
        "drift-guard (Check C): mobile-web calls `client.request(\"m\", …)` for these methods but \
         the `mobile_relay` dispatcher does not handle them, so the mobile app errors at runtime. \
         Add the arm to mobile_relay, fix the method name, or allow-list in CHECK_C_KNOWN_DRIFT: \
         {new_drift:?}"
    );
}
