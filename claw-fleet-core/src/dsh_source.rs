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

fn server_slot() -> &'static Mutex<Option<DshServer>> {
    SERVER.get_or_init(|| Mutex::new(None))
}

fn watcher_slot() -> &'static Mutex<Option<DshEventWatcher>> {
    WATCHER.get_or_init(|| Mutex::new(None))
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
                    // touches a concurrently running Fleet's instance.
                    crate::dsh_server::reap_orphans();
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
    fn prompt(client: &DshClient, session_id: &str, prompt: &str) -> Result<(), String> {
        client
            .call(
                "session.prompt",
                json!({
                    "sessionId": session_id,
                    // "queue" appends to the session's inbox; "steer" would cut
                    // into a turn already running, which is not what either of
                    // Fleet's launch paths means.
                    "mode": "queue",
                    "content": [{ "type": "text", "text": prompt }],
                }),
            )
            .map(|_| ())
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

/// Channel B (prompt-prepend) of dsh guidance: if the dsh **PRD** block is
/// installed (its `$DSH_HOME/AGENTS.md` sentinel is present) AND the workspace
/// has active TASKS.md plans, prepend the same `<system-reminder>` block Claude
/// gets via the `fleet prd-context` UserPromptSubmit hook.
///
/// Channel A ([`crate::dsh_guidance`]) is static: the AGENTS.md blocks enter a
/// session's history once, as the agent-instructions baseline. The macro plan is
/// not static — it changes as boxes get ticked — so it rides the prompt instead,
/// exactly as [`crate::codex_launch`] does for codex. Every Fleet-driven dsh turn
/// goes through `session.prompt`, so prepending at both prompt sites gives the
/// per-turn re-injection Claude's hook provides.
///
/// Gated on the dsh PRD concept specifically: static and dynamic injection move
/// together with the PRD toggle. No PRD block / no active plan → the prompt is
/// returned unchanged.
fn maybe_prepend_active_plans(workspace_path: &str, session_id: &str, prompt: &str) -> String {
    if workspace_path.trim().is_empty() || !crate::dsh_guidance::is_dsh_prd_installed() {
        return prompt.to_string();
    }
    match crate::prd_tasks::render_active_plans_reminder(
        std::path::Path::new(workspace_path),
        Some(session_id),
    ) {
        Some(reminder) => format!("{reminder}\n\n{prompt}"),
        None => prompt.to_string(),
    }
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
        let listed =
            self.with_client(|client| client.call("session.list", json!({})).map_err(Into::into));

        match listed {
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
        // `maxMessages` is absent, so this is the tail page: dsh pages by whole
        // append-origin messages and the tail additionally carries the
        // in-flight partial.
        let value = self.with_client(|client| {
            client
                .call("session.history", json!({ "sessionId": id }))
                .map_err(Into::into)
        })?;

        // Normalised, not raw: every Fleet client reads Claude Code's message
        // vocabulary, and dsh's own records match none of it. See
        // `crate::dsh_messages`.
        Ok(crate::dsh_messages::normalize(&history_events(&value)))
    }

    fn get_messages_tail(&self, path: &str, n: usize) -> Result<Vec<Value>, String> {
        let id = Self::session_id_of(path).ok_or_else(|| format!("invalid dsh URI: {path}"))?;
        let value = self.with_client(|client| {
            client
                .call(
                    "session.history",
                    json!({ "sessionId": id, "maxMessages": n }),
                )
                .map_err(Into::into)
        })?;

        // Normalised, not raw: every Fleet client reads Claude Code's message
        // vocabulary, and dsh's own records match none of it. See
        // `crate::dsh_messages`.
        Ok(crate::dsh_messages::normalize(&history_events(&value)))
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
            let created = client
                .call(
                    "session.create",
                    json!({ "cwd": spec.workspace_path, "sessionId": session_id }),
                )
                .map_err(String::from)?;
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
            let prompt =
                maybe_prepend_active_plans(&spec.workspace_path, &session_id, &spec.prompt);
            Self::prompt(client, &session_id, &prompt)
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
            let prompt = maybe_prepend_active_plans(&spec.workspace_path, &session_id, prompt);
            Self::prompt(client, &session_id, &prompt)
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
/// No model id either — a session's model appears only in its single
/// `request/header` event near the head of the log, which would cost a full
/// `session.history` fetch to reach.
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

    /// Channel B: the active-plans reminder rides the prompt, but only when the
    /// dsh PRD block is installed. Both branches, against a temp `$DSH_HOME` and
    /// a temp workspace holding a real TASKS.md.
    ///
    /// Self-guarding on the shared home lock (see `tests/home_env_lock_guard.rs`)
    /// because it repoints `DSH_HOME`.
    #[test]
    fn active_plans_ride_the_prompt_only_when_the_prd_block_is_installed() {
        let _guard = crate::session::fleet_home_lock();
        let base = std::env::temp_dir()
            .join(format!("fleet-dsh-prepend-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let home = base.join("dsh-home");
        let ws = base.join("ws");
        std::fs::create_dir_all(&home).unwrap();
        std::fs::create_dir_all(&ws).unwrap();
        std::fs::write(
            ws.join("TASKS.md"),
            "# TASKS\n\n<!-- fleet:prd:begin id=\"demo-plan\" v=\"2\" -->\n\n\
             **Plan:** Demo\n\n- [ ] **P1** — do the thing\n\n\
             <!-- fleet:prd:end id=\"demo-plan\" -->\n",
        )
        .unwrap();

        let prev = std::env::var_os("DSH_HOME");
        std::env::set_var("DSH_HOME", &home);
        let ws_str = ws.to_string_lossy().to_string();

        // No AGENTS.md at all → the PRD concept is off, so nothing is prepended
        // even though the workspace has a live plan.
        assert_eq!(
            maybe_prepend_active_plans(&ws_str, "session-x", "hello"),
            "hello",
            "no dsh PRD block installed → prompt untouched"
        );

        crate::dsh_guidance::reconcile_dsh_agents_md(
            crate::dsh_guidance::DshGuidanceSet {
                prd: true,
                ..Default::default()
            },
            "Boss",
            "en",
        )
        .unwrap();

        let out = maybe_prepend_active_plans(&ws_str, "session-x", "hello");
        assert!(out.ends_with("hello"), "the original prompt stays at the tail");
        assert!(
            out.contains("demo-plan") && out.contains("P1"),
            "the active plan must reach the model: {out}"
        );

        // An empty workspace path is the resume-without-a-workspace case: there is
        // no TASKS.md to resolve, and Path::new("") would resolve against cwd.
        assert_eq!(maybe_prepend_active_plans("", "session-x", "hello"), "hello");

        match prev {
            Some(v) => std::env::set_var("DSH_HOME", v),
            None => std::env::remove_var("DSH_HOME"),
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
}
