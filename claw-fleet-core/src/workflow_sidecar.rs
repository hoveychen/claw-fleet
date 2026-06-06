//! Execution-based workflow extractor (node sidecar).
//!
//! The static byte-scanner in [`crate::workflow`] recovers the orchestration DAG
//! by regex-scanning the script text. This module offers a higher-fidelity
//! alternative: it actually *runs* the Workflow script once under a fully mocked
//! framework (see the embedded `workflow_harness.mjs`) and reports the
//! call-sites that executed, each with a resolved-prompt static head (a far more
//! robust binding fingerprint than regex extraction), the orchestration kind,
//! phase, label, agentType, and pipeline grouping.
//!
//! It is **best-effort and falls back silently**: every failure mode (no `node`
//! on PATH, timeout, non-zero exit, unparseable output) returns an [`Err`] so
//! the caller can drop back to the static path. The harness is embedded via
//! `include_str!` and written to a temp file at run time, so there is no runtime
//! file dependency and no zero-dependency violation on hosts without `node`
//! (they simply fall back).

use crate::workflow::{WorkflowNodeKind, WorkflowPhase};
use serde::Deserialize;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

/// The mocked-framework harness, embedded into the binary.
const HARNESS_JS: &str = include_str!("workflow_harness.mjs");

/// Hard wall-clock cap for one script execution. Scripts are mocked (no real
/// agents), so a healthy run finishes in tens of ms; anything approaching this
/// is a pathological loop and gets killed → caller falls back to static.
const TIMEOUT: Duration = Duration::from_secs(8);

/// One executed `agent(...)` call-site, deduplicated by source position.
#[derive(Debug, Clone, Deserialize)]
pub struct SidecarCall {
    /// Source position `line:col` of the lexical call-site (stable identity).
    pub site: String,
    /// Orchestration kind, decoded straight into the workflow enum.
    pub kind: WorkflowNodeKind,
    /// Enclosing `pipeline(...)` id, for stage chaining (None outside a pipeline).
    #[serde(rename = "pipelineId")]
    pub pipeline_id: Option<usize>,
    pub phase: Option<String>,
    pub label: Option<String>,
    #[serde(rename = "agentType")]
    pub agent_type: Option<String>,
    pub schema: bool,
    /// Resolved-prompt static head (text before the first interpolation point).
    pub fingerprint: String,
    #[serde(rename = "promptLen")]
    pub prompt_len: usize,
    /// First execution's full resolved prompt as a readable template
    /// (interpolation points rendered as "…"), truncated for display. `None`
    /// when the call had no string prompt.
    #[serde(rename = "promptResolved", default)]
    pub prompt_resolved: Option<String>,
}

/// Parsed harness output for one script.
#[derive(Debug, Clone, Deserialize)]
pub struct SidecarResult {
    /// True when the script ran to completion without throwing.
    pub ok: bool,
    /// True when the agent-call cap tripped (loop guard) — partial result.
    #[serde(default)]
    pub capped: bool,
    /// Truncated error/stack when the script threw (still may carry partial calls).
    pub error: Option<String>,
    pub name: Option<String>,
    pub description: Option<String>,
    #[serde(default)]
    pub phases: Vec<WorkflowPhase>,
    #[serde(default)]
    pub calls: Vec<SidecarCall>,
}

/// Why a sidecar extraction did not yield usable output. The caller treats every
/// variant the same way (fall back to static), but the distinction aids logging.
#[derive(Debug)]
pub enum SidecarError {
    /// `node` is not on PATH (or `FLEET_NODE_BIN` points at nothing runnable).
    NodeNotFound,
    /// The run exceeded [`TIMEOUT`] and was killed.
    Timeout,
    /// `node` exited non-zero (harness crash / syntax error in harness).
    NonZeroExit(i32),
    /// stdout was not valid harness JSON.
    BadOutput(String),
    /// I/O failure spawning or communicating with the child.
    Io(String),
}

