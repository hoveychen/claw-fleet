//! Side questions on a Codex thread, answered in a fork made by **copying the
//! rollout** (the Codex backend of [`crate::session_explain`]).
//!
//! Codex has a fork of its own — `codex exec fork <thread> --ephemeral` — and
//! it is *not* used, for the one reason this feature exists: the prompt cache.
//! Codex keys the cache on the thread's `session_id`
//! (`core/src/client.rs::prompt_cache_key`), and on the shipped CLI
//! (0.153.4 … 0.155.1) an ephemeral fork mints a fresh key, so the replayed
//! history — byte-identical to the source's — routes to a cold cache. Measured
//! 2026-09-20 on four forks: 0–45% cached. `main` fixes this (the fork reuses
//! the source key) but only in the 0.156 alpha.
//!
//! So the fork is made by hand: copy the rollout under a new file name, give
//! the copy's `session_meta` a new `id` (that is the thread id `exec resume`
//! finds it by) while **keeping `session_id`** (the cache key), then
//! `codex exec resume <copy-id> … -- <prompt>`. Measured 99% cached. The
//! costs of that route, and how each is paid for:
//!
//! - **The copy is a real thread to Codex.** It appends to the copy and writes
//!   a `threads` row for it in `state_5.sqlite`. The file is deleted when the
//!   answer is in ([`crate::session_explain::RemoveOnDrop`]); the row stays,
//!   so the copy id is marked internal up front
//!   ([`crate::codex_image::mark_internal_thread`]) and `codex_source` never
//!   mints a `SessionInfo` for it. The source rollout is never opened for
//!   writing.
//! - **The tool surface must match.** A Fleet-launched thread carries the
//!   `fleet mcp` server and the guard hook; the fork gets the same `-c`
//!   overrides (minus the sandbox bypass), and its id is noted in
//!   `launch_spec` so `fleet mcp` advertises the same tool set to it.
//! - **One turn, text only.** Codex has no `--max-turns`; the prompt forbids
//!   tools, the sandbox is `read-only` with approvals off, and a turn that ran
//!   a tool but produced no text is reported as an error.
//! - **No streaming.** `codex exec --json` emits whole items, never text
//!   deltas, so the answer lands in one piece when `item.completed` arrives.
//!
//! The model is mirrored with `-m` (Codex otherwise falls back to
//! `config.toml`'s default and inserts a `<model_switch>` into the history,
//! breaking the prefix); it comes from the `launch_spec` note for
//! Fleet-launched threads, else from the rollout's last `turn_context`.

use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::agent_source::{ForkAskOutcome, ForkAskSpec};
use crate::model_cost::TurnUsage;
use crate::session_explain::{
    drive_fork_process, flags_without_prompt, fork_stderr_sink, ForgetLaunchSpec, RemoveOnDrop,
    ANSWER_TIMEOUT,
};

/// `-c` overrides that keep the fork from acting: commands run read-only and
/// nothing waits on an approver that a headless fork does not have.
pub fn fork_sandbox_args() -> Vec<String> {
    ["sandbox_mode=\"read-only\"", "approval_policy=\"never\""]
        .iter()
        .flat_map(|kv| ["-c".to_string(), (*kv).to_string()])
        .collect()
}

/// The fork's rollout text, derived from the source rollout's: the first
/// line's `session_meta.payload.id` becomes `fork_id` and everything else is
/// carried over verbatim — in particular `session_id`, which Codex uses as the
/// prompt cache key. Returns the text and the source thread id.
pub fn fork_rollout_copy(source: &str, fork_id: &str) -> Result<(String, String), String> {
    let mut lines = source.split_inclusive('\n');
    let first = lines.next().ok_or_else(|| "rollout is empty".to_string())?;
    let mut meta: Value = serde_json::from_str(first.trim())
        .map_err(|e| format!("rollout first line is not JSON: {e}"))?;
    if meta.get("type").and_then(Value::as_str) != Some("session_meta") {
        return Err("rollout does not open with session_meta".to_string());
    }
    let payload = meta
        .get_mut("payload")
        .and_then(Value::as_object_mut)
        .ok_or_else(|| "session_meta has no payload".to_string())?;
    let source_id = payload
        .get("id")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| "session_meta has no id".to_string())?
        .to_string();
    // A rollout predating the split field: the cache key is the thread id.
    if payload.get("session_id").and_then(Value::as_str).is_none() {
        payload.insert("session_id".to_string(), Value::String(source_id.clone()));
    }
    payload.insert("id".to_string(), Value::String(fork_id.to_string()));
    let mut out = serde_json::to_string(&meta).map_err(|e| e.to_string())?;
    out.push('\n');
    for l in lines {
        out.push_str(l);
    }
    if !out.ends_with('\n') {
        out.push('\n');
    }
    Ok((out, source_id))
}

