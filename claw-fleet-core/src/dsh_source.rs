//! [`AgentSource`] for dsh (DeepSeek Harness).
//!
//! Read-only half: list sessions and read their history. Unlike the other two
//! sources, dsh keeps its transcripts in a zstd-compressed private layout under
//! `~/.dsh/sessions` and exposes them only through a running `dsh web` server —
//! so this source talks RPC ([`crate::dsh_client`]) to a server it owns
//! ([`crate::dsh_server`]) rather than reading files.
//!
//! One server covers every workspace: `session.list` returns all sessions the
//! harness home knows about regardless of their cwd, including ones written by
//! a completely different profile (a `dsh --profile headless` run shows up
//! here). The server's own cwd only decides where *new* sessions would root.
//!
//! # No account or quota surface (measured — do not re-probe)
//!
//! [`AgentSource::fetch_account`], [`AgentSource::fetch_usage`] and
//! [`AgentSource::usage_summary`] stay at their refusing defaults for dsh, and
//! that is not an omission: **dsh exposes nothing to implement them with.**
//!
//! The `/api` method catalog (read off `@deepseek-ai/dsh-host-apiproxy`, then
//! called against a live server) is `agentPreset.*`, `credentials.*`, `goal.*`,
//! `host.*`, `llm.*`, `session.*`, `settings.*`, `skill.list`, `subagent.*`,
//! `workspace.*`. There is no `account.*`, `usage.*`, `quota.*`, or
//! `rateLimit.*`. Of the near misses:
//!
//! * `credentials.describe` takes `{refs: [...]}` and reports whether *those
//!   credential refs* are set — presence, not an account or a balance.
//! * `llm.providers` returns the provider catalog with an `active` flag
//!   (measured on this machine: `openrouter` and `deepseek-official` active,
//!   ~35 others declared-but-inactive). No plan, no limit, no reset.
//! * `host.describe` returns `{version, cwd, provider, model, attachedSessions,
//!   canOpenPath}` — the host's default route, not the session's, and no usage.
//! * `QUOTA_EXCEEDED` / `RATE_LIMIT` exist only as *error classifications* an
//!   adapter assigns to a request that already failed (HTTP 429 and friends in
//!   `dsh-llm-deepseek` / `dsh-llm-pi-ai`). They are not a pollable window with
//!   a utilization and a reset time, which is what [`crate::backend::UsageBar`]
//!   needs.
//!
//! This follows from what dsh *is*: a bring-your-own-key harness. The quota
//! belongs to whichever provider the user configured, so any real usage view for
//! a dsh user has to come from that provider's own API (for the OpenRouter case,
//! see the generation-cost path in [`dsh_token_breakdown`]'s neighbourhood), not
//! from dsh. Per-session token accounting is a different question and dsh *does*
//! answer it — see [`dsh_token_breakdown`], which reads the `session.list`
//! projections `@deepseek-ai/dsh-token-meter` publishes.

use std::path::PathBuf;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use base64::Engine as _;
use serde_json::{json, Value};

use crate::agent_source::{AgentSource, WatchStrategy};
use crate::dsh_client::DshClient;
use crate::dsh_events::DshEventWatcher;
use crate::dsh_server::DshServer;
use crate::session::{SessionInfo, SessionStatus};

/// URI scheme for this source's `SessionInfo::jsonl_path`.
pub const DSH_URI_PREFIX: &str = "dsh://";

/// dsh has no filesystem signal Fleet can watch (its logs are compressed and
/// written by a server that owns them), so the session list is polled.
///
/// The `events.mux` / `events.host` downlinks do not replace this poll: they
/// carry a session's *phase*, not its roster or its token totals, and the
/// registry's watch loop is a shared thread no single source may block. So the
/// poll stays the roster refresh and [`DshEventWatcher`] sharpens the status of
/// what it returns.
const POLL_INTERVAL: Duration = Duration::from_secs(3);

/// How long one `session.list` answer may be reused for a later roster scan.
///
/// Deliberately shorter than [`POLL_INTERVAL`], so the registry's own cadence
/// always reaches dsh and the roster cannot freeze behind the cache; what it
/// absorbs is the *extra* scans — the desktop rescan and, in `fleet serve`, one
/// per scan-bearing HTTP route — that pile up while a slow dsh call is in
/// flight.
pub const ROSTER_TTL: Duration = Duration::from_secs(2);

/// Fleet's single `dsh web` instance, shared by every [`DshSource`] in the
/// process rather than owned by one.
///
/// It has to be process-global: `agent_source::spawn_session` and
/// `resume_session` call `build_sources()` on every invocation, so the source
/// they route through is a throwaway. An instance-owned server would be started
/// for the call and killed by `Drop` on the way out — taking the turn that had
/// just been admitted with it. One shared server also matches what the source
/// already does, which is root itself at the harness home and observe every
/// workspace from there.
///
/// Because it is never dropped, [`shutdown`] is the only thing that stops it,
/// and every process exit path must call it: `dsh web` has no authentication
/// layer, so a leaked child is an open door onto every session on the machine.
static SERVER: OnceLock<Mutex<Option<DshServer>>> = OnceLock::new();

/// Follower of [`SERVER`]'s two downlinks, rebuilt whenever the server lands on
/// a new port.
static WATCHER: OnceLock<Mutex<Option<DshEventWatcher>>> = OnceLock::new();

/// The last `session.list` answer, with the instant it landed. Shared by every
/// roster scan in the process — see [`DshSource::roster`].
static ROSTER: OnceLock<Mutex<Option<(std::time::Instant, Value)>>> = OnceLock::new();

/// Held by whoever is currently fetching the roster, so simultaneous scans wait
/// on that one flight instead of each starting its own.
static ROSTER_FETCH: OnceLock<Mutex<()>> = OnceLock::new();

fn server_slot() -> &'static Mutex<Option<DshServer>> {
    SERVER.get_or_init(|| Mutex::new(None))
}

fn watcher_slot() -> &'static Mutex<Option<DshEventWatcher>> {
    WATCHER.get_or_init(|| Mutex::new(None))
}

fn roster_slot() -> &'static Mutex<Option<(std::time::Instant, Value)>> {
    ROSTER.get_or_init(|| Mutex::new(None))
}

fn roster_fetch_slot() -> &'static Mutex<()> {
    ROSTER_FETCH.get_or_init(|| Mutex::new(()))
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// Stop Fleet's `dsh web` and its downlink follower, if either is running.
///
/// Idempotent, and a no-op when dsh was never used. Every long-lived Fleet
/// process must call this on exit — see [`SERVER`].
pub fn shutdown() {
    // Watcher first: it holds sockets onto the server it follows.
    *lock(watcher_slot()) = None;
    if let Some(mut server) = lock(server_slot()).take() {
        server.stop();
    }
    // The roster described sessions as one server saw them; a later server is a
    // different view, so it must not inherit this one.
    *lock(roster_slot()) = None;
}

pub struct DshSource;

impl Default for DshSource {
    fn default() -> Self {
        Self::new()
    }
}

impl DshSource {
    pub fn new() -> Self {
        Self
    }

    /// Run `f` against a live server, starting or restarting it as needed.
    ///
    /// The client is rebuilt per call on purpose: a restart lands on a fresh
    /// OS-assigned port, so a memoized client would silently point at a dead
    /// listener.
    ///
    /// **The lock is released before `f` runs.** It guards the `Option<DshServer>`
    /// — the start / restart / port handoff — and nothing about an in-flight RPC
    /// needs it: a [`DshClient`] is just a port plus an HTTP client, and `dsh web`
    /// answers concurrent requests. Holding it across the call instead made every
    /// dsh RPC in the process serial, so the `session.history` behind the 对话 tab
    /// queued behind the 3s roster poll — measured at 4.15s for a call that costs
    /// 50ms, and worse than one poll's worth because `std`'s mutex lets a later
    /// poller barge past a waiting reader (`tests/dsh_read_starvation.rs`).
    ///
    /// The narrow race this opens: another thread may restart the server while
    /// `f` is on the wire, and that call then fails against a dead port. That is
    /// the same outcome the call had before — the restart happens precisely
    /// because the server died under it — and the next call rebuilds its client
    /// off the new port.
    fn with_client<T>(&self, f: impl FnOnce(&DshClient) -> Result<T, String>) -> Result<T, String> {
        let client = {
            let mut guard = lock(server_slot());

            match guard.as_mut() {
                Some(server) => server.ensure_alive()?,
                None => {
                    let binary = crate::dsh_server::discover().ok_or_else(|| {
                        "dsh is not installed (npm i -g @deepseek-ai/dsh)".to_string()
                    })?;
                    // Before adding one, take away any this machine is still
                    // carrying from a Fleet that died without stopping its own.
                    // Servers whose owner is alive are left alone, so this never
                    // touches a concurrently running Fleet's instance. The
                    // signature sweep additionally catches servers the registry
                    // never heard of (temp-FLEET_HOME spawns, dropped records).
                    crate::dsh_server::reap_orphans();
                    crate::dsh_server::sweep_unregistered_orphans();
                    // Root the server at the harness home rather than a project:
                    // observation spans every workspace, and a server rooted in a
                    // directory that later disappears would fail to restart.
                    let cwd = crate::session::real_home_dir()
                        .ok_or_else(|| "cannot determine home dir".to_string())?;
                    *guard = Some(DshServer::start(&binary, &cwd)?);
                }
            }

            let server = guard.as_ref().expect("server started above");
            Self::ensure_watcher(server.port());
            server.client()?
        };

        f(&client)
    }

    /// One `session.history` page.
    ///
    /// `before_seq` walks backwards through the history (absent = the tail page,
    /// which additionally carries the in-flight partial); `max_messages` is
    /// dsh's own unit — whole append-origin messages *and every raw event they
    /// own*, chunks included. That is the reason [`history_with`] folds pages
    /// onto a cache rather than asking for the whole thing on every poll.
    fn fetch_history(
        &self,
        id: &str,
        before_seq: Option<i64>,
        max: Option<usize>,
    ) -> Result<Value, String> {
        let mut payload = json!({ "sessionId": id });
        if let Some(before) = before_seq {
            payload["beforeSeq"] = json!(before);
        }
        if let Some(n) = max {
            payload["maxMessages"] = json!(n);
        }
        self.with_client(|client| {
            client
                .call("session.history", payload.clone())
                .map_err(Into::into)
        })
    }

    /// Turn the image references in one session's records into renderable store
    /// paths (see [`crate::dsh_attachments::resolve_image_blocks`]).
    ///
    /// The fetch is `session.attachment`, which answers only for an attachment
    /// the named session's log references — so this cannot be turned into a
    /// read of arbitrary attachments by handing it another session's id. It runs
    /// at most once per image ever, since the bytes then live in the store.
    fn resolve_images(&self, session_id: &str, records: &mut [Value]) {
        crate::dsh_attachments::resolve_image_blocks(records, |attachment_id| {
            let answer = self
                .with_client(|client| {
                    client
                        .call(
                            "session.attachment",
                            json!({ "sessionId": session_id, "attachmentId": attachment_id }),
                        )
                        .map_err(Into::into)
                })
                .ok()?;
            let data = answer.get("data")?.as_str()?;
            base64::engine::general_purpose::STANDARD.decode(data).ok()
        });
    }

    /// The session roster, shared across simultaneous scans.
    ///
    /// Fleet scans the roster from several places at once — the registry watch
    /// loop, the desktop's rescan, and one per scan-bearing `fleet serve` route.
    /// Each used to mean its own `session.list`, which is fine against a fast dsh
    /// and ruinous against a slow one: a frontend polling faster than the call
    /// completes queues scans without bound (measured through `fleet serve` with
    /// a 5s dsh: `/sessions` 61s, `/health` 26–29s behind the pile).
    ///
    /// So a scan either reuses an answer younger than [`ROSTER_TTL`], or — when
    /// another scan is already fetching — waits for that flight and reuses its
    /// answer. N simultaneous scans cost one call, and because the TTL is shorter
    /// than [`POLL_INTERVAL`] the registry's own cadence still reaches dsh, so the
    /// roster cannot freeze.
    ///
    /// Only the roster takes this path. Interactive reads (`session.history`,
    /// `dsh_token_breakdown`) go straight to [`with_client`] as before: making
    /// them queue behind a scan's in-flight call is the very thing this module
    /// just stopped doing.
    ///
    /// [`with_client`]: Self::with_client
    fn roster(&self) -> Result<Value, String> {
        if let Some(fresh) = Self::fresh_roster() {
            return Ok(fresh);
        }

        let _flight = lock(roster_fetch_slot());
        // Another scan's flight may have landed while this one queued for it.
        if let Some(fresh) = Self::fresh_roster() {
            return Ok(fresh);
        }

        let value =
            self.with_client(|client| client.call("session.list", json!({})).map_err(Into::into))?;
        *lock(roster_slot()) = Some((std::time::Instant::now(), value.clone()));
        Ok(value)
    }

    /// The cached roster, if it is younger than [`ROSTER_TTL`].
    fn fresh_roster() -> Option<Value> {
        lock(roster_slot())
            .as_ref()
            .filter(|(at, _)| at.elapsed() < ROSTER_TTL)
            .map(|(_, value)| value.clone())
    }