impl std::fmt::Display for SidecarError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SidecarError::NodeNotFound => write!(f, "node not found on PATH"),
            SidecarError::Timeout => write!(f, "execution timed out after {}s", TIMEOUT.as_secs()),
            SidecarError::NonZeroExit(c) => write!(f, "node exited with code {c}"),
            SidecarError::BadOutput(s) => write!(f, "unparseable harness output: {s}"),
            SidecarError::Io(s) => write!(f, "io error: {s}"),
        }
    }
}

/// A temp file deleted on drop. We avoid the `tempfile` crate here because it is
/// a dev-dependency only (the runtime build must stay free of it); a pid + atomic
/// counter name is unique per process without needing Date/random.
struct TempFile {
    path: PathBuf,
}
impl Drop for TempFile {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}
fn write_temp(suffix: &str, content: &[u8]) -> std::io::Result<TempFile> {
    static CTR: AtomicU64 = AtomicU64::new(0);
    let n = CTR.fetch_add(1, Ordering::Relaxed);
    let path = std::env::temp_dir().join(format!("fleet-wf-{}-{n}{suffix}", std::process::id()));
    std::fs::write(&path, content)?;
    Ok(TempFile { path })
}

/// The node binary to invoke: `FLEET_NODE_BIN` override, else `node`.
fn node_bin() -> String {
    std::env::var("FLEET_NODE_BIN")
        .ok()
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| "node".to_string())
}

/// Run a workflow `script` (text) through the node harness once.
///
/// `args` is the optional invocation argument the workflow was launched with
/// (JSON-encoded string), used to resolve `args`-driven shapes; pass `None` when
/// unknown. Returns the parsed skeleton on success, or a typed error the caller
/// uses to fall back to the static scanner.
pub fn extract(script: &str, args: Option<&str>) -> Result<SidecarResult, SidecarError> {
    // Write the script and the harness to temp files (the harness reads the
    // script path from argv, so it must be a real file). Both guards live until
    // run_node returns, then drop-clean.
    let script_file =
        write_temp(".js", script.as_bytes()).map_err(|e| SidecarError::Io(e.to_string()))?;
    let harness_file =
        write_temp(".mjs", HARNESS_JS.as_bytes()).map_err(|e| SidecarError::Io(e.to_string()))?;

    run_node(&harness_file.path, &script_file.path, args)
}

/// Spawn `node <harness> <script> [args]` with a hard timeout, returning parsed
/// stdout. A reader thread drains stdout so a full pipe can't deadlock the
/// timeout poll.
fn run_node(harness: &Path, script: &Path, args: Option<&str>) -> Result<SidecarResult, SidecarError> {
    use std::process::Stdio;

    let mut cmd = crate::process_util::command(node_bin());
    cmd.arg(harness).arg(script);
    if let Some(a) = args {
        cmd.arg(a);
    }
    cmd.stdout(Stdio::piped()).stderr(Stdio::null()).stdin(Stdio::null());

    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Err(SidecarError::NodeNotFound),
        Err(e) => return Err(SidecarError::Io(e.to_string())),
    };

    // Drain stdout in a thread so a >64KB payload can't block node's write while
    // we're polling try_wait().
    let mut stdout = child.stdout.take().expect("piped stdout");
    let reader = std::thread::spawn(move || {
        let mut s = String::new();
        let _ = stdout.read_to_string(&mut s);
        s
    });

    let start = Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) => {
                if start.elapsed() > TIMEOUT {
                    let _ = child.kill();
                    let _ = child.wait();
                    let _ = reader.join();
                    return Err(SidecarError::Timeout);
                }
                std::thread::sleep(Duration::from_millis(25));
            }
            Err(e) => return Err(SidecarError::Io(e.to_string())),
        }
    };

    let out = reader.join().unwrap_or_default();

    if !status.success() {
        return Err(SidecarError::NonZeroExit(status.code().unwrap_or(-1)));
    }

    serde_json::from_str::<SidecarResult>(out.trim())
        .map_err(|e| SidecarError::BadOutput(format!("{e}; head={:?}", out.chars().take(80).collect::<String>())))
}