/// File name for the fork's rollout, next to the source's: the source name
/// with its thread id swapped for `fork_id`, always plain `.jsonl` (a
/// `.jsonl.zst` source is written back decompressed). A source name that does
/// not carry its id falls back to Codex's own `rollout-<ts>-<id>.jsonl` shape.
pub fn fork_rollout_name(source_name: &str, source_id: &str, fork_id: &str) -> String {
    let stem = source_name
        .strip_suffix(".jsonl.zst")
        .or_else(|| source_name.strip_suffix(".jsonl"))
        .unwrap_or(source_name);
    if !source_id.is_empty() && stem.contains(source_id) {
        return format!("{}.jsonl", stem.replacen(source_id, fork_id, 1));
    }
    format!(
        "rollout-{}-{fork_id}.jsonl",
        chrono::Utc::now().format("%Y-%m-%dT%H-%M-%S")
    )
}

/// The `codex exec resume` argv for the fork. Pure so the cache-governing
/// pieces can be asserted: `fleet_owned_args` are the source thread's Fleet
/// overrides (MCP server + guard hook) with the sandbox bypass stripped,
/// `transport_args` the ChatGPT transport pins, then the read-only sandbox.
pub fn codex_fork_args(
    fork_id: &str,
    prompt: &str,
    model: Option<&str>,
    effort: Option<&str>,
    fleet_owned_args: &[String],
    transport_args: &[String],
) -> Vec<String> {
    let mut pre: Vec<String> = fleet_owned_args
        .iter()
        .filter(|a| !a.starts_with("--dangerously-"))
        .cloned()
        .collect();
    pre.extend(transport_args.iter().cloned());
    pre.extend(fork_sandbox_args());
    crate::codex_launch::build_codex_resume_args(fork_id, prompt, model, effort, &pre)
}

/// Fold of a `codex exec --json` stdout, one line at a time.
#[derive(Default, Debug)]
pub struct CodexExecFold {
    pub thread_id: Option<String>,
    /// Agent messages so far, blank-line separated.
    pub text: String,
    pub usage: Option<TurnUsage>,
    pub error: Option<String>,
    /// Commands, patches and tool calls the fork ran despite being told not to.
    pub tool_calls: u32,
}

impl CodexExecFold {
    /// Feed one stdout line. Returns the agent message it carried, if any.
    pub fn feed(&mut self, line: &str) -> Option<String> {
        let v: Value = serde_json::from_str(line.trim()).ok()?;
        match v.get("type").and_then(Value::as_str)? {
            "thread.started" => {
                self.thread_id = v
                    .get("thread_id")
                    .and_then(Value::as_str)
                    .map(str::to_string);
                None
            }
            "item.completed" => {
                let item = v.get("item")?;
                match item.get("type").and_then(Value::as_str)? {
                    "agent_message" => {
                        let text = item.get("text").and_then(Value::as_str)?.to_string();
                        if text.trim().is_empty() {
                            return None;
                        }
                        if !self.text.is_empty() {
                            self.text.push_str("\n\n");
                        }
                        self.text.push_str(&text);
                        Some(text)
                    }
                    "command_execution" | "file_change" | "mcp_tool_call" | "collab_tool_call"
                    | "web_search" => {
                        self.tool_calls += 1;
                        None
                    }
                    "error" => {
                        if let Some(m) = item.get("message").and_then(Value::as_str) {
                            self.error.get_or_insert_with(|| m.to_string());
                        }
                        None
                    }
                    _ => None,
                }
            }
            "turn.completed" => {
                let u = v.get("usage")?;
                let n = |k: &str| u.get(k).and_then(Value::as_u64).unwrap_or(0);
                let input = n("input_tokens");
                let cached = n("cached_input_tokens").min(input);
                self.usage = Some(TurnUsage {
                    input_tokens: input - cached,
                    output_tokens: n("output_tokens"),
                    cache_read_tokens: cached,
                    cache_creation_tokens: n("cache_write_input_tokens"),
                    cache_creation_1h_tokens: 0,
                    web_search_requests: 0,
                });
                None
            }
            "turn.failed" => {
                let m = v
                    .pointer("/error/message")
                    .and_then(Value::as_str)
                    .unwrap_or("codex turn failed");
                self.error = Some(m.to_string());
                None
            }
            "error" => {
                let m = v
                    .get("message")
                    .and_then(Value::as_str)
                    .unwrap_or("codex reported an error");
                self.error = Some(m.to_string());
                None
            }
            _ => None,
        }
    }