    /// Point the downlink follower at `port`, replacing one that is following a
    /// stale port.
    ///
    /// Called from inside [`with_client`], so the lock order is always
    /// server-then-watcher; nothing takes them the other way round.
    ///
    /// [`with_client`]: Self::with_client
    fn ensure_watcher(port: u16) {
        let mut guard = lock(watcher_slot());
        if guard.as_ref().is_some_and(|w| w.port() == port) {
            return;
        }
        // Assigning drops the old watcher, which stops its follower thread. Its
        // live view goes with it: those phases belong to sessions as seen by a
        // server that no longer exists.
        *guard = Some(DshEventWatcher::start(port));
    }

    /// The port Fleet's `dsh web` instance is listening on, or `None` before
    /// the first RPC starts it. Diagnostics, and how the tests drive a turn
    /// through the same server this source observes.
    pub fn server_port(&self) -> Option<u16> {
        lock(server_slot()).as_ref().map(DshServer::port)
    }

    /// The launcher pid, or 0 when the server is not up. Reported as a dsh
    /// session's pid — see [`DshServer::pid`] for why it is shared.
    fn server_pid() -> u32 {
        lock(server_slot()).as_ref().map(DshServer::pid).unwrap_or(0)
    }

    /// The phase the downlinks report for `session_id`, if any is fresh.
    fn live_phase(&self, session_id: &str) -> Option<SessionStatus> {
        lock(watcher_slot())
            .as_ref()
            .and_then(|w| w.phase_of(session_id))
    }

    /// Strip the `dsh://` scheme off a session URI.
    fn session_id_of(path: &str) -> Option<&str> {
        path.strip_prefix(DSH_URI_PREFIX).filter(|s| !s.is_empty())
    }

    /// Point an existing session at a model, when the caller named one.
    ///
    /// Also the step that makes resume work: a session written by another
    /// process is cold to this server until something touches it, and
    /// `session.selectModel` loads it (verified against a session created by a
    /// different `dsh web` instance).
    fn select_model(
        client: &DshClient,
        session_id: &str,
        model: Option<&str>,
        effort: Option<&str>,
    ) -> Result<(), String> {
        let Some((provider, model)) = model.and_then(split_model) else {
            return Ok(());
        };
        let mut payload = json!({
            "sessionId": session_id,
            "provider": provider,
            "model": model,
        });
        if let Some(effort) = effort.filter(|e| !e.is_empty()) {
            payload["reasoningEffort"] = json!(effort);
        }
        client
            .call("session.selectModel", payload)
            .map(|_| ())
            .map_err(Into::into)
    }

    /// Hand a prompt to a session. Returns once dsh has admitted the turn, not
    /// when it finishes — completion arrives on the mux as `turn/end`.
    ///
    /// The content is built by [`crate::dsh_attachments::prompt_content`], which
    /// lifts any attached images out of the composer's `Context files:` block
    /// into real image parts: a path alone never reaches dsh's model, since dsh
    /// admits images only as content parts on this call. A deployment that
    /// refuses those images (a text-only model route, an over-limit batch) gets
    /// the prompt again as plain text rather than losing the turn — see
    /// [`crate::dsh_attachments::send_with_text_fallback`].
    fn prompt(client: &DshClient, session_id: &str, prompt: &str) -> Result<(), String> {
        crate::dsh_attachments::send_with_text_fallback(prompt, |content| {
            client
                .call(
                    "session.prompt",
                    json!({
                        "sessionId": session_id,
                        // "queue" appends to the session's inbox; "steer" would
                        // cut into a turn already running, which is not what
                        // either of Fleet's launch paths means.
                        "mode": "queue",
                        "content": content,
                    }),
                )
                .map(|_| ())
        })
        .map_err(Into::into)
    }

    /// Arrange for `on_exit` to fire when `session_id`'s turn ends.
    ///
    /// Returns a handle that fires it with `false` if the caller never gets the
    /// turn started — registering *before* prompting is what closes the race
    /// where a very short turn ends before the callback is in place.
    fn arm_turn_end(session_id: &str, on_exit: Box<dyn FnOnce(bool) + Send>) -> ArmedCallback {
        let cell: ArmedCallback = Arc::new(Mutex::new(Some(on_exit)));
        let for_watcher = cell.clone();
        let fired = move |ok: bool| {
            if let Some(cb) = lock(&for_watcher).take() {
                cb(ok);
            }
        };
        match lock(watcher_slot()).as_ref() {
            Some(watcher) => watcher.on_turn_end(session_id, Box::new(fired)),
            // No follower means no completion signal will ever arrive; say so
            // now rather than leaving the caller's slot held forever.
            None => fired(false),
        }
        cell
    }
}

/// A `on_exit` callback that has been handed to the watcher but can still be
/// reclaimed by the caller if the turn never starts. Whoever takes it first runs
/// it, so it fires exactly once.
type ArmedCallback = Arc<Mutex<Option<Box<dyn FnOnce(bool) + Send>>>>;

// PRD/TASKS.md injection for dsh lives in `dsh-plugin/index.js`, fed by
// `fleet dsh-context` (see [`crate::dsh_plugin`]). It used to be prepended to
// the prompt here, which made it part of a `source.kind = "user"` message —
// and dsh's session-title provider frames the first such message for its title
// model under a hard 4096-byte budget that rejects rather than truncates. The
// plan block alone runs 4.7-5.3 KB, so every Fleet-spawned dsh session lost its
// LLM title and was named `<system-reminder> The workspace `TASKS.m` instead.
// A plugin-sourced message reaches the same request without being an eligible
// title message.

/// The agent preset a new session in `workspace_path` should be created under,
/// or `None` to let dsh mount its own default.
///
/// Only the chat workspace asks for one: dsh loads `$DSH_HOME/AGENTS.md` — where
/// Fleet writes its engineering doctrine — unconditionally, and `session.create`
/// carries no per-session instruction knob, so the preset is the only seam (see
/// [`crate::dsh_chat_preset`]). Two RPCs: `agentPreset.list` to learn the
/// deployment's own default, `agentPreset.read` to get its composition text,
/// then Fleet re-authors `fleet-chat` from it.
///
/// `call` is a parameter so the orchestration is testable without a live server,
/// the same way [`history_with`] takes its `fetch`.
///
/// Every failure degrades to `None` — a chat session that carries the doctrine
/// is worse than one that doesn't, but no session at all is worse than both, and
/// the chat brief still wins on precedence (dsh renders the project file after
/// the user-global one and tells the model the specific file takes precedence).
fn resolve_chat_preset<F>(workspace_path: &str, call: F) -> Option<String>
where
    F: Fn(&str, Value) -> Result<Value, String>,
{
    if !crate::dsh_chat_preset::wants_chat_preset(workspace_path) {
        return None;
    }
    let listed = call("agentPreset.list", json!({}))
        .inspect_err(|e| log_chat_preset_skip(&format!("agentPreset.list failed: {e}")))
        .ok()?;
    let source_id = crate::dsh_chat_preset::default_preset_id(&listed).or_else(|| {
        log_chat_preset_skip("agentPreset.list reported no usable default preset");
        None
    })?;
    let read = call("agentPreset.read", json!({ "agentPreset": source_id }))
        .inspect_err(|e| log_chat_preset_skip(&format!("agentPreset.read failed: {e}")))
        .ok()?;
    let composition = read.get("content").and_then(Value::as_str).or_else(|| {
        log_chat_preset_skip("agentPreset.read answered without a composition");
        None
    })?;
    crate::dsh_chat_preset::ensure_chat_preset(composition)
        .inspect_err(|e| log_chat_preset_skip(e))
        .ok()
}

/// Say why a chat session is about to run with the doctrine loaded. Silent
/// degradation here is exactly the failure the preset exists to prevent, so it
/// is never swallowed.
fn log_chat_preset_skip(reason: &str) {
    eprintln!("fleet dsh: chat preset unavailable, session keeps the default composition — {reason}");
}

/// Split Fleet's single `model` string into dsh's `provider` + `model` pair.
///
/// dsh addresses a model as two fields, Fleet's [`SpawnSpec`] carries one
/// string, so the first `/` separates them: `openrouter/anthropic/claude-haiku-4.5`
/// → provider `openrouter`, model `anthropic/claude-haiku-4.5`. A string with no
/// `/` names no provider and is ignored, leaving the session on the model the
/// harness itself is configured with.
///
/// [`SpawnSpec`]: crate::agent_source::SpawnSpec
fn split_model(model: &str) -> Option<(&str, &str)> {
    let (provider, rest) = model.trim().split_once('/')?;
    if provider.is_empty() || rest.is_empty() {
        return None;
    }
    Some((provider, rest))
}

/// dsh's own id shape. Fleet mints one when the caller did not pre-assign an id,
/// and prefixes a bare one so every dsh session id looks the same on the wire.
fn normalize_session_id(id: Option<&str>) -> String {
    match id.map(str::trim).filter(|s| !s.is_empty()) {
        Some(id) if id.starts_with("session-") => id.to_string(),
        Some(id) => format!("session-{id}"),
        None => format!("session-{}", uuid::Uuid::new_v4()),
    }
}

/// Read a token count out of a `session.list` projections block.
fn projection_u64(projections: &Value, key: &str, field: &str) -> u64 {
    projections
        .get("values")
        .and_then(|v| v.get(key))
        .and_then(|v| v.get(field))
        .and_then(Value::as_u64)
        .unwrap_or(0)
}

/// Map one `session.list` item onto Fleet's [`SessionInfo`].
///
/// dsh hands over its own projections (title, token usage, context breakdown)
/// already folded, so this is a field rename rather than a transcript parse.
/// Everything Fleet derives from a transcript it does not have here (entrypoint,
/// pid liveness, todos, skills) stays at its default.
pub(crate) fn session_info_from_list_item(item: &Value) -> Option<SessionInfo> {
    let id = item.get("sessionId").and_then(Value::as_str)?.to_string();
    let workspace_path = item
        .get("cwd")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let updated_at = item.get("updatedAt").and_then(Value::as_u64).unwrap_or(0);
    let running = item
        .get("running")
        .and_then(Value::as_bool)
        .unwrap_or(false);

    let projections = item
        .get("projections")
        .cloned()
        .unwrap_or_else(|| json!({}));
    let title = projections
        .get("values")
        .and_then(|v| v.get("title"))
        .and_then(Value::as_str)
        .map(str::to_string);

    // dsh reports the four usage buckets separately; Fleet's "input" total is
    // everything sent to the API, cache re-reads included — the same口径 as the
    // other two sources.
    let output = projection_u64(&projections, "tokenUsage", "outputTokens");
    let input = projection_u64(&projections, "tokenUsage", "uncachedInputTokens")
        + projection_u64(&projections, "tokenUsage", "cacheReadTokens")
        + projection_u64(&projections, "tokenUsage", "cacheWriteTokens");

    // Fleet ownership. dsh has no originator channel of its own (every session
    // runs inside one shared server, so there is no per-session environment to
    // stamp), so the spawn note written by `DshSource::spawn` is the only
    // evidence — and the desktop's Tasks list needs *both* fields, not one.
    let fleet_spawned = crate::launch_spec::was_fleet_spawned(&id);
    let entrypoint = fleet_spawned.then(|| {
        crate::launch_spec::entrypoint_of(&id)
            .unwrap_or_else(|| crate::session_launch::NEW_SESSION_ENTRYPOINT.to_string())
    });

    Some(SessionInfo {
        // The roster names no route (see `latest_route`), so this is
        // whatever reading the session's own history last taught this process.
        // `None` until something reads it — the desktop's detail pane does so
        // the moment a session is opened.
        model: known_model(&id),
        // Same story for the effort, with one extra source: for a session Fleet
        // spawned with an explicit `--effort`, the launch spec is a record of
        // what Fleet asked for (and applied via `session.selectModel`). The log
        // header wins when known — it is the effort a real request went out
        // with, and it tracks a later change the spawn record cannot.
        thinking_level: known_effort(&id).or_else(|| crate::launch_spec::effort_of(&id)),
        id: id.clone(),
        workspace_name: crate::session::workspace_name(&workspace_path),
        workspace_path,
        ai_title: title,
        entrypoint,
        fleet_spawned,
        // `running` is the only liveness bit `session.list` carries. Finer
        // phases (Thinking / Streaming / Executing / Processing) come off the
        // mux downlink and are overlaid by `scan_sessions`.
        status: if running {
            SessionStatus::Active
        } else {
            SessionStatus::Idle
        },
        total_output_tokens: output,
        total_input_tokens: input,
        last_activity_ms: updated_at,
        agent_last_activity_ms: updated_at,
        created_at_ms: updated_at,
        jsonl_path: format!("{DSH_URI_PREFIX}{id}"),
        agent_source: "dsh".to_string(),
        ..Default::default()
    })
}