/// One process-wide lock serializing every test that reads/writes the global
/// `FLEET_NODE_BIN` env var. It MUST be shared across modules (workflow_sidecar
/// AND workflow) — separate per-module mutexes don't serialize a global, so a
/// `workflow` test setting `/bin/false` would race a `workflow_sidecar` test
/// reading `node` (and vice versa). Crate-visible so both test modules use it.
#[cfg(test)]
pub(crate) static NODE_ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[cfg(test)]
mod tests {
    use super::*;
    use super::NODE_ENV_LOCK as ENV_LOCK;

    /// Probe whether `node` is runnable in this environment; tests that need it
    /// skip (return early) when absent so CI without node still passes.
    fn node_available() -> bool {
        crate::process_util::command(node_bin())
            .arg("--version")
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
    }

    const VIZ_PROBE: &str = r#"export const meta = {
  name: 'fixture-probe',
  description: 'a fixture',
  phases: [{ title: 'Probe' }, { title: 'Synthesize' }],
}
phase('Probe')
const ASPECTS = [{ key: 'a' }, { key: 'b' }]
const findings = await parallel(
  ASPECTS.map((a) => () => agent('Investigate aspect ' + a.key, { label: 'probe:' + a.key, phase: 'Probe', agentType: 'Explore' }))
)
phase('Synthesize')
const out = await agent('Synthesize the findings: ' + findings[0], { label: 'synthesize', phase: 'Synthesize' })
return out
"#;

    #[test]
    fn extracts_static_parallel_then_single() {
        let _g = ENV_LOCK.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        if !node_available() {
            eprintln!("skipping: node not available");
            return;
        }
        let r = extract(VIZ_PROBE, None).expect("sidecar should succeed");
        assert!(r.ok, "script ran clean: {:?}", r.error);
        assert_eq!(r.name.as_deref(), Some("fixture-probe"));
        assert_eq!(r.phases.len(), 2);
        // Two lexical call-sites: the parallel probe (one site, runs 2×) + synthesize.
        assert_eq!(r.calls.len(), 2, "calls: {:?}", r.calls);

        let probe = &r.calls[0];
        assert_eq!(probe.kind, WorkflowNodeKind::Parallel);
        assert_eq!(probe.phase.as_deref(), Some("Probe"));
        assert_eq!(probe.agent_type.as_deref(), Some("Explore"));
        assert!(probe.label.as_deref().unwrap().starts_with("probe:"));
        // resolved static head ends before the first interpolation (a.key)
        assert_eq!(probe.fingerprint, "Investigate aspect ");

        let synth = &r.calls[1];
        assert_eq!(synth.kind, WorkflowNodeKind::Single);
        assert_eq!(synth.phase.as_deref(), Some("Synthesize"));
        assert_eq!(synth.label.as_deref(), Some("synthesize"));
        assert_eq!(synth.fingerprint, "Synthesize the findings: ");

        // Display prompt: full resolved template. The probe's only interpolation
        // (a.key) is literal data, so it fully resolves (no ellipsis); the synth
        // interpolates a magic agent result, so its point renders as "…".
        let probe_pr = probe
            .prompt_resolved
            .as_deref()
            .expect("probe carries a resolved prompt");
        assert!(
            probe_pr.starts_with("Investigate aspect "),
            "resolved head: {probe_pr:?}"
        );
        let synth_pr = synth
            .prompt_resolved
            .as_deref()
            .expect("synth carries a resolved prompt");
        assert!(
            synth_pr.starts_with("Synthesize the findings: "),
            "synth resolved head: {synth_pr:?}"
        );
        assert!(
            synth_pr.contains('…'),
            "magic interpolation rendered as ellipsis: {synth_pr:?}"
        );
    }

    #[test]
    fn node_not_found_is_typed() {
        let _g = ENV_LOCK.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        // An override that can't be a real binary → NodeNotFound, never a panic.
        std::env::set_var("FLEET_NODE_BIN", "/nonexistent/definitely-not-node-xyz");
        let r = extract(VIZ_PROBE, None);
        std::env::remove_var("FLEET_NODE_BIN");
        assert!(matches!(r, Err(SidecarError::NodeNotFound)), "got {r:?}");
    }
}