    /// Settle into the answer. `model` is what the fork was launched with;
    /// Codex's JSON does not name it, and it prices the turn.
    pub fn into_outcome(
        self,
        model: Option<&str>,
        fork_id: &str,
    ) -> Result<ForkAskOutcome, String> {
        if let Some(e) = self.error {
            return Err(e);
        }
        if self.text.trim().is_empty() {
            return Err(if self.tool_calls > 0 {
                format!(
                    "the fork ran {} tool call(s) and produced no text; try again",
                    self.tool_calls
                )
            } else {
                "the fork produced no text".to_string()
            });
        }
        let cost_usd = match (model, self.usage.as_ref()) {
            (Some(m), Some(u)) => Some(crate::model_cost::turn_cost_usd(m, u)),
            _ => None,
        };
        Ok(ForkAskOutcome {
            text: self.text,
            model: model.map(str::to_string),
            usage: self.usage,
            cost_usd,
            fork_session_id: Some(fork_id.to_string()),
        })
    }
}

/// Where the fork's rollout copy goes: beside the source rollout.
fn fork_rollout_path(source: &Path, source_id: &str, fork_id: &str) -> PathBuf {
    let name = source
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or_default();
    source.with_file_name(fork_rollout_name(name, source_id, fork_id))
}

/// Fork a Codex thread for one answer; see the module docs for the route and
/// its costs. Owns the process like the Claude backend: piped stdout folded
/// line by line, stderr to `~/.fleet/session_explain_stderr.log`, killed at
/// [`ANSWER_TIMEOUT`]; the rollout copy and the `launch_spec` note are gone on
/// every exit path.
pub(crate) fn codex_fork_ask(
    spec: &ForkAskSpec,
    on_delta: &mut dyn FnMut(&str),
) -> Result<ForkAskOutcome, String> {
    let codex = crate::codex_source::find_codex_binary()
        .ok_or_else(|| "Codex CLI not found".to_string())?;
    if !Path::new(&spec.workspace_path).is_dir() {
        return Err(format!(
            "workspace directory not found: {}",
            spec.workspace_path
        ));
    }
    let source = crate::codex_source::find_codex_rollout(&spec.session_id)
        .ok_or_else(|| format!("no rollout found for codex thread {}", spec.session_id))?;
    let content = crate::codex_source::read_session_content(&source)?;

    let fork_id = uuid::Uuid::new_v4().to_string();
    let (copy_text, source_id) = fork_rollout_copy(&content, &fork_id)?;
    let copy_path = fork_rollout_path(&source, &source_id, &fork_id);
    // Marked before the copy exists, so no scan in between can list it.
    crate::codex_image::mark_internal_thread(&fork_id);
    std::fs::write(&copy_path, copy_text)
        .map_err(|e| format!("write rollout copy {}: {e}", copy_path.display()))?;
    let _remove = RemoveOnDrop(copy_path.clone());

    let lines: Vec<Value> = content
        .lines()
        .filter(|l| !l.trim().is_empty())
        .filter_map(|l| serde_json::from_str(l).ok())
        .collect();
    let model = crate::launch_spec::model_of(&spec.session_id)
        .or_else(|| crate::codex_source::extract_model(&lines));
    let effort = crate::launch_spec::effort_of(&spec.session_id)
        .or_else(|| crate::codex_source::extract_effort(&lines));

    // A Fleet-launched thread's tool surface includes the fleet MCP server;
    // give the fork the same, and the note that makes `fleet mcp` show it the
    // same tools (see `session_explain::claude_fork_ask` for the measurement).
    let fleet_owned = crate::codex_source::codex_fleet_owned_cwd(&spec.session_id).is_some();
    let _forget = fleet_owned.then(|| {
        crate::launch_spec::record(&fork_id, model.as_deref(), effort.as_deref());
        ForgetLaunchSpec(fork_id.clone())
    });
    let fleet_owned_args = if fleet_owned {
        crate::codex_launch::fleet_decision_card_args(&[
            ("FLEET_SESSION_ID".to_string(), fork_id.clone()),
            (
                "CLAUDE_PROJECT_DIR".to_string(),
                spec.workspace_path.clone(),
            ),
        ])
    } else {
        Vec::new()
    };
    let transport_args = crate::codex_launch::codex_ws_disable_args(model.as_deref());
    let args = codex_fork_args(
        &fork_id,
        &spec.prompt,
        model.as_deref(),
        effort.as_deref(),
        &fleet_owned_args,
        &transport_args,
    );

    let stderr = fork_stderr_sink(
        &format!(
            "codex fork thread={} copy={} cwd={} fleet_owned={fleet_owned}",
            spec.session_id,
            copy_path.display(),
            spec.workspace_path
        ),
        &flags_without_prompt(&args, "--"),
    );

    let (program, args, rca_envs) =
        crate::codex_launch::wrap_codex_launch(codex, args, &spec.workspace_path)?;
    let mut cmd = crate::process_util::command(&program);
    cmd.args(&args)
        .current_dir(&spec.workspace_path)
        // `codex exec` blocks on an open stdin.
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(stderr);
    crate::codex_launch::apply_codex_launch_env(&mut cmd);
    // The launch env turns on codex's HTTP transport trail for diagnosing hung
    // turns; a fork has a watchdog instead, and the trail would swamp the
    // shared stderr log.
    cmd.env("RUST_LOG", "error");
    for (k, v) in &rca_envs {
        cmd.env(k, v);
    }
    cmd.env("FLEET_SESSION_ID", &fork_id);

    let mut fold = CodexExecFold::default();
    let exit = drive_fork_process(cmd, "codex", &mut |line| {
        if let Some(text) = fold.feed(line) {
            on_delta(&text);
        }
    })?;
    if fold.text.trim().is_empty() && fold.error.is_none() {
        return Err(if exit.timed_out {
            format!("codex fork timed out after {}s", ANSWER_TIMEOUT.as_secs())
        } else {
            format!(
                "codex fork exited without an answer (status {:?}); see session_explain_stderr.log",
                exit.code
            )
        });
    }
    fold.into_outcome(model.as_deref(), &fork_id)
}