impl AgentSource for DshSource {
    fn name(&self) -> &'static str {
        "dsh"
    }

    fn uri_prefix(&self) -> &'static str {
        DSH_URI_PREFIX
    }

    fn is_available(&self) -> bool {
        crate::dsh_server::is_available()
    }

    fn scan_sessions(&self) -> Vec<SessionInfo> {
        match self.roster() {
            Ok(value) => value
                .get("items")
                .and_then(Value::as_array)
                .map(|items| {
                    items
                        .iter()
                        .filter_map(session_info_from_list_item)
                        .map(|mut info| {
                            // The poll only knows running/not-running; the
                            // downlinks know which phase of the turn it is in.
                            if let Some(phase) = self.live_phase(&info.id) {
                                info.status = phase;
                            }
                            info
                        })
                        .collect()
                })
                .unwrap_or_default(),
            Err(e) => {
                crate::log_debug(&format!("dsh scan_sessions: {e}"));
                Vec::new()
            }
        }
    }

    fn get_messages(&self, path: &str) -> Result<Vec<Value>, String> {
        let id = Self::session_id_of(path).ok_or_else(|| format!("invalid dsh URI: {path}"))?;
        let mut records = history_with(id, None, |before, max| self.fetch_history(id, before, max))?;
        self.resolve_images(id, &mut records);
        Ok(records)
    }

    fn get_messages_tail(&self, path: &str, n: usize) -> Result<Vec<Value>, String> {
        let id = Self::session_id_of(path).ok_or_else(|| format!("invalid dsh URI: {path}"))?;
        let mut records =
            history_with(id, Some(n), |before, max| self.fetch_history(id, before, max))?;
        self.resolve_images(id, &mut records);
        Ok(records)
    }

    fn watch_strategy(&self) -> WatchStrategy {
        WatchStrategy::Poll(POLL_INTERVAL)
    }

    fn owns_path(&self, path: &str) -> bool {
        path.starts_with(DSH_URI_PREFIX)
    }

    /// Start a brand-new dsh session and give it its first prompt.
    ///
    /// Three RPCs, no process: `session.create` with a Fleet-minted id (dsh
    /// honours a pre-assigned one, so the caller can correlate immediately
    /// instead of guessing which session appeared), an optional
    /// `session.selectModel`, then `session.prompt`.
    fn spawn(
        &self,
        spec: &crate::agent_source::SpawnSpec,
    ) -> Result<crate::session_launch::SpawnSessionResponse, String> {
        if spec.workspace_path.trim().is_empty() {
            return Err("dsh spawn: workspace_path is required".into());
        }
        let session_id = normalize_session_id(spec.session_id.as_deref());

        self.with_client(|client| {
            let mut payload = json!({ "cwd": spec.workspace_path, "sessionId": session_id });
            let asked_preset = resolve_chat_preset(&spec.workspace_path, |m, p| {
                client.call(m, p).map_err(String::from)
            });
            if let Some(preset) = &asked_preset {
                payload["agentPreset"] = json!(preset);
            }
            let created = client
                .call("session.create", payload)
                .map_err(String::from)?;
            // `session.create` echoes the preset it mounted. A mismatch means the
            // chat session silently kept the doctrine, which is the whole failure
            // the preset exists to prevent — so it is said out loud rather than
            // inferred later from a transcript. Not fatal: the session is healthy
            // and the chat brief still wins on precedence.
            if let Some(asked) = &asked_preset {
                let got = created.get("agentPreset").and_then(Value::as_str);
                if got != Some(asked.as_str()) {
                    log_chat_preset_skip(&format!(
                        "asked for preset {asked}, dsh mounted {}",
                        got.unwrap_or("<none>")
                    ));
                }
            }
            // dsh echoes the id it actually used. If it ever stopped honouring
            // the pre-assignment, silently returning our own id would hand the
            // caller a session that does not exist.
            let assigned = created
                .get("sessionId")
                .and_then(Value::as_str)
                .unwrap_or_default();
            if assigned != session_id {
                return Err(format!(
                    "dsh spawn: asked for session {session_id}, got {assigned}"
                ));
            }
            Self::select_model(client, &session_id, spec.model.as_deref(), spec.effort.as_deref())?;
            Self::prompt(client, &session_id, &spec.prompt)
        })?;

        // Mark it as Fleet's, the way every other spawn path does. Without this
        // the session is created and healthy but never enters the Tasks list
        // (`isFleetOwnedTask` needs both `fleet_spawned` and a known
        // `entrypoint`), so the "新建会话" dialog waits for an id that can never
        // arrive and spins forever. dsh has no per-session process to stamp an
        // entrypoint on — its turns run inside the shared server — so the note
        // carries the launcher surface too.
        let entrypoint = Some(spec.entrypoint.trim())
            .filter(|e| !e.is_empty())
            .unwrap_or(crate::session_launch::NEW_SESSION_ENTRYPOINT);
        crate::launch_spec::record_with_entrypoint(
            &session_id,
            spec.model.as_deref(),
            spec.effort.as_deref(),
            Some(entrypoint),
        );

        Ok(crate::session_launch::SpawnSessionResponse {
            // Every dsh session shares the server's pid — there is no per-session
            // process. See `DshServer::pid`.
            pid: Self::server_pid(),
            session_id: Some(session_id),
        })
    }

    /// Continue an existing dsh session.
    ///
    /// dsh has no separate resume method and needs none: `session.selectModel`
    /// loads a session this server has never seen (verified against one written
    /// by a different `dsh web` process) and `session.prompt` appends to it, so
    /// the history is already back in the model's context.
    fn resume(
        &self,
        spec: &crate::agent_source::ResumeSpec,
        on_exit: Box<dyn FnOnce(bool) + Send>,
    ) -> Result<(), String> {
        let session_id = Self::session_id_of(&spec.session_id)
            .unwrap_or(&spec.session_id)
            .to_string();
        if session_id.is_empty() {
            let _ = on_exit;
            return Err("dsh resume: session_id is required".into());
        }
        let prompt = if spec.prompt.trim().is_empty() {
            "continue"
        } else {
            &spec.prompt
        };

        // Make sure the server and its follower are up before arming: the
        // callback registry lives on the watcher.
        self.with_client(|_| Ok(()))?;
        let armed = Self::arm_turn_end(&session_id, on_exit);

        let started = self.with_client(|client| {
            Self::select_model(client, &session_id, spec.model.as_deref(), spec.effort.as_deref())?;
            Self::prompt(client, &session_id, prompt)
        });

        if let Err(e) = started {
            // The turn never began, so no `turn/end` is coming. Reclaim the
            // callback and report the failure ourselves.
            if let Some(cb) = lock(&armed).take() {
                cb(false);
            }
            return Err(e);
        }
        Ok(())
    }

    /// Cut the session's current turn short.
    ///
    /// This is the whole reason [`AgentSource::interrupt_session`] exists: every
    /// dsh session shares one server process, so the pid-based stop path would
    /// signal that server and take down every dsh session on the machine
    /// (Fleet's included). `session.cancel` is the per-session lever — measured
    /// live, it ends the turn with `turn/end.data.reason = {"kind":"aborted",
    /// "reason":{"kind":"user"}}` against `{"kind":"completed"}` for a turn that
    /// finished on its own, which is also what makes
    /// [`crate::dsh_events::LiveView`] report the outcome as a failure rather
    /// than a success.
    fn interrupt_session(&self, session_id: &str) -> Result<(), String> {
        let id = Self::session_id_of(session_id).unwrap_or(session_id);
        if id.is_empty() {
            return Err("dsh interrupt: session_id is required".into());
        }
        self.with_client(|client| {
            client
                .call("session.cancel", json!({ "sessionId": id }))
                .map(|_| ())
                .map_err(Into::into)
        })
    }

    /// dsh sessions have no Fleet-readable file: the transcript is a
    /// zstd-framed log the server owns. Nothing may hand out a path for it.
    fn resolve_file_path(&self, _path: &str) -> Option<PathBuf> {
        None
    }
}

/// dsh-native token breakdown for one session, shown in the desktop "Token" tab.
///
/// The other two sources parse a transcript file; dsh has none (see
/// [`AgentSource::resolve_file_path`] above), so this is assembled from the
/// projections dsh itself folds and hands over on `session.list`. Two distinct
/// things live here and must not be added together:
///
/// - **Billed, cumulative** (`tokenUsage`): the four buckets dsh meters over the
///   whole session. These sum to [`Self::total_tokens`].
/// - **Context, current** (`contextPressure` / `contextBreakdown`): how full the
///   window is *right now* and what is occupying it. A single re-read of a large
///   cached prefix adds to the cumulative buckets without moving these at all.
///
/// No cost figure: dsh routes through OpenRouter to an open model space, and
/// Fleet's price table only knows Claude and GPT tiers, so an unknown model
/// would silently price at the Opus fallback. A wrong number is worse than none.
/// No model id either: the roster carries none, and the durable log's route
/// evidence is reached by reading history — which [`latest_route`] already
/// harvests on every read, so `SessionInfo::model` is where the route shows up.
#[derive(serde::Serialize, serde::Deserialize, Clone, Debug, Default, PartialEq)]
#[cfg_attr(feature = "ts-export", derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub struct DshTokenBreakdown {
    /// Input tokens that missed the cache and were billed at full price.
    pub uncached_input_tokens: u64,
    /// Input tokens served from the prompt cache, billed at the cache-read rate.
    pub cache_read_tokens: u64,
    /// Input tokens written into the prompt cache, billed at the cache-write rate.
    pub cache_write_tokens: u64,
    /// Output tokens.
    pub output_tokens: u64,
    /// The four buckets above, summed — the panel's total.
    pub total_tokens: u64,
    /// System prompt currently occupying the window.
    pub system_tokens: u64,
    /// Tool definitions currently occupying the window.
    pub tools_tokens: u64,
    /// Conversation messages currently occupying the window.
    pub message_tokens: u64,
    /// What the next request is projected to send, per dsh's own estimate.
    /// `None` until the session has run a turn (dsh reports `contextPressure`
    /// as an empty object on a blank session).
    pub projected_tokens: Option<u64>,
    /// The model's context window, when dsh knows it.
    pub context_window: Option<u64>,
    /// `projected_tokens / context_window`, `None` when either is missing.
    pub context_percent: Option<f64>,
}

/// Build a [`DshTokenBreakdown`] from one `session.list` item's projections.
///
/// Split out from the RPC so the mapping is unit-testable against a recorded
/// payload. A projections block missing a key yields zeros rather than an error:
/// a session that has not run a turn genuinely has no usage, and the panel
/// should show that rather than an error string.
fn dsh_token_breakdown_from_projections(projections: &Value) -> DshTokenBreakdown {
    let uncached_input_tokens = projection_u64(projections, "tokenUsage", "uncachedInputTokens");
    let cache_read_tokens = projection_u64(projections, "tokenUsage", "cacheReadTokens");
    let cache_write_tokens = projection_u64(projections, "tokenUsage", "cacheWriteTokens");
    let output_tokens = projection_u64(projections, "tokenUsage", "outputTokens");

    let opt = |key: &str, field: &str| -> Option<u64> {
        projections
            .get("values")
            .and_then(|v| v.get(key))
            .and_then(|v| v.get(field))
            .and_then(Value::as_u64)
    };
    let projected_tokens = opt("contextPressure", "projectedTokens");
    let context_window = opt("contextPressure", "contextWindow");
    let context_percent = match (projected_tokens, context_window) {
        (Some(used), Some(window)) if window > 0 => Some(used as f64 / window as f64),
        _ => None,
    };

    DshTokenBreakdown {
        uncached_input_tokens,
        cache_read_tokens,
        cache_write_tokens,
        output_tokens,
        total_tokens: uncached_input_tokens
            + cache_read_tokens
            + cache_write_tokens
            + output_tokens,
        system_tokens: projection_u64(projections, "contextBreakdown", "systemTokens"),
        tools_tokens: projection_u64(projections, "contextBreakdown", "toolsTokens"),
        message_tokens: projection_u64(projections, "contextBreakdown", "messageTokens"),
        projected_tokens,
        context_window,
        context_percent,
    }
}

/// Token breakdown for a `dsh://` session URI.
///
/// Reads the projections off `session.list` — the same call [`DshSource::scan_sessions`]
/// already makes — rather than `session.history`: both carry the same
/// projections block, but history additionally ships every event of the session
/// just to be thrown away here. Errors when the id is unknown to the server, so
/// a stale URI reports that instead of rendering a plausible all-zero panel.
pub fn dsh_token_breakdown(uri: &str) -> Result<DshTokenBreakdown, String> {
    let id = DshSource::session_id_of(uri).ok_or_else(|| format!("invalid dsh URI: {uri}"))?;
    let value = DshSource::new()
        .with_client(|client| client.call("session.list", json!({})).map_err(Into::into))?;

    let item = value
        .get("items")
        .and_then(Value::as_array)
        .and_then(|items| {
            items
                .iter()
                .find(|it| it.get("sessionId").and_then(Value::as_str) == Some(id))
        })
        .ok_or_else(|| format!("dsh session not found: {id}"))?;

    let projections = item
        .get("projections")
        .cloned()
        .unwrap_or_else(|| json!({}));
    Ok(dsh_token_breakdown_from_projections(&projections))
}

/// The **raw**, un-normalised durable events of a `dsh://` session.
///
/// [`AgentSource::get_messages`] returns the same history translated into Claude
/// Code's message vocabulary, which is what every Fleet client reads — but that
/// translation drops the typed `source` metadata dsh hangs off each record.
/// [`crate::dsh_cost`] needs exactly that metadata (the provider's generation
/// id), so it reads the events themselves.
pub fn session_events(uri: &str) -> Result<Vec<Value>, String> {
    let id = DshSource::session_id_of(uri).ok_or_else(|| format!("invalid dsh URI: {uri}"))?;
    let value = DshSource::new().with_client(|client| {
        client
            .call("session.history", json!({ "sessionId": id }))
            .map_err(Into::into)
    })?;
    Ok(history_events(&value))
}