#[cfg(test)]
mod tests {
    use super::*;

    const META: &str = r#"{"timestamp":"2026-09-19T22:44:37.480Z","ordinal":0,"type":"session_meta","payload":{"session_id":"01a0bbd7-cce1-7d50-8f62-48479d5d790d","id":"01a0bbd7-cce1-7d50-8f62-48479d5d790d","cwd":"/w","originator":"fleet","cli_version":"0.153.4","source":"exec"}}"#;

    #[test]
    fn copy_renames_the_thread_and_keeps_the_cache_key() {
        let source = format!(
            "{META}\n{{\"type\":\"turn_context\",\"payload\":{{\"model\":\"gpt-5.6-sol\"}}}}\n"
        );
        let (out, source_id) = fork_rollout_copy(&source, "fork-id").unwrap();
        assert_eq!(source_id, "01a0bbd7-cce1-7d50-8f62-48479d5d790d");
        let mut lines = out.lines();
        let meta: Value = serde_json::from_str(lines.next().unwrap()).unwrap();
        assert_eq!(meta["payload"]["id"], "fork-id");
        assert_eq!(
            meta["payload"]["session_id"],
            "01a0bbd7-cce1-7d50-8f62-48479d5d790d"
        );
        assert_eq!(meta["payload"]["originator"], "fleet");
        // The rest is carried over byte for byte.
        assert_eq!(
            lines.next().unwrap(),
            r#"{"type":"turn_context","payload":{"model":"gpt-5.6-sol"}}"#
        );
        assert!(lines.next().is_none());
    }

    #[test]
    fn copy_of_a_rollout_without_session_id_pins_the_key_to_the_source() {
        let source = r#"{"type":"session_meta","payload":{"id":"src-thread"}}"#;
        let (out, _) = fork_rollout_copy(source, "fork-id").unwrap();
        let meta: Value = serde_json::from_str(out.trim()).unwrap();
        assert_eq!(meta["payload"]["id"], "fork-id");
        assert_eq!(meta["payload"]["session_id"], "src-thread");
    }

    #[test]
    fn copy_refuses_a_file_that_is_not_a_rollout() {
        assert!(fork_rollout_copy("", "f").is_err());
        assert!(fork_rollout_copy("{\"type\":\"event_msg\"}\n", "f").is_err());
        assert!(fork_rollout_copy("not json\n", "f").is_err());
    }

    #[test]
    fn fork_name_swaps_the_id_and_drops_compression() {
        assert_eq!(
            fork_rollout_name("rollout-2026-09-19T18-44-37-src.jsonl.zst", "src", "fork"),
            "rollout-2026-09-19T18-44-37-fork.jsonl"
        );
        assert_eq!(
            fork_rollout_name("rollout-2026-09-19T18-44-37-src.jsonl", "src", "fork"),
            "rollout-2026-09-19T18-44-37-fork.jsonl"
        );
        let odd = fork_rollout_name("weird.jsonl", "src", "fork");
        assert!(
            odd.starts_with("rollout-") && odd.ends_with("-fork.jsonl"),
            "{odd}"
        );
    }

    #[test]
    fn fork_args_resume_the_copy_read_only_without_the_bypass() {
        let fleet_owned = vec![
            "--dangerously-bypass-approvals-and-sandbox".to_string(),
            "--dangerously-bypass-hook-trust".to_string(),
            "-c".to_string(),
            "mcp_servers.fleet.command=\"/f\"".to_string(),
        ];
        let transport = vec!["-c".to_string(), "model_provider=chatgpt-http".to_string()];
        let args = codex_fork_args(
            "fork-id",
            "why?",
            Some("gpt-5.6-sol"),
            Some("high"),
            &fleet_owned,
            &transport,
        );
        assert_eq!(&args[..3], &["exec", "resume", "fork-id"]);
        assert!(!args.iter().any(|a| a.starts_with("--dangerously-")));
        assert!(args.contains(&"mcp_servers.fleet.command=\"/f\"".to_string()));
        assert!(args.contains(&"model_provider=chatgpt-http".to_string()));
        assert!(args.contains(&"sandbox_mode=\"read-only\"".to_string()));
        assert!(args.contains(&"approval_policy=\"never\"".to_string()));
        let mi = args.iter().position(|a| a == "-m").unwrap();
        assert_eq!(args[mi + 1], "gpt-5.6-sol");
        assert!(args.contains(&"model_reasoning_effort=high".to_string()));
        let sep = args.iter().position(|a| a == "--").unwrap();
        assert_eq!(&args[sep + 1..], &["why?"]);
    }

    #[test]
    fn fold_collects_the_message_usage_and_thread() {
        let mut fold = CodexExecFold::default();
        assert!(fold
            .feed(r#"{"type":"thread.started","thread_id":"fork-id"}"#)
            .is_none());
        assert!(fold.feed(r#"{"type":"turn.started"}"#).is_none());
        assert!(fold.feed("garbage").is_none());
        assert!(fold
            .feed(r#"{"type":"item.completed","item":{"id":"r1","type":"reasoning","text":"hmm"}}"#)
            .is_none());
        assert_eq!(
            fold.feed(r#"{"type":"item.completed","item":{"id":"m1","type":"agent_message","text":"因为要命中缓存。"}}"#),
            Some("因为要命中缓存。".to_string())
        );
        assert!(fold
            .feed(r#"{"type":"turn.completed","usage":{"input_tokens":50000,"cached_input_tokens":49500,"output_tokens":120,"reasoning_output_tokens":0}}"#)
            .is_none());
        assert_eq!(fold.thread_id.as_deref(), Some("fork-id"));
        let out = fold.into_outcome(Some("gpt-5.6-sol"), "fork-id").unwrap();
        assert_eq!(out.text, "因为要命中缓存。");
        let u = out.usage.unwrap();
        assert_eq!(u.input_tokens, 500);
        assert_eq!(u.cache_read_tokens, 49500);
        assert_eq!(u.output_tokens, 120);
        assert!(out.cost_usd.unwrap() > 0.0);
        assert_eq!(out.fork_session_id.as_deref(), Some("fork-id"));
        assert_eq!(out.model.as_deref(), Some("gpt-5.6-sol"));
    }

    #[test]
    fn fold_reports_failures_and_silent_tool_turns() {
        let mut fold = CodexExecFold::default();
        fold.feed(r#"{"type":"turn.failed","error":{"message":"rate limited"}}"#);
        assert_eq!(fold.into_outcome(None, "f").unwrap_err(), "rate limited");

        let mut fold = CodexExecFold::default();
        fold.feed(r#"{"type":"item.completed","item":{"id":"c1","type":"command_execution","command":"ls","status":"completed"}}"#);
        fold.feed(r#"{"type":"turn.completed","usage":{"input_tokens":1,"cached_input_tokens":0,"output_tokens":1,"reasoning_output_tokens":0}}"#);
        let err = fold.into_outcome(None, "f").unwrap_err();
        assert!(err.contains("1 tool call"), "{err}");

        let mut fold = CodexExecFold::default();
        fold.feed(r#"{"type":"error","message":"stream disconnected"}"#);
        assert_eq!(
            fold.into_outcome(None, "f").unwrap_err(),
            "stream disconnected"
        );
    }

    #[test]
    fn fold_without_a_model_leaves_the_price_open() {
        let mut fold = CodexExecFold::default();
        fold.feed(
            r#"{"type":"item.completed","item":{"id":"m1","type":"agent_message","text":"ok"}}"#,
        );
        let out = fold.into_outcome(None, "f").unwrap();
        assert!(out.cost_usd.is_none());
        assert!(out.model.is_none());
    }
}