/// Pull the raw `SessionEvent`s out of a `session.history` answer.
///
/// Each entry pairs the durable event with an optional host-computed tool view;
/// only the event is durable, so that is what Fleet keeps.
/// How many trailing append-origin messages a live-tail refresh asks for.
///
/// The unit is dsh's, not Fleet's: one `maxMessages` is a whole append-origin
/// message *and every raw event it owns*, streaming chunks included. Measured on
/// a 130-step session, `maxMessages: 150` (what `get_messages_tail(…, 150)` used
/// to send verbatim) answered 13.3 MB / 63,534 events — of which 62,853 were
/// `assistant/chunk` that [`crate::dsh_messages::normalize`] drops on the floor.
/// A refresh only has to cover what changed since the last poll, so it asks for
/// a handful of trailing messages and [`history_with`] falls back to a rebuild
/// when that window turns out not to reach back far enough.
const TAIL_REFRESH_MESSAGES: usize = 4;

/// How many append-origin messages one backfill page asks for.
///
/// Backfill is the `beforeSeq` walk that reaches back until the caller's tail
/// length is covered. Bigger pages mean fewer round trips but more wire events
/// per trip; 60 covers a typical 150-record detail view in one or two pages.
const BACKFILL_MESSAGES: usize = 60;

/// How many sessions keep a normalized history in memory.
///
/// A cached transcript is the *normalized* form (463 KB for the 130-step session
/// above, against the 13.3 MB of wire events it came from), and only sessions
/// someone is actually reading get one.
const HISTORY_CACHE_CAP: usize = 8;

/// One session's normalized transcript and the `seq` span it covers.
///
/// dsh stamps every durable event with a monotonically increasing `seq`
/// (verified across a 63,534-event history: no gaps, no event without one), so
/// the span is what makes both folds — newer events onto the end, older pages
/// onto the front — idempotent.
#[derive(Default)]
struct CachedHistory {
    /// Lowest `seq` folded in, or `None` while the cache is empty.
    first_seq: Option<i64>,
    /// Highest `seq` folded in, or `None` while the cache is empty.
    last_seq: Option<i64>,
    /// True once a page came back with `hasMore: false` — the walk has reached
    /// the start of the session and no earlier page exists.
    reached_start: bool,
    messages: Vec<Value>,
    touched: Option<std::time::Instant>,
}

static HISTORY: OnceLock<Mutex<std::collections::HashMap<String, CachedHistory>>> = OnceLock::new();

fn history_slot() -> &'static Mutex<std::collections::HashMap<String, CachedHistory>> {
    HISTORY.get_or_init(|| Mutex::new(std::collections::HashMap::new()))
}

/// Drop this session's cached transcript, so the next read rebuilds it.
pub fn forget_history(session_id: &str) {
    lock(history_slot()).remove(session_id);
}

/// What one page of durable events says about how a session is configured:
/// its route, and the reasoning effort that route runs at.
///
/// The two resolve independently because they live in different places (see
/// [`latest_route`] and [`latest_effort`]), each carrying the `seq` of the
/// event that supplied it so a later backfill page of *older* events cannot
/// overwrite a newer reading.
#[derive(Debug, Default, PartialEq)]
struct Seen {
    route: Option<(i64, String)>,
    effort: Option<(i64, String)>,
}

/// Read both readings off one page.
fn seen_in(events: &[Value]) -> Seen {
    Seen {
        route: latest_route(events),
        effort: latest_effort(events),
    }
}

/// The reasoning effort one page records, latest wins.
///
/// Unlike the route, this rides **only** `request/header`
/// (`data.header.config.reasoningEffort`) — measured against a live server: a
/// long session's tail page held 7,877 events and zero headers, while a short
/// session's whole log carried one with `reasoningEffort: "high"`. dsh appends a
/// header at `initial` / `resume` / `change`, so the value shows up for short
/// sessions, for resumed ones, and for anyone who reads a transcript back to
/// its head — and stays unknown otherwise. `session.models` would answer
/// authoritatively for every session, but it **attaches an agent** to the
/// session it is asked about (measured: `host.describe.attachedSessions` 2 → 3)
/// and dsh publishes no detach call, so paying that for a chip is not on.
///
/// Absent is a legitimate answer, not a failure: a model with no reasoning
/// ladder (measured: `openrouter/anthropic/claude-haiku-4.5`) carries no
/// `reasoningEffort` at all.
fn latest_effort(events: &[Value]) -> Option<(i64, String)> {
    let mut best: Option<(i64, String)> = None;
    for event in events {
        if event.get("type").and_then(Value::as_str) != Some("request/header") {
            continue;
        }
        let effort = event
            .pointer("/data/header/config/reasoningEffort")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|e| !e.is_empty());
        if let Some(effort) = effort {
            let seq = seq_of(event).unwrap_or(i64::MIN);
            if best.as_ref().is_none_or(|(at, _)| seq >= *at) {
                best = Some((seq, effort.to_string()));
            }
        }
    }
    best
}

/// The `provider/model` route one page of durable events records, latest wins.
///
/// Three event types carry the route, all verbatim off a live `session.history`:
///
/// * `assistant/message` — `data.message.source` is
///   `{kind:"model", provider, model}` on every assembled model reply. This is
///   the one that exists per *step*, so it tracks a mid-session model switch.
/// * `request/context` — `{provider, model, contextWindow}`, appended only when
///   the route or the capacity changes.
/// * `request/header` — `data.header.config` carries `{provider, model,
///   reasoningEffort, …}`. Near the head of the log, so it is the only evidence
///   a session that has not finished a step yet can offer.
///
/// The composed string is deliberately dsh's *spec* form — the same
/// `provider/model` [`split_model`] parses back and [`SpawnSpec::model`]
/// carries — so what the UI shows is exactly what would relaunch the session on
/// the same route.
///
/// Not to be confused with [`crate::dsh_cost::metered_calls`], which walks the
/// same `assistant/message.source` for a different question: every *priceable*
/// call and its tokens, rather than the one route the session is on now.
///
/// Reasoning effort rides only `request/header`, so it resolves separately —
/// see [`Seen`].
///
/// [`SpawnSpec::model`]: crate::agent_source::SpawnSpec
fn latest_route(events: &[Value]) -> Option<(i64, String)> {
    /// The route one event names, if it is one of the three that carry one.
    fn route_of(event: &Value) -> Option<String> {
        fn spec(provider: Option<&Value>, model: Option<&Value>) -> Option<String> {
            let provider = provider?.as_str()?.trim();
            let model = model?.as_str()?.trim();
            (!provider.is_empty() && !model.is_empty()).then(|| format!("{provider}/{model}"))
        }
        let data = event.get("data")?;
        match event.get("type").and_then(Value::as_str)? {
            "assistant/message" => {
                let source = data
                    .get("message")
                    .and_then(|m| m.get("source"))
                    .filter(|s| s.get("kind").and_then(Value::as_str) == Some("model"))?;
                spec(source.get("provider"), source.get("model"))
            }
            "request/context" => spec(data.get("provider"), data.get("model")),
            "request/header" => {
                let config = data.get("header").and_then(|h| h.get("config"))?;
                spec(config.get("provider"), config.get("model"))
            }
            _ => None,
        }
    }

    // Ordered by `seq` rather than by position: `merge_page` hands over whole
    // pages, and a backfill page walking `beforeSeq` arrives newest-first.
    let mut best: Option<(i64, String)> = None;
    for event in events {
        if let Some(found) = route_of(event) {
            // `seq` is contiguous and monotonic across a session's whole log
            // (verified on a 63,534-event history), so it totally orders the
            // evidence. An event without one loses to anything that has one.
            let seq = seq_of(event).unwrap_or(i64::MIN);
            if best.as_ref().is_none_or(|(at, _)| seq >= *at) {
                best = Some((seq, found));
            }
        }
    }
    best
}

/// Every session's newest known [`Seen`], keyed by session id.
///
/// Separate from [`history_slot`] on purpose: a transcript is hundreds of KB
/// and gets evicted at [`HISTORY_CACHE_CAP`], while this is two short strings
/// per session and there is no reason to make the model chip flicker off just
/// because someone opened a ninth session. Bounded by the number of dsh
/// sessions read in one process lifetime — a few dozen strings.
static MODELS: OnceLock<Mutex<std::collections::HashMap<String, Seen>>> = OnceLock::new();

fn models_slot() -> &'static Mutex<std::collections::HashMap<String, Seen>> {
    MODELS.get_or_init(|| Mutex::new(std::collections::HashMap::new()))
}

/// Fold what a page showed into what is already known, for a later scan to read.
///
/// Older evidence loses, per field: history pages arrive in both directions (a
/// tail refresh, then `beforeSeq` backfill walking away from the present), so
/// last-write-wins would let a backfill page re-pin a session to the model it
/// started on. Per *field* because the two readings come from different events —
/// a page that names a route but no effort must not erase a known effort.
fn remember_seen(session_id: &str, seen: Seen) {
    fn newer(prev: &mut Option<(i64, String)>, found: Option<(i64, String)>) {
        let Some((seq, value)) = found else { return };
        match prev {
            Some((at, _)) if *at > seq => {}
            _ => *prev = Some((seq, value)),
        }
    }
    let mut map = lock(models_slot());
    let entry = map.entry(session_id.to_string()).or_default();
    newer(&mut entry.route, seen.route);
    newer(&mut entry.effort, seen.effort);
}

/// The route last seen for `session_id`, or `None` when Fleet has never read
/// this session's history in this process.
fn known_model(session_id: &str) -> Option<String> {
    lock(models_slot())
        .get(session_id)?
        .route
        .as_ref()
        .map(|(_, spec)| spec.clone())
}

/// The reasoning effort last seen for `session_id`. `None` both when nothing
/// has been read and when the log has no header in the pages that were.
fn known_effort(session_id: &str) -> Option<String> {
    lock(models_slot())
        .get(session_id)?
        .effort
        .as_ref()
        .map(|(_, effort)| effort.clone())
}

fn seq_of(event: &Value) -> Option<i64> {
    event.get("seq").and_then(Value::as_i64)
}

/// The highest `seq` in a page, if any event carries one.
fn max_seq(events: &[Value]) -> Option<i64> {
    events.iter().filter_map(seq_of).max()
}

/// The lowest `seq` in a page, if any event carries one.
fn min_seq(events: &[Value]) -> Option<i64> {
    events.iter().filter_map(seq_of).min()
}

/// Does this tail page reach back far enough to append onto a cache that ends at
/// `last_seq`?
///
/// It does when the page either overlaps the cache (some event at or below
/// `last_seq`) or starts exactly where the cache stopped. Anything else means
/// more messages landed between two polls than the refresh window covers, and
/// appending would silently skip the events in between — so the caller drops the
/// cache and reads again.
fn page_continues(last_seq: i64, page: &[Value]) -> bool {
    match min_seq(page) {
        Some(min) => min <= last_seq + 1,
        // A page with no sequenced event adds nothing; treating it as continuous
        // keeps the cache as it is rather than triggering a pointless refetch.
        None => true,
    }
}

/// The events of `page` a cache ending at `last_seq` has not folded in yet.
fn events_after(last_seq: i64, page: &[Value]) -> Vec<Value> {
    page.iter()
        .filter(|e| seq_of(e).is_some_and(|s| s > last_seq))
        .cloned()
        .collect()
}

/// The events of `page` older than a cache starting at `first_seq`.
fn events_before(first_seq: i64, page: &[Value]) -> Vec<Value> {
    page.iter()
        .filter(|e| seq_of(e).is_some_and(|s| s < first_seq))
        .cloned()
        .collect()
}

/// Fold one page into the cache, in both directions, and return the entry's
/// state afterwards as `(records, first_seq, reached_start)`.
///
/// Both folds key off the cached `seq` span rather than off what the caller
/// believed it was, which is what makes concurrent readers safe: whoever gets
/// the lock second sees the first one's events already in and skips them.
fn merge_page(session_id: &str, events: &[Value], has_more: bool) -> (usize, Option<i64>, bool) {
    // Every history read is also the one chance to learn how the session is
    // configured: the roster names neither route nor effort, and asking per
    // session would be a `session.list` sized cost on every scan. Recorded
    // before the fold so a page that turns out to be entirely already-cached
    // still teaches the scan something.
    remember_seen(session_id, seen_in(events));

    let mut map = lock(history_slot());
    evict_cold(&mut map, session_id);
    let entry = map.entry(session_id.to_string()).or_default();

    match (entry.first_seq, entry.last_seq) {
        (Some(first), Some(last)) => {
            let older = events_before(first, events);
            if !older.is_empty() {
                let mut folded = crate::dsh_messages::normalize(&older);
                folded.append(&mut entry.messages);
                entry.messages = folded;
                entry.first_seq = min_seq(&older).or(Some(first));
            }
            let newer = events_after(last, events);
            if !newer.is_empty() {
                entry
                    .messages
                    .extend(crate::dsh_messages::normalize(&newer));
                entry.last_seq = max_seq(&newer).or(Some(last));
            }
        }
        // Empty cache: the page is the whole of what is known so far.
        _ => {
            entry.messages = crate::dsh_messages::normalize(events);
            entry.first_seq = min_seq(events);
            entry.last_seq = max_seq(events);
        }
    }

    // `hasMore: false` means this page reached the start of the session — there
    // is no earlier page to walk back to. Only a page that actually extended the
    // front can settle that, so a tail refresh (which never does) leaves it be.
    if !has_more && entry.first_seq.is_some_and(|f| min_seq(events) == Some(f)) {
        entry.reached_start = true;
    }
    entry.touched = Some(std::time::Instant::now());
    (entry.messages.len(), entry.first_seq, entry.reached_start)
}

/// The cached `(records, last_seq, first_seq, reached_start)` for one session.
fn cache_state(session_id: &str) -> (usize, Option<i64>, Option<i64>, bool) {
    let mut map = lock(history_slot());
    match map.get_mut(session_id) {
        Some(entry) => {
            entry.touched = Some(std::time::Instant::now());
            (
                entry.messages.len(),
                entry.last_seq,
                entry.first_seq,
                entry.reached_start,
            )
        }
        None => (0, None, None, false),
    }
}

/// The tail of a cached transcript: the last `n` records, or all of them.
fn cached_tail(session_id: &str, n: Option<usize>) -> Vec<Value> {
    let map = lock(history_slot());
    let Some(entry) = map.get(session_id) else {
        return Vec::new();
    };
    match n {
        Some(n) if entry.messages.len() > n => entry.messages[entry.messages.len() - n..].to_vec(),
        _ => entry.messages.clone(),
    }
}

/// Read a session's transcript, folding a small tail refresh onto what is
/// already cached instead of re-reading the whole history every poll.
///
/// `fetch(before_seq, max_messages)` returns the raw `session.history` answer for
/// one page: `before_seq` walks backwards (`None` = the tail page), `max_messages`
/// is dsh's own unit. It is a parameter so the paging contract is testable
/// without a live server.
///
/// `n` is Fleet's tail length — a count of *normalized* records, which is not
/// dsh's `maxMessages` unit (see [`TAIL_REFRESH_MESSAGES`]) — or `None` for the
/// whole transcript.
fn history_with<F>(session_id: &str, n: Option<usize>, fetch: F) -> Result<Vec<Value>, String>
where
    F: Fn(Option<i64>, Option<usize>) -> Result<Value, String>,
{
    // Held for the whole read: the cache entry this walk is building must not be
    // evicted by another session's read while pages are still landing.
    let _reading = ReadGuard::enter(session_id);

    let (_, last_seq, _, _) = cache_state(session_id);

    // 1. The tail: a small refresh onto a warm cache, a first page onto a cold
    //    one. A refresh that does not reach back to where the cache stopped
    //    would skip the messages in between, so that cache is dropped instead.
    let want = match last_seq {
        Some(_) => TAIL_REFRESH_MESSAGES,
        None => BACKFILL_MESSAGES,
    };
    let page = fetch(None, Some(want))?;
    let events = history_events(&page);
    if let Some(last) = last_seq {
        if !page_continues(last, &events) {
            forget_history(session_id);
        }
    }
    let (mut records, mut first_seq, mut reached_start) =
        merge_page(session_id, &events, has_more(&page));

    // 2. Backfill: walk `beforeSeq` back until the caller's window is covered.
    //    A bare tail page is NOT the whole history — measured on a 63,534-event
    //    session it answered `hasMore: true` with only the last 26,510 — so this
    //    walk is what makes `get_messages` (n = None) actually complete.
    while !reached_start && needs_more(records, n) {
        let Some(before) = first_seq else { break };
        let page = fetch(Some(before), Some(BACKFILL_MESSAGES))?;
        let events = history_events(&page);
        if events.is_empty() {
            break;
        }
        let (r, f, done) = merge_page(session_id, &events, has_more(&page));
        // No page extended the front: nothing older is reachable, and looping
        // would spin on the same request forever.
        if f == first_seq {
            break;
        }
        (records, first_seq, reached_start) = (r, f, done);
    }

    Ok(cached_tail(session_id, n))
}

/// `hasMore` is dsh's "an earlier page exists" flag; a malformed answer is
/// treated as "more", which only costs one extra walk.
fn has_more(page: &Value) -> bool {
    page.get("hasMore").and_then(Value::as_bool).unwrap_or(true)
}

fn needs_more(records: usize, n: Option<usize>) -> bool {
    match n {
        Some(n) => records < n,
        None => true,
    }
}

/// Sessions with a read in flight, by id. Evicting one of these mid-backfill
/// would strand the walk: the next page would land in a fresh entry and be taken
/// for the whole transcript, so the reader would hand back a middle slice of the
/// history as if it were the tail.
static HISTORY_INFLIGHT: OnceLock<Mutex<std::collections::HashMap<String, usize>>> =
    OnceLock::new();

fn inflight_slot() -> &'static Mutex<std::collections::HashMap<String, usize>> {
    HISTORY_INFLIGHT.get_or_init(|| Mutex::new(std::collections::HashMap::new()))
}

/// Marks one session as being read for as long as it lives.
struct ReadGuard(String);

impl ReadGuard {
    fn enter(session_id: &str) -> Self {
        *lock(inflight_slot())
            .entry(session_id.to_string())
            .or_insert(0) += 1;
        Self(session_id.to_string())
    }
}

impl Drop for ReadGuard {
    fn drop(&mut self) {
        let mut map = lock(inflight_slot());
        if let Some(n) = map.get_mut(&self.0) {
            *n = n.saturating_sub(1);
            if *n == 0 {
                map.remove(&self.0);
            }
        }
    }
}

fn is_being_read(session_id: &str) -> bool {
    lock(inflight_slot()).contains_key(session_id)
}

/// Keep the cache at [`HISTORY_CACHE_CAP`] by dropping the least recently read
/// session — one nobody has open. The session being read, and any other with a
/// read in flight, are never candidates.
fn evict_cold(map: &mut std::collections::HashMap<String, CachedHistory>, keep: &str) {
    while map.len() >= HISTORY_CACHE_CAP {
        let Some(coldest) = map
            .iter()
            .filter(|(id, _)| id.as_str() != keep && !is_being_read(id))
            .min_by_key(|(_, c)| c.touched)
            .map(|(id, _)| id.clone())
        else {
            return;
        };
        map.remove(&coldest);
    }
}

fn history_events(value: &Value) -> Vec<Value> {
    value
        .get("events")
        .and_then(Value::as_array)
        .map(|entries| {
            entries
                .iter()
                .filter_map(|entry| entry.get("event").cloned())
                .collect()
        })
        .unwrap_or_default()
}

/// One reasoning-effort step a model accepts, as the catalogue names it.
#[derive(serde::Serialize, serde::Deserialize, Clone, Debug, Default, PartialEq)]
#[cfg_attr(feature = "ts-export", derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub struct DshEffort {
    /// The value to send as `reasoningEffort` (`"off"`, `"low"`, `"max"`, …).
    pub id: String,
    /// Display label (`"Off"`, `"Low"`, `"Max"`, …).
    pub name: String,
}

/// One selectable model inside a provider group.
#[derive(serde::Serialize, serde::Deserialize, Clone, Debug, Default, PartialEq)]
#[cfg_attr(feature = "ts-export", derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub struct DshModelEntry {
    /// The provider-scoped model id, exactly as dsh reports it. Not unique
    /// across groups and **not** what Fleet stores — see [`Self::spec`].
    pub id: String,
    /// Display label.
    pub name: String,
    /// Optional one-liner from the provider.
    pub description: Option<String>,
    /// The `provider/model` string to put in [`SpawnSpec::model`], i.e. what
    /// [`split_model`] parses back into dsh's two fields. Composed here rather
    /// than in the UI so the addressing convention lives next to its parser.
    ///
    /// [`SpawnSpec::model`]: crate::agent_source::SpawnSpec
    pub spec: String,
    /// The effort scale this model accepts, flattened out of the wire's
    /// optional `reasoning` object. **Empty means the model has no reasoning
    /// control at all** (most openrouter entries), which is a real state the UI
    /// must render as "no effort menu" — not as "the menu failed to load".
    pub efforts: Vec<DshEffort>,
    /// The effort dsh picks when the caller names none. `None` even for models
    /// that *do* have an effort scale (openrouter reports scales without a
    /// default), so the UI cannot assume one exists.
    pub default_effort: Option<String>,
}

/// One provider's slice of the catalogue.
#[derive(serde::Serialize, serde::Deserialize, Clone, Debug, Default, PartialEq)]
#[cfg_attr(feature = "ts-export", derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub struct DshModelGroup {
    /// The provider id — the first segment of every member's
    /// [`DshModelEntry::spec`] (`"deepseek-official"`, `"openrouter"`).
    pub id: String,
    /// Display label.
    pub name: String,
    pub models: Vec<DshModelEntry>,
}

/// A provider that could not be enumerated at all.
#[derive(serde::Serialize, serde::Deserialize, Clone, Debug, Default, PartialEq)]
#[cfg_attr(feature = "ts-export", derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub struct DshModelCatalogFailure {
    pub id: String,
    pub name: String,
    pub message: String,
}

/// dsh's session-independent model catalogue: what the launcher can offer.
///
/// Partial success is the normal case, not an error: `llm.models` answers `ok`
/// with the providers it *could* reach in `groups` and the ones it could not in
/// `failures`. A caller that treated a non-empty `failures` as a hard error
/// would blank a working DeepSeek list because some third-party route was
/// misconfigured, so both halves come back and the UI decides.
#[derive(serde::Serialize, serde::Deserialize, Clone, Debug, Default, PartialEq)]
#[cfg_attr(feature = "ts-export", derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub struct DshModelCatalog {
    pub groups: Vec<DshModelGroup>,
    pub failures: Vec<DshModelCatalogFailure>,
}

/// Map an `llm.models` answer onto [`DshModelCatalog`].
///
/// Split from [`dsh_models`] so the mapping is unit-testable against a recorded
/// payload. Tolerant by construction — a group or model missing `id` is dropped
/// rather than failing the whole catalogue, because a single unparseable route
/// should not cost the user every other model on the machine.
fn parse_model_catalog(value: &Value) -> DshModelCatalog {
    /// A non-empty string field, or `None`.
    fn text(v: &Value, key: &str) -> Option<String> {
        v.get(key)
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string)
    }
    fn rows(v: &Value, key: &str) -> Vec<Value> {
        v.get(key)
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default()
    }

    let groups = rows(value, "groups")
        .iter()
        .filter_map(|g| {
            let id = text(g, "id")?;
            let models = rows(g, "models")
                .iter()
                .filter_map(|m| {
                    let model_id = text(m, "id")?;
                    // dsh's `reasoning` is optional, and so is the `defaultEffort`
                    // inside it — flattened here so the UI reads one shape.
                    let reasoning = m.get("reasoning");
                    let efforts = reasoning
                        .map(|r| rows(r, "efforts"))
                        .unwrap_or_default()
                        .iter()
                        .filter_map(|e| {
                            let id = text(e, "id")?;
                            let name = text(e, "name").unwrap_or_else(|| id.clone());
                            Some(DshEffort { id, name })
                        })
                        .collect();
                    Some(DshModelEntry {
                        spec: format!("{id}/{model_id}"),
                        name: text(m, "name").unwrap_or_else(|| model_id.clone()),
                        description: text(m, "description"),
                        id: model_id,
                        efforts,
                        default_effort: reasoning.and_then(|r| text(r, "defaultEffort")),
                    })
                })
                .collect();
            Some(DshModelGroup {
                name: text(g, "name").unwrap_or_else(|| id.clone()),
                id,
                models,
            })
        })
        .collect();

    let failures = rows(value, "failures")
        .iter()
        .filter_map(|f| {
            let id = text(f, "id")?;
            Some(DshModelCatalogFailure {
                name: text(f, "name").unwrap_or_else(|| id.clone()),
                id,
                message: text(f, "message").unwrap_or_default(),
            })
        })
        .collect();

    DshModelCatalog { groups, failures }
}

/// dsh's model catalogue, for the launcher's model / effort menus.
///
/// Session-independent: `llm.models` takes an empty payload and describes the
/// machine's configured providers, so the answer is the same for every session
/// and needs no id. Reached through [`DshSource::with_client`] like every other
/// read, which starts `dsh web` if it is not already up.
pub fn dsh_models() -> Result<DshModelCatalog, String> {
    let value = DshSource::new()
        .with_client(|client| client.call("llm.models", json!({})).map_err(Into::into))?;
    Ok(parse_model_catalog(&value))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Verbatim shape of one `session.list` item observed live.
    fn live_list_item() -> Value {
        json!({
            "sessionId": "session-f8d1103c-db06-45c9-9902-6dc522f0f0ee",
            "updatedAt": 1786739335270u64,
            "running": false,
            "blank": false,
            "cwd": "/tmp",
            "agentPreset": "standard",
            "projections": {
                "asOfSeq": 15,
                "values": {
                    "title": "say hi",
                    "sessionStats": { "turns": 1, "steps": 1 },
                    "tokenUsage": {
                        "uncachedInputTokens": 100,
                        "outputTokens": 20,
                        "cacheReadTokens": 5,
                        "cacheWriteTokens": 2
                    },
                    "contextPressure": { "contextWindow": 1000000 }
                }
            }
        })
    }

    #[test]
    /// A dsh session Fleet spawned must read back as a Fleet-owned task, or the
    /// desktop's "新建会话" dialog spins forever.
    ///
    /// The 启动台 list is `sessions.filter(isFleetOwnedTask)` =
    /// `!isSubagent && isFleetOwnedEntrypoint(entrypoint) && fleetSpawned`, and
    /// the dialog waits for the id it just spawned to appear in *that* list
    /// before swapping the spinner for the session view. dsh sessions came back
    /// `entrypoint: None, fleet_spawned: false` (measured on the installed
    /// build), so the id could never arrive and the spinner never stopped —
    /// while `dsh web` was healthy and the session existed, which is what made
    /// it look like a hang rather than a mismatch.
    #[test]
    fn a_fleet_spawned_session_reads_back_as_a_fleet_owned_task() {
        let _lock = crate::session::fleet_home_lock();
        let tmp = tempfile::tempdir().unwrap();
        let prev = std::env::var_os("FLEET_HOME");
        std::env::set_var("FLEET_HOME", tmp.path());

        let item = live_list_item();
        let id = item.get("sessionId").and_then(Value::as_str).unwrap().to_string();

        // Not spawned by Fleet (a session the user started in dsh's own UI):
        // it belongs in the sessions list, never in the Tasks list.
        let foreign = session_info_from_list_item(&item).expect("mapped");
        assert!(!foreign.fleet_spawned, "an unrecorded session is not Fleet's");
        assert_eq!(foreign.entrypoint, None);

        // Spawned by Fleet: the spawn path records the marker, and the mapping
        // has to reflect it.
        crate::launch_spec::record_with_entrypoint(
            &id,
            None,
            None,
            Some(crate::session_launch::NEW_SESSION_ENTRYPOINT),
        );
        let owned = session_info_from_list_item(&item).expect("mapped");
        assert!(
            owned.fleet_spawned,
            "a recorded spawn must set fleet_spawned, or isFleetOwnedTask() is false"
        );
        assert_eq!(
            owned.entrypoint.as_deref(),
            Some(crate::session_launch::NEW_SESSION_ENTRYPOINT),
            "entrypoint must be one isFleetOwnedEntrypoint() accepts"
        );
        assert!(!owned.is_subagent, "a spawned dsh session is a main session");

        match prev {
            Some(v) => std::env::set_var("FLEET_HOME", v),
            None => std::env::remove_var("FLEET_HOME"),
        }
    }

    #[test]
    fn maps_a_live_list_item() {
        let info = session_info_from_list_item(&live_list_item()).expect("mapped");
        assert_eq!(info.id, "session-f8d1103c-db06-45c9-9902-6dc522f0f0ee");
        assert_eq!(info.workspace_path, "/tmp");
        assert_eq!(info.ai_title.as_deref(), Some("say hi"));
        assert_eq!(info.agent_source, "dsh");
        assert_eq!(
            info.jsonl_path,
            "dsh://session-f8d1103c-db06-45c9-9902-6dc522f0f0ee"
        );
        assert_eq!(info.last_activity_ms, 1786739335270);
    }

    #[test]
    fn sums_every_input_bucket_into_total_input() {
        let info = session_info_from_list_item(&live_list_item()).expect("mapped");
        // uncached 100 + cacheRead 5 + cacheWrite 2 — cache re-reads included,
        // matching what the other sources report as "tokens sent".
        assert_eq!(info.total_input_tokens, 107);
        assert_eq!(info.total_output_tokens, 20);
    }

    #[test]
    fn running_flag_maps_to_active() {
        let mut item = live_list_item();
        item["running"] = json!(true);
        let info = session_info_from_list_item(&item).expect("mapped");
        assert!(matches!(info.status, SessionStatus::Active));
    }

    #[test]
    fn tolerates_a_session_with_no_projections() {
        // A blank session that never ran carries no projections block.
        let item = json!({
            "sessionId": "session-blank",
            "updatedAt": 1u64,
            "running": false,
            "cwd": "/tmp"
        });
        let info = session_info_from_list_item(&item).expect("mapped");
        assert_eq!(info.ai_title, None);
        assert_eq!(info.total_input_tokens, 0);
    }

    #[test]
    fn rejects_an_item_without_a_session_id() {
        assert!(session_info_from_list_item(&json!({ "cwd": "/tmp" })).is_none());
    }

    // ── Which model a session runs on ────────────────────────────────────────
    //
    // `session.list` names no route (the `SessionSummary` contract carries
    // sessionId / updatedAt / running / blank / parentSessionId / origin / cwd /
    // agentPreset / projections and nothing else, and none of the projection
    // units publishes one either — read off `@deepseek-ai/dsh-host-apiproxy`
    // and confirmed against a live 75-session roster). The durable log does, in
    // three places; these fixtures are verbatim events off `session.history`.

    /// [`latest_route`] without its `seq`, which only `remember_model` needs.
    fn model_from_events(events: &[Value]) -> Option<String> {
        latest_route(events).map(|(_, spec)| spec)
    }

    fn header_event() -> Value {
        json!({
            "type": "request/header",
            "seq": 11,
            "time": 1787168057902i64,
            "data": {
                "header": {
                    "config": {
                        "provider": "deepseek-official",
                        "model": "deepseek-v4-flash",
                        "reasoningEffort": "high",
                        "maxTokens": 256000
                    },
                    "adapterDefaults": { "maxTokens": true },
                    "system": "…",
                    "tools": []
                },
                "reason": "initial"
            }
        })
    }

    fn context_event() -> Value {
        json!({
            "type": "request/context",
            "seq": 12,
            "time": 1787168057903i64,
            "data": {
                "provider": "deepseek-official",
                "model": "deepseek-v4-flash",
                "contextWindow": 1000000
            }
        })
    }

    fn reply_event(seq: i64, provider: &str, model: &str) -> Value {
        json!({
            "type": "assistant/message",
            "seq": seq,
            "time": 1787168059480i64,
            "data": {
                "turn": 1,
                "step": 1,
                "message": {
                    "role": "assistant",
                    "content": [{ "type": "text", "text": "嗨，老板！" }],
                    "source": { "kind": "model", "provider": provider, "model": model },
                    "id": "bbb00ec3"
                },
                "usage": { "inputTokens": 14184, "outputTokens": 66 }
            }
        })
    }

    #[test]
    fn reads_the_route_off_an_assistant_reply() {
        let events = vec![reply_event(86, "deepseek-official", "deepseek-v4-pro")];
        assert_eq!(
            model_from_events(&events).as_deref(),
            Some("deepseek-official/deepseek-v4-pro")
        );
    }

    #[test]
    fn a_session_that_has_not_replied_yet_still_names_its_route() {
        // The header lands before the first request goes out, so a session
        // caught mid-first-step has evidence while it has no reply.
        assert_eq!(
            model_from_events(&[header_event()]).as_deref(),
            Some("deepseek-official/deepseek-v4-flash")
        );
        assert_eq!(
            model_from_events(&[context_event()]).as_deref(),
            Some("deepseek-official/deepseek-v4-flash")
        );
    }

    #[test]
    fn the_latest_route_wins_when_the_model_changed_mid_session() {
        // dsh lets a session switch model between turns; the chip must name the
        // one it is on now, not the one it started on.
        let events = vec![
            header_event(),
            context_event(),
            reply_event(86, "deepseek-official", "deepseek-v4-flash"),
            reply_event(140, "openrouter", "anthropic/claude-opus-5"),
        ];
        assert_eq!(
            model_from_events(&events).as_deref(),
            Some("openrouter/anthropic/claude-opus-5"),
            "a page must resolve to its newest route"
        );
    }

    #[test]
    fn a_page_with_no_route_evidence_claims_none() {
        // Chunks, tool traffic and bookkeeping carry no route — and a guess
        // would be shown to the user as fact.
        let events = vec![json!({
            "type": "assistant/chunk",
            "seq": 20,
            "data": { "turn": 1, "step": 1, "chunk": { "type": "text-delta" } }
        })];
        assert_eq!(model_from_events(&events), None);
    }

    #[test]
    fn a_session_whose_history_was_read_reports_its_model() {
        // The roster names no route, so the mapping can only carry one once the
        // session's own events have been through Fleet — which is exactly what
        // opening it in the desktop does.
        let item = live_list_item();
        let id = item.get("sessionId").and_then(Value::as_str).unwrap();
        remember_seen(
            id,
            Seen {
                route: Some((86, "deepseek-official/deepseek-v4-pro".into())),
                effort: None,
            },
        );
        let info = session_info_from_list_item(&item).expect("mapped");
        assert_eq!(
            info.model.as_deref(),
            Some("deepseek-official/deepseek-v4-pro"),
            "the detail header's model chip reads SessionInfo::model"
        );
    }

    // ── Reasoning effort ─────────────────────────────────────────────────────

    #[test]
    fn reads_the_effort_off_a_request_header() {
        assert_eq!(
            latest_effort(&[header_event()]),
            Some((11, "high".to_string()))
        );
    }

    #[test]
    fn the_latest_header_wins_when_the_effort_changed() {
        // dsh appends a fresh header at `change`, so a session that switched
        // effort mid-run has two — the newer one is the one in force.
        let mut changed = header_event();
        changed["seq"] = json!(400);
        changed["data"]["header"]["config"]["reasoningEffort"] = json!("max");
        changed["data"]["reason"] = json!("change");
        assert_eq!(
            latest_effort(&[header_event(), changed.clone()]).map(|(_, e)| e),
            Some("max".to_string())
        );
        // …and the same in the order a `beforeSeq` backfill delivers them.
        assert_eq!(
            latest_effort(&[changed, header_event()]).map(|(_, e)| e),
            Some("max".to_string())
        );
    }

    #[test]
    fn a_model_with_no_reasoning_ladder_reports_no_effort() {
        // Measured: `openrouter/anthropic/claude-haiku-4.5` carries no
        // `reasoningEffort` at all. Absent must stay absent, not become "".
        let mut no_ladder = header_event();
        no_ladder["data"]["header"]["config"]
            .as_object_mut()
            .unwrap()
            .remove("reasoningEffort");
        assert_eq!(latest_effort(&[no_ladder]), None);
        // Nor does a reply or a context event carry one.
        assert_eq!(
            latest_effort(&[context_event(), reply_event(86, "p", "m")]),
            None
        );
    }

    #[test]
    fn folding_a_history_page_records_the_effort_too() {
        let id = "session-effort-fold";
        merge_page(id, &[header_event()], false);
        assert_eq!(
            known_effort(id).as_deref(),
            Some("high"),
            "the effort chip needs the same free ride as the model chip"
        );
        // A later page that names a route but no effort must not erase it.
        merge_page(id, &[reply_event(900, "deepseek-official", "deepseek-v4-pro")], false);
        assert_eq!(known_effort(id).as_deref(), Some("high"));
        forget_history(id);
    }

    #[test]
    fn the_effort_reaches_the_session_info() {
        let item = live_list_item();
        let id = item.get("sessionId").and_then(Value::as_str).unwrap();
        remember_seen(
            id,
            Seen {
                route: None,
                effort: Some((11, "high".into())),
            },
        );
        let info = session_info_from_list_item(&item).expect("mapped");
        assert_eq!(
            info.thinking_level.as_deref(),
            Some("high"),
            "the header's effort chip reads SessionInfo::thinking_level"
        );
    }

    /// For a session Fleet spawned with an explicit `--effort`, the launch spec
    /// already knows — no transcript read needed. It only fills in what the log
    /// has not shown: the header wins when both are known, because it is the
    /// effort a real request went out with.
    #[test]
    fn a_fleet_spawned_session_falls_back_to_its_launch_spec_effort() {
        let _lock = crate::session::fleet_home_lock();
        let tmp = tempfile::tempdir().unwrap();
        let prev = std::env::var_os("FLEET_HOME");
        std::env::set_var("FLEET_HOME", tmp.path());

        let mut item = live_list_item();
        let id = format!("session-effort-spec-{}", uuid::Uuid::new_v4());
        item["sessionId"] = json!(id);
        crate::launch_spec::record(&id, Some("deepseek-official/deepseek-v4-pro"), Some("max"));

        let info = session_info_from_list_item(&item).expect("mapped");
        assert_eq!(
            info.thinking_level.as_deref(),
            Some("max"),
            "a spawn Fleet recorded needs no transcript read to name its effort"
        );

        // The log's own header outranks the spawn record.
        remember_seen(
            &id,
            Seen {
                route: None,
                effort: Some((400, "high".into())),
            },
        );
        let info = session_info_from_list_item(&item).expect("mapped");
        assert_eq!(info.thinking_level.as_deref(), Some("high"));

        match prev {
            Some(v) => std::env::set_var("FLEET_HOME", v),
            None => std::env::remove_var("FLEET_HOME"),
        }
    }

    #[test]
    fn folding_a_history_page_records_its_route() {
        let id = "session-model-fold";
        merge_page(
            id,
            &[
                header_event(),
                reply_event(86, "deepseek-official", "deepseek-v4-pro"),
            ],
            false,
        );
        assert_eq!(
            known_model(id).as_deref(),
            Some("deepseek-official/deepseek-v4-pro"),
            "every history read must leave the route behind for the next scan"
        );
        forget_history(id);
    }

    /// Verbatim projections of a session that has run four turns, copied off a
    /// live `session.list` (server started with `dsh web --port 0`).
    fn live_projections() -> Value {
        json!({
            "asOfSeq": 175,
            "values": {
                "sessionStats": { "turns": 4, "steps": 8 },
                "title": "Fleet P4 Probe Echo Command",
                "tokenUsage": {
                    "uncachedInputTokens": 36,
                    "outputTokens": 509,
                    "cacheReadTokens": 60325,
                    "cacheWriteTokens": 10306
                },
                "contextPressure": {
                    "pressureTokens": 9178,
                    "projectedTokens": 9215,
                    "contextWindow": 200000
                },
                "contextBreakdown": {
                    "systemTokens": 1506,
                    "toolsTokens": 6376,
                    "messageTokens": 574
                }
            }
        })
    }

    #[test]
    fn token_breakdown_keeps_the_four_billed_buckets_separate() {
        let b = dsh_token_breakdown_from_projections(&live_projections());
        assert_eq!(b.uncached_input_tokens, 36);
        assert_eq!(b.cache_read_tokens, 60325);
        assert_eq!(b.cache_write_tokens, 10306);
        assert_eq!(b.output_tokens, 509);
        // The rows must sum to the total or the panel's percentages lie.
        assert_eq!(b.total_tokens, 36 + 60325 + 10306 + 509);
        assert_eq!(
            b.uncached_input_tokens + b.cache_read_tokens + b.cache_write_tokens + b.output_tokens,
            b.total_tokens
        );
    }

    #[test]
    fn token_breakdown_reports_context_separately_from_billing() {
        let b = dsh_token_breakdown_from_projections(&live_projections());
        // Context occupancy is a snapshot of the current window, not a running
        // total: 9215 projected against 200k, while 71k tokens were billed.
        assert_eq!(b.projected_tokens, Some(9215));
        assert_eq!(b.context_window, Some(200000));
        let pct = b.context_percent.expect("percent");
        assert!((pct - 9215.0 / 200000.0).abs() < 1e-9, "got {pct}");
        assert!(pct < 0.05, "must not be confused with the 71k billed");
        assert_eq!(b.system_tokens, 1506);
        assert_eq!(b.tools_tokens, 6376);
        assert_eq!(b.message_tokens, 574);
    }

    #[test]
    fn token_breakdown_on_a_blank_session_is_zeroed_not_an_error() {
        // Live shape of a session that never ran: `contextPressure` is `{}`.
        let projections = json!({
            "asOfSeq": 6,
            "values": {
                "tokenUsage": {
                    "uncachedInputTokens": 0,
                    "outputTokens": 0,
                    "cacheReadTokens": 0,
                    "cacheWriteTokens": 0
                },
                "contextPressure": {},
                "contextBreakdown": { "systemTokens": 0, "toolsTokens": 0, "messageTokens": 0 }
            }
        });
        let b = dsh_token_breakdown_from_projections(&projections);
        assert_eq!(b.total_tokens, 0);
        // No window known yet — the panel must show "—", not 0%.
        assert_eq!(b.projected_tokens, None);
        assert_eq!(b.context_percent, None);
    }

    #[test]
    fn token_breakdown_tolerates_a_missing_projections_block() {
        assert_eq!(
            dsh_token_breakdown_from_projections(&json!({})),
            DshTokenBreakdown::default()
        );
    }

    #[test]
    fn token_breakdown_rejects_a_uri_without_a_session_id() {
        // Guards the URI parse without reaching a server: a bare scheme has no
        // id, so this must fail before `with_client` tries to start one.
        let err = dsh_token_breakdown("dsh://").expect_err("no id");
        assert!(err.contains("invalid dsh URI"), "got {err}");
    }

    // ── History cache ─────────────────────────────────────────────────────

    /// A fake `session.history` face that pages the way dsh documents: pages are
    /// cut on append-origin message boundaries, `beforeSeq` walks backwards, and
    /// `maxMessages` counts whole messages — every raw event they own, streaming
    /// chunks included, rides along.
    struct FakeHistory {
        /// `(message index, event)`, in seq order.
        events: Mutex<Vec<(usize, Value)>>,
        calls: Mutex<Vec<(Option<i64>, Option<usize>)>>,
    }

    impl FakeHistory {
        /// A session of `messages` messages, each one a `user/message` followed
        /// by `chunks` streaming chunks — the ratio that makes a full re-read
        /// expensive (62,853 of 63,534 events on the session that motivated the
        /// cache were chunks).
        fn with_messages(messages: usize, chunks: usize) -> Self {
            let mut events = Vec::new();
            let mut seq = 0i64;
            for m in 0..messages {
                events.push((m, wire_user_event(seq, &format!("msg{m}"))));
                seq += 1;
                for _ in 0..chunks {
                    events.push((m, wire_chunk_event(seq)));
                    seq += 1;
                }
            }
            Self {
                events: Mutex::new(events),
                calls: Mutex::new(Vec::new()),
            }
        }

        /// Append one more message, as a live session would between two polls.
        fn append_message(&self, label: &str, chunks: usize) {
            let mut events = self.events.lock().unwrap();
            let mut seq = events.last().map_or(0, |(_, e)| e["event"]["seq"].as_i64().unwrap() + 1);
            let m = events.last().map_or(0, |(m, _)| m + 1);
            events.push((m, wire_user_event(seq, label)));
            seq += 1;
            for _ in 0..chunks {
                events.push((m, wire_chunk_event(seq)));
                seq += 1;
            }
        }

        fn fetch(&self, before_seq: Option<i64>, max: Option<usize>) -> Result<Value, String> {
            self.calls.lock().unwrap().push((before_seq, max));
            let events = self.events.lock().unwrap();
            let visible: Vec<&(usize, Value)> = events
                .iter()
                .filter(|(_, e)| match before_seq {
                    Some(before) => e["event"]["seq"].as_i64().unwrap() < before,
                    None => true,
                })
                .collect();
            let mut ids: Vec<usize> = visible.iter().map(|(m, _)| *m).collect();
            ids.dedup();
            let keep: std::collections::HashSet<usize> = match max {
                Some(n) if ids.len() > n => ids[ids.len() - n..].iter().copied().collect(),
                _ => ids.iter().copied().collect(),
            };
            let page: Vec<Value> = visible
                .iter()
                .filter(|(m, _)| keep.contains(m))
                .map(|(_, e)| e.clone())
                .collect();
            let has_more = keep.len() < ids.len();
            Ok(json!({ "events": page, "hasMore": has_more }))
        }

        fn calls(&self) -> Vec<(Option<i64>, Option<usize>)> {
            self.calls.lock().unwrap().clone()
        }
    }

    /// One durable `user/message` event, wrapped the way `session.history`
    /// entries are (`{event, view?}`).
    fn wire_user_event(seq: i64, text: &str) -> Value {
        json!({
            "event": {
                "type": "user/message",
                "seq": seq,
                "time": 1787109640994u64,
                "data": {
                    "source": { "kind": "user" },
                    "content": [{ "type": "text", "text": text }],
                },
            }
        })
    }

    /// A streaming chunk: carried by every page, dropped by `normalize`.
    fn wire_chunk_event(seq: i64) -> Value {
        json!({ "event": { "type": "assistant/chunk", "seq": seq, "data": { "delta": "…" } } })
    }

    fn text_of(record: &Value) -> String {
        record["message"]["content"][0]["text"]
            .as_str()
            .unwrap_or_default()
            .to_string()
    }

    /// The regression this cache exists for: a live-tail poll used to re-request
    /// the caller's tail length as dsh `maxMessages`, and dsh answers that with
    /// every raw event those messages own — 13.3 MB per poll on the session that
    /// motivated this, 99% of it chunks `normalize` throws away. The second read
    /// must ask only for the small refresh window.
    #[test]
    fn a_second_read_refreshes_only_the_tail_window() {
        let id = "session-cache-tail-window";
        forget_history(id);
        let fake = FakeHistory::with_messages(5, 20);

        let first = history_with(id, Some(3), |b, m| fake.fetch(b, m)).unwrap();
        assert_eq!(first.len(), 3, "the tail the caller asked for");
        assert_eq!(text_of(&first[2]), "msg4");

        fake.append_message("msg5", 20);
        let second = history_with(id, Some(3), |b, m| fake.fetch(b, m)).unwrap();

        assert_eq!(
            fake.calls(),
            vec![
                (None, Some(BACKFILL_MESSAGES)),
                (None, Some(TAIL_REFRESH_MESSAGES))
            ],
            "one page cold, then only the refresh window"
        );
        assert_eq!(text_of(&second[2]), "msg5", "the new message is in");
        assert_eq!(second.len(), 3);
    }

    /// A bare tail page is NOT the whole history: measured live on a
    /// 63,534-event session, `session.history` with no `maxMessages` answered
    /// `hasMore: true` carrying only the last 26,510 events. So a full read has
    /// to walk `beforeSeq` back until a page says `hasMore: false` — returning
    /// the first page would silently drop the front of the transcript.
    #[test]
    fn a_full_read_walks_back_until_the_history_is_complete() {
        let id = "session-cache-full";
        forget_history(id);
        // 150 messages against a 60-message page: no single page can hold it.
        let fake = FakeHistory::with_messages(150, 2);

        let all = history_with(id, None, |b, m| fake.fetch(b, m)).unwrap();

        assert_eq!(all.len(), 150, "every message, not just the last page");
        assert_eq!(text_of(&all[0]), "msg0");
        assert_eq!(text_of(&all[149]), "msg149");
        let calls = fake.calls();
        assert!(calls.len() > 1, "one page cannot be the whole history");
        assert!(
            calls[1..].iter().all(|(before, _)| before.is_some()),
            "every page after the tail walks beforeSeq: {calls:?}"
        );
    }

    /// A tail longer than one page pulls earlier pages in, and stops as soon as
    /// it is covered rather than reading the whole session.
    #[test]
    fn a_tail_wider_than_one_page_backfills_but_stops_when_covered() {
        let id = "session-cache-backfill";
        forget_history(id);
        let fake = FakeHistory::with_messages(300, 1);

        let tail = history_with(id, Some(100), |b, m| fake.fetch(b, m)).unwrap();

        assert_eq!(tail.len(), 100);
        assert_eq!(text_of(&tail[99]), "msg299", "anchored at the end");
        assert_eq!(text_of(&tail[0]), "msg200");
        assert!(
            fake.calls().len() < 5,
            "covered in a couple of pages, not by reading all 300: {:?}",
            fake.calls()
        );
    }

    /// A refresh window that does not reach back to where the cache stopped
    /// means messages landed in between. Folding it on would skip them, so the
    /// cache is dropped and the read starts over.
    #[test]
    fn a_gapped_refresh_drops_the_cache_instead_of_skipping_messages() {
        let id = "session-cache-gap";
        forget_history(id);
        let fake = FakeHistory::with_messages(3, 1);
        history_with(id, Some(3), |b, m| fake.fetch(b, m)).unwrap();

        // More new messages than the refresh window covers.
        for i in 0..TAIL_REFRESH_MESSAGES + 3 {
            fake.append_message(&format!("late{i}"), 1);
        }
        let after = history_with(id, Some(20), |b, m| fake.fetch(b, m)).unwrap();

        assert_eq!(
            after.len(),
            3 + TAIL_REFRESH_MESSAGES + 3,
            "nothing skipped: {after:?}"
        );
        assert_eq!(text_of(&after[0]), "msg0");
    }

    /// A poll that finds nothing new must not grow the transcript.
    #[test]
    fn a_refresh_with_no_new_events_changes_nothing() {
        let id = "session-cache-idle";
        forget_history(id);
        let fake = FakeHistory::with_messages(4, 3);

        let first = history_with(id, Some(10), |b, m| fake.fetch(b, m)).unwrap();
        let second = history_with(id, Some(10), |b, m| fake.fetch(b, m)).unwrap();
        assert_eq!(first, second);
        assert_eq!(first.len(), 4);
    }

    /// `n` counts normalized records — the tail Fleet's UI asked for — and is
    /// applied after the fold, not passed to dsh as `maxMessages`.
    #[test]
    fn the_tail_length_trims_normalized_records() {
        let id = "session-cache-trim";
        forget_history(id);
        let fake = FakeHistory::with_messages(6, 5);

        let tail = history_with(id, Some(2), |b, m| fake.fetch(b, m)).unwrap();
        assert_eq!(tail.len(), 2);
        assert_eq!(text_of(&tail[0]), "msg4");
        assert_eq!(text_of(&tail[1]), "msg5");
    }

    /// The cache holds a bounded number of sessions; the one being read is never
    /// the one evicted.
    #[test]
    fn the_cache_is_bounded_and_never_evicts_the_session_being_read() {
        for i in 0..HISTORY_CACHE_CAP + 4 {
            let id = format!("session-cache-cap-{i}");
            let fake = FakeHistory::with_messages(2, 1);
            let out = history_with(&id, Some(2), |b, m| fake.fetch(b, m)).unwrap();
            assert_eq!(out.len(), 2, "{id} read back its own transcript");
        }
        assert!(lock(history_slot()).len() <= HISTORY_CACHE_CAP);
    }

    #[test]
    fn continuity_is_overlap_or_the_very_next_seq() {
        let events = history_events(&json!({
            "events": [wire_user_event(10, "x"), wire_user_event(11, "y")],
        }));
        assert!(page_continues(10, &events), "overlapping page");
        assert!(page_continues(9, &events), "starts exactly after the cache");
        assert!(!page_continues(8, &events), "seq 9 would be skipped");
        assert!(page_continues(5, &[]), "an empty page adds nothing");
    }

    #[test]
    fn folds_key_off_the_cached_seq_span() {
        let events = history_events(&json!({
            "events": [
                wire_user_event(1, "old"),
                wire_user_event(2, "mid"),
                wire_user_event(3, "new"),
            ],
        }));
        assert_eq!(events_after(2, &events).len(), 1);
        assert_eq!(events_before(2, &events).len(), 1);
        assert_eq!(min_seq(&events), Some(1));
        assert_eq!(max_seq(&events), Some(3));
    }

    #[test]
    fn history_events_keeps_the_durable_event_only() {
        // Entries pair the durable event with a transient host-computed view.
        let value = json!({
            "events": [
                { "event": { "type": "user/message", "seq": 7 }, "view": { "for": "call" } },
                { "event": { "type": "turn/end", "seq": 8 } }
            ],
            "hasMore": false
        });
        let events = history_events(&value);
        assert_eq!(events.len(), 2);
        assert_eq!(events[0]["type"], "user/message");
        assert!(events[0].get("view").is_none(), "the view is not durable");
    }

    #[test]
    fn history_events_on_an_empty_answer() {
        assert!(history_events(&json!({})).is_empty());
    }

    #[test]
    fn owns_only_its_own_scheme() {
        let source = DshSource::new();
        assert!(source.owns_path("dsh://session-abc"));
        assert!(!source.owns_path("codex://rollout-abc"));
        assert!(!source.owns_path("/Users/me/.claude/projects/x/y.jsonl"));
    }

    #[test]
    fn session_id_needs_a_non_empty_tail() {
        assert_eq!(
            DshSource::session_id_of("dsh://session-abc"),
            Some("session-abc")
        );
        assert_eq!(DshSource::session_id_of("dsh://"), None);
        assert_eq!(DshSource::session_id_of("session-abc"), None);
    }

    /// Fleet carries one model string; dsh wants provider and model separately.
    #[test]
    fn splits_a_model_string_at_its_first_slash() {
        assert_eq!(
            split_model("openrouter/anthropic/claude-haiku-4.5"),
            Some(("openrouter", "anthropic/claude-haiku-4.5")),
            "the provider is the first segment; the rest is the model verbatim"
        );
        assert_eq!(split_model("deepseek/deepseek-chat"), Some(("deepseek", "deepseek-chat")));
    }

    /// A model with no provider cannot be selected, and guessing one would point
    /// the session at a provider the user never configured — leave it on the
    /// harness default instead.
    #[test]
    fn a_model_without_a_provider_selects_nothing() {
        assert_eq!(split_model("claude-haiku-4.5"), None);
        assert_eq!(split_model(""), None);
        assert_eq!(split_model("/model"), None);
        assert_eq!(split_model("provider/"), None);
    }

    #[test]
    fn session_ids_take_dsh_shape() {
        assert_eq!(
            normalize_session_id(Some("session-abc")),
            "session-abc",
            "an id that is already dsh-shaped passes through"
        );
        assert_eq!(
            normalize_session_id(Some("abc")),
            "session-abc",
            "a bare id (Claude's shape) gets the prefix"
        );
        let minted = normalize_session_id(None);
        assert!(minted.starts_with("session-"));
        assert_ne!(minted, normalize_session_id(None), "each mint is unique");
    }

    #[test]
    fn blank_session_ids_are_minted_not_prefixed() {
        assert_ne!(normalize_session_id(Some("   ")), "session-   ");
        assert!(normalize_session_id(Some("   ")).len() > "session-".len());
    }

    /// The spawn path refuses to guess a workspace: `session.create`'s `cwd`
    /// decides where the agent's tools run.
    #[test]
    fn spawn_requires_a_workspace() {
        let err = DshSource::new()
            .spawn(&crate::agent_source::SpawnSpec::default())
            .unwrap_err();
        assert!(err.contains("workspace_path"), "unexpected error: {err}");
    }

    /// The chat preset orchestration, end to end against a scripted `call`: an
    /// ordinary workspace asks dsh nothing, and the chat workspace walks
    /// `agentPreset.list` → `agentPreset.read` → author, then names the preset.
    ///
    /// Repoints both homes, so it holds the shared home lock.
    #[test]
    fn only_a_chat_workspace_resolves_a_preset_and_it_walks_both_rpcs() {
        let _guard = crate::session::fleet_home_lock();
        let base = std::env::temp_dir()
            .join(format!("fleet-dsh-preset-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(base.join("dsh-home")).unwrap();
        let prev_dsh = std::env::var_os("DSH_HOME");
        let prev_fleet = std::env::var_os("FLEET_HOME");
        std::env::set_var("DSH_HOME", base.join("dsh-home"));
        std::env::set_var("FLEET_HOME", &base);

        let calls = std::cell::RefCell::new(Vec::<String>::new());
        let call = |method: &str, _payload: Value| -> Result<Value, String> {
            calls.borrow_mut().push(method.to_string());
            Ok(match method {
                "agentPreset.list" => json!({
                    "presets": [{ "id": "standard", "trust": "system", "isDefault": true }],
                    "authorable": true,
                    "hasDocument": true,
                }),
                "agentPreset.read" => json!({
                    "agentPreset": "standard",
                    "trust": "system",
                    "content": "- id: agent-instructions\n  name: 'x'\n  config:\n    maxBytes: 65536\n",
                }),
                other => panic!("unexpected rpc {other}"),
            })
        };

        // An ordinary workspace must not even ask.
        assert_eq!(resolve_chat_preset("/Users/foo/my-project", &call), None);
        assert!(calls.borrow().is_empty(), "no RPC for a normal workspace");

        let chat = base.join(".fleet/chat");
        std::fs::create_dir_all(&chat).unwrap();
        let got = resolve_chat_preset(&chat.to_string_lossy(), &call);
        assert_eq!(got.as_deref(), Some(crate::dsh_chat_preset::CHAT_PRESET_ID));
        assert_eq!(
            *calls.borrow(),
            vec!["agentPreset.list", "agentPreset.read"],
            "the deployment default is read, never hardcoded",
        );

        // The authored composition redirects the instruction plugin away from the
        // real $DSH_HOME, which is the whole point.
        let composed = std::fs::read_to_string(
            base.join("dsh-home/.agent-presets")
                .join(crate::dsh_chat_preset::CHAT_PRESET_ID)
                .join("agent.cordis.yml"),
        )
        .unwrap();
        assert!(composed.contains("dshHome:"), "{composed}");
        assert!(composed.contains("dsh-chat-home"), "{composed}");
        assert!(
            !base.join("dsh-chat-home/AGENTS.md").exists(),
            "the stand-in home must stay empty or the doctrine comes back",
        );

        // A transport failure degrades to "no preset" rather than failing a spawn.
        let failing = |_: &str, _: Value| -> Result<Value, String> { Err("refused".into()) };
        assert_eq!(resolve_chat_preset(&chat.to_string_lossy(), failing), None);

        match prev_dsh {
            Some(v) => std::env::set_var("DSH_HOME", v),
            None => std::env::remove_var("DSH_HOME"),
        }
        match prev_fleet {
            Some(v) => std::env::set_var("FLEET_HOME", v),
            None => std::env::remove_var("FLEET_HOME"),
        }
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn dsh_sessions_expose_no_file_path() {
        // The transcript is a zstd-framed log the server owns; handing out a
        // path would invite a reader that cannot parse it.
        assert_eq!(
            DshSource::new().resolve_file_path("dsh://session-abc"),
            None
        );
    }

    /// Verbatim `llm.models` value observed live (dsh 0.1.0-rc.7), trimmed to
    /// one model per interesting shape. The three openrouter rows are the whole
    /// reason this mapping is not a one-liner: a model may carry no `reasoning`
    /// object at all, or carry `efforts` with no `defaultEffort`.
    fn live_models_value() -> Value {
        json!({
            "groups": [
                {
                    "id": "deepseek-official",
                    "name": "DeepSeek",
                    "models": [{
                        "id": "deepseek-v4-pro",
                        "name": "DeepSeek-V4-Pro",
                        "reasoning": {
                            "efforts": [
                                { "id": "off", "name": "Off" },
                                { "id": "low", "name": "Low" },
                                { "id": "high", "name": "High" },
                                { "id": "max", "name": "Max" }
                            ],
                            "defaultEffort": "high"
                        }
                    }]
                },
                {
                    "id": "openrouter",
                    "name": "openrouter",
                    "models": [
                        { "id": "ai21/jamba-large-1.7", "name": "AI21: Jamba Large 1.7" },
                        {
                            "id": "aion-labs/aion-2.0",
                            "name": "AionLabs: Aion-2.0",
                            "reasoning": {
                                "efforts": [
                                    { "id": "off", "name": "Off" },
                                    { "id": "minimal", "name": "Minimal" }
                                ]
                            }
                        },
                        {
                            "id": "anthropic/claude-haiku-4.5",
                            "name": "Anthropic: Claude Haiku 4.5",
                            "description": "Fast and cheap."
                        }
                    ]
                }
            ],
            "failures": []
        })
    }

    #[test]
    /// The catalogue's whole job is to hand the launcher strings that survive
    /// the round trip back into dsh's two fields, so every mapped model's
    /// `spec` must re-split into exactly the provider + model it came from.
    fn the_catalogue_maps_every_group_and_round_trips_each_spec() {
        let cat = parse_model_catalog(&live_models_value());

        assert_eq!(cat.groups.len(), 2, "both providers must survive mapping");
        assert!(cat.failures.is_empty());

        let deepseek = &cat.groups[0];
        assert_eq!(deepseek.id, "deepseek-official");
        assert_eq!(deepseek.name, "DeepSeek");
        assert_eq!(deepseek.models.len(), 1);
        assert_eq!(cat.groups[1].models.len(), 3);

        for group in &cat.groups {
            for model in &group.models {
                assert_eq!(
                    split_model(&model.spec),
                    Some((group.id.as_str(), model.id.as_str())),
                    "{} must address {}/{}",
                    model.spec,
                    group.id,
                    model.id
                );
            }
        }
        // openrouter ids carry their own slash, so the spec is three segments —
        // the case that makes `split_model`'s split-on-*first*-slash load-bearing.
        assert_eq!(
            cat.groups[1].models[2].spec,
            "openrouter/anthropic/claude-haiku-4.5"
        );
    }

    #[test]
    /// dsh publishes a per-model effort scale, which is why Fleet cannot reuse
    /// Claude's or codex's fixed ladder here. The three states a model can be
    /// in — full scale with a default, scale without a default, and no
    /// reasoning control at all — must all come through distinguishable.
    fn effort_scales_come_through_per_model() {
        let cat = parse_model_catalog(&live_models_value());

        let pro = &cat.groups[0].models[0];
        assert_eq!(
            pro.efforts.iter().map(|e| e.id.as_str()).collect::<Vec<_>>(),
            ["off", "low", "high", "max"]
        );
        assert_eq!(pro.efforts[3].name, "Max");
        assert_eq!(pro.default_effort.as_deref(), Some("high"));

        // No `reasoning` object: an empty scale, not a missing one. The UI hides
        // the effort menu rather than showing a stale ladder from another model.
        let jamba = &cat.groups[1].models[0];
        assert!(jamba.efforts.is_empty());
        assert_eq!(jamba.default_effort, None);

        // A scale with no default — dsh decides, so Fleet must not invent one.
        let aion = &cat.groups[1].models[1];
        assert_eq!(aion.efforts.len(), 2);
        assert_eq!(aion.default_effort, None);

        assert_eq!(
            cat.groups[1].models[2].description.as_deref(),
            Some("Fast and cheap.")
        );
        assert_eq!(cat.groups[0].models[0].description, None);
    }

    #[test]
    /// `failures` is an **array** of per-provider errors, not a count, and it
    /// arrives alongside a populated `groups` — a route Fleet cannot reach must
    /// not blank the routes it can. (Also the guard against an earlier reading
    /// of this field as a number, which would have parsed to nothing.)
    fn a_failed_provider_does_not_cost_the_working_ones() {
        let mut value = live_models_value();
        value["failures"] = json!([{
            "id": "moonshot",
            "name": "Moonshot",
            "message": "missing credential"
        }]);

        let cat = parse_model_catalog(&value);
        assert_eq!(cat.groups.len(), 2, "the reachable providers still list");
        assert_eq!(cat.failures.len(), 1);
        assert_eq!(cat.failures[0].id, "moonshot");
        assert_eq!(cat.failures[0].message, "missing credential");
    }

    #[test]
    /// An unusable row is dropped, never mapped to a spec that would silently
    /// no-op at `session.selectModel`: `split_model` ignores a string without a
    /// provider prefix, so an id-less model offered in the menu would look
    /// selectable and change nothing.
    fn rows_without_an_id_are_dropped_rather_than_offered() {
        let value = json!({
            "groups": [
                { "name": "no id here", "models": [{ "id": "m", "name": "M" }] },
                {
                    "id": "ok",
                    "name": "OK",
                    "models": [
                        { "name": "nameless" },
                        { "id": "", "name": "blank" },
                        { "id": "good", "name": "Good" }
                    ]
                }
            ],
            "failures": []
        });

        let cat = parse_model_catalog(&value);
        assert_eq!(cat.groups.len(), 1, "the id-less group is dropped");
        assert_eq!(cat.groups[0].id, "ok");
        assert_eq!(cat.groups[0].models.len(), 1);
        assert_eq!(cat.groups[0].models[0].spec, "ok/good");

        // A shape with nothing recognisable is an empty catalogue, not a panic.
        assert_eq!(parse_model_catalog(&json!({})), DshModelCatalog::default());
    }
}
