//! Decoder for Antigravity CLI (`agy`) per-conversation databases
//! (`~/.gemini/antigravity-cli/conversations/<id>.db`): message bodies from the
//! `steps` table and token usage from `gen_metadata`.
//!
//! # Provenance — read before trusting any field number here
//!
//! agy publishes no schema. Every field number below comes from one of:
//!
//! - **The protobuf descriptors embedded in the agy 1.5.12 binary** (Homebrew
//!   cask `antigravity-cli` 1.5.12): `gemini_coder.Step`, the
//!   `exa.cortex_pb.CortexStep*` messages, `ChatModelMetadata`,
//!   `ModelUsageStats`, `ChatToolCall`. These carry field *names*, so the
//!   mapping is not guessed — but it is pinned to that build.
//! - **The storage layout** of the `steps` table (`step_payload` is a whole
//!   serialized `gemini_coder.Step`; only `step_format = 0` is understood; rows
//!   with `has_subtrajectory` are not) follows the offline exporter of
//!   agy-reader (MIT, Matthew Jacobs), `internal/completeness/sqlite.go`.
//! - **The usage pipeline** — the protobuf wire reader, the `#3 == #9 + #10`
//!   self-check, response-id dedup, the timestamp fallback chain and the
//!   served-model label — is ported from agent-walker
//!   (`src/collector/agy/conv.rs`, Copyright (c) 2026 michisato, dual-licensed
//!   MIT OR Apache-2.0; used here under MIT). One deliberate deviation:
//!   agent-walker reads `ModelUsageStats.#1` as system-prompt tokens and treats
//!   `#9` as answer / `#10` as thinking. The embedded descriptor says `#1` is
//!   the `Model` *enum*, `#9` is `thinking_output_tokens` and `#10`
//!   `response_output_tokens`, so this module follows the descriptor (the same
//!   reading tokscale's current Antigravity parser uses). The self-check is
//!   symmetric in `#9`/`#10`, so it is unaffected.
//!
//! **None of this has been exercised against a real, non-empty agy
//! conversation**: the only account available while writing it is refused by
//! agy's region eligibility check, so every local conversation has zero steps.
//! The tests build fixtures from the descriptor, not from captured data. Treat
//! a mismatch on a real database as a schema drift, not a Fleet bug in disguise.
//!
//! # Reading discipline
//!
//! agy writes these databases in WAL mode while a turn runs. They are opened
//! read-only — never with `immutable=1`, which reads torn pages under a live
//! writer — and a 0-byte file is never opened at all: it is still in
//! rollback-journal mode, where a reader's lock blocks agy's first write.

use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::time::{Duration, UNIX_EPOCH};

use serde_json::{json, Value};

// ── protobuf wire reader (ported from agent-walker) ──────────────────────────

enum Wire<'a> {
    Varint(u64),
    Len(&'a [u8]),
    Fixed,
}

struct ProtoReader<'a> {
    buf: &'a [u8],
    pos: usize,
}

impl<'a> ProtoReader<'a> {
    fn new(buf: &'a [u8]) -> Self {
        Self { buf, pos: 0 }
    }

    fn read_varint(&mut self) -> Option<u64> {
        let mut result = 0u64;
        let mut shift = 0u32;
        loop {
            let byte = *self.buf.get(self.pos)?;
            self.pos += 1;
            result |= u64::from(byte & 0x7f) << shift;
            if byte & 0x80 == 0 {
                return Some(result);
            }
            shift += 7;
            if shift >= 64 {
                return None;
            }
        }
    }

    fn next_field(&mut self) -> Option<(u64, Wire<'a>)> {
        if self.pos >= self.buf.len() {
            return None;
        }
        let tag = self.read_varint()?;
        let wire = match tag & 0x7 {
            0 => Wire::Varint(self.read_varint()?),
            1 => {
                self.pos = self.pos.checked_add(8).filter(|&p| p <= self.buf.len())?;
                Wire::Fixed
            }
            2 => {
                let len = usize::try_from(self.read_varint()?).ok()?;
                let end = self.pos.checked_add(len).filter(|&p| p <= self.buf.len())?;
                let bytes = &self.buf[self.pos..end];
                self.pos = end;
                Wire::Len(bytes)
            }
            5 => {
                self.pos = self.pos.checked_add(4).filter(|&p| p <= self.buf.len())?;
                Wire::Fixed
            }
            _ => return None,
        };
        Some((tag >> 3, wire))
    }
}

/// First length-delimited occurrence of `field`.
fn msg(buf: &[u8], field: u64) -> Option<&[u8]> {
    let mut r = ProtoReader::new(buf);
    while let Some((f, w)) = r.next_field() {
        if f == field {
            if let Wire::Len(b) = w {
                return Some(b);
            }
        }
    }
    None
}

/// Every length-delimited occurrence of `field` (a repeated message/string).
fn msgs(buf: &[u8], field: u64) -> Vec<&[u8]> {
    let mut out = Vec::new();
    let mut r = ProtoReader::new(buf);
    while let Some((f, w)) = r.next_field() {
        if f == field {
            if let Wire::Len(b) = w {
                out.push(b);
            }
        }
    }
    out
}

fn varint(buf: &[u8], field: u64) -> Option<u64> {
    let mut r = ProtoReader::new(buf);
    while let Some((f, w)) = r.next_field() {
        if f == field {
            if let Wire::Varint(v) = w {
                return Some(v);
            }
        }
    }
    None
}

/// A non-blank UTF-8 string field.
fn text(buf: &[u8], field: u64) -> Option<&str> {
    msg(buf, field)
        .and_then(|b| std::str::from_utf8(b).ok())
        .filter(|s| !s.trim().is_empty())
}

/// `int32` fields are sign-extended to 64 bits on the wire.
fn int32(buf: &[u8], field: u64) -> Option<i64> {
    varint(buf, field).map(|v| v as i64 as i32 as i64)
}

/// `google.protobuf.Timestamp` → ms since epoch; implausible values → `None`.
fn timestamp_ms(ts: &[u8]) -> Option<u64> {
    let seconds = varint(ts, 1)?;
    let nanos = varint(ts, 2).unwrap_or(0);
    let ms = seconds.checked_mul(1000)?.checked_add(nanos / 1_000_000)?;
    // 2000-01-01 .. 2100-01-01
    (946_684_800_000..4_102_444_800_000).contains(&ms).then_some(ms)
}

// ── field numbers (agy 1.5.12 embedded descriptors) ──────────────────────────

mod f {
    // gemini_coder.Step (#1 type is not needed: the oneof body says it)
    pub const STEP_STATUS: u64 = 4;
    pub const STEP_METADATA: u64 = 5;
    pub const STEP_ERROR: u64 = 31;
    pub const USER_INPUT: u64 = 19;
    pub const PLANNER_RESPONSE: u64 = 20;
    pub const FINISH: u64 = 12;
    pub const GREP_SEARCH: u64 = 13;
    pub const VIEW_FILE: u64 = 14;
    pub const LIST_DIRECTORY: u64 = 15;
    pub const WRITE_TO_FILE: u64 = 23;
    pub const ERROR_MESSAGE: u64 = 24;
    pub const RUN_COMMAND: u64 = 28;
    pub const FIND: u64 = 34;
    pub const COMMAND_STATUS: u64 = 37;
    pub const READ_URL_CONTENT: u64 = 40;
    pub const SEARCH_WEB: u64 = 42;
    pub const MCP_TOOL: u64 = 47;
    pub const TASK_BOUNDARY: u64 = 93;
    pub const NOTIFY_USER: u64 = 94;
    pub const FILE_CHANGE: u64 = 98;
    pub const SYSTEM_MESSAGE: u64 = 114;
    pub const GENERIC: u64 = 140;
    pub const INVOKE_SUBAGENT: u64 = 143;
    pub const ASK_QUESTION: u64 = 154;

    // CortexStepMetadata
    pub const META_CREATED_AT: u64 = 1;
    pub const META_TOOL_CALL: u64 = 4;
    pub const META_MODEL_USAGE: u64 = 9;

    // ChatToolCall
    pub const CALL_ID: u64 = 1;
    pub const CALL_NAME: u64 = 2;
    pub const CALL_ARGS: u64 = 3;

    // CortexErrorDetails
    pub const ERR_USER: u64 = 1;
    pub const ERR_SHORT: u64 = 2;
    pub const ERR_FULL: u64 = 3;

    // ModelUsageStats
    pub const USAGE_INPUT: u64 = 2;
    pub const USAGE_OUTPUT: u64 = 3;
    pub const USAGE_CACHE_WRITE: u64 = 4;
    pub const USAGE_CACHE_READ: u64 = 5;
    pub const USAGE_THINKING: u64 = 9;
    pub const USAGE_RESPONSE: u64 = 10;
    pub const USAGE_RESPONSE_ID: u64 = 11;

    // CortexStepGeneratorMetadata / ChatModelMetadata / ChatStartMetadata
    pub const GEN_CHAT_MODEL: u64 = 1;
    pub const GEN_PLANNER_CONFIG: u64 = 3;
    pub const PLANNER_MODEL_NAME: u64 = 28;
    pub const CHAT_USAGE: u64 = 4;
    pub const CHAT_START: u64 = 9;
    pub const CHAT_START_CREATED_AT: u64 = 4;
    pub const CHAT_RESPONSE_MODEL: u64 = 19;
    pub const CHAT_DISPLAY_NAME: u64 = 21;

    // CortexTrajectoryMetadata
    pub const TRAJ_CREATED_AT: u64 = 2;
}

/// CortexStepStatus values for a step that is still being produced. Such a
/// step is not rendered yet: clients dedup the tail by uuid and never replace
/// a message they already hold, so a half-generated step would freeze as-is.
const IN_FLIGHT_STATUSES: [u64; 5] = [
    8,  // GENERATING
    11, // QUEUED
    1,  // PENDING
    2,  // RUNNING
    9,  // WAITING
];
const STATUS_ERROR: u64 = 7;
const STATUS_CANCELED: u64 = 6;

/// Tool output kept per tool_result, in chars. Transport trims further.
const TOOL_OUTPUT_CAP: usize = 50_000;

// ── opening ──────────────────────────────────────────────────────────────────

/// Open a conversation database read-only. `Ok(None)` for a missing or 0-byte
/// file (see the module docs).
pub fn open_db(db: &Path) -> Result<Option<rusqlite::Connection>, String> {
    match std::fs::metadata(db) {
        Ok(m) if m.len() > 0 => {}
        _ => return Ok(None),
    }
    let conn = rusqlite::Connection::open_with_flags(
        db,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .map_err(|e| format!("open {}: {e}", db.display()))?;
    let _ = conn.busy_timeout(Duration::from_millis(250));
    Ok(Some(conn))
}

fn has_table(conn: &rusqlite::Connection, name: &str) -> bool {
    conn.query_row(
        "SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = ?1",
        [name],
        |_| Ok(()),
    )
    .is_ok()
}

// ── steps ────────────────────────────────────────────────────────────────────

/// One `steps` row this decoder understands.
#[derive(Clone, Debug, Default)]
pub struct StepRow {
    pub idx: i64,
    pub status: u64,
    /// The serialized `gemini_coder.Step`.
    pub payload: Vec<u8>,
    /// The `metadata` column (a copy of `Step.metadata`), used when the
    /// payload lacks it.
    pub metadata: Vec<u8>,
}

/// Steps read from one database.
#[derive(Clone, Debug, Default)]
pub struct Steps {
    pub rows: Vec<StepRow>,
    /// Rows skipped because their storage shape is not understood
    /// (`step_format != 0`, `has_subtrajectory`, or no payload).
    pub skipped: usize,
}

pub fn read_steps(conn: &rusqlite::Connection) -> Result<Steps, String> {
    let mut out = Steps::default();
    if !has_table(conn, "steps") {
        return Ok(out);
    }
    let mut stmt = conn
        .prepare(
            "SELECT idx, status, has_subtrajectory, step_format, step_payload, metadata \
             FROM steps ORDER BY idx",
        )
        .map_err(|e| format!("query steps: {e}"))?;
    let rows = stmt
        .query_map([], |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, Option<i64>>(1)?.unwrap_or(0),
                r.get::<_, Option<i64>>(2)?.unwrap_or(0) != 0,
                r.get::<_, Option<i64>>(3)?.unwrap_or(0),
                r.get::<_, Option<Vec<u8>>>(4)?.unwrap_or_default(),
                r.get::<_, Option<Vec<u8>>>(5)?.unwrap_or_default(),
            ))
        })
        .map_err(|e| format!("read steps: {e}"))?;
    for row in rows {
        let Ok((idx, status, sub, format, payload, metadata)) = row else {
            out.skipped += 1;
            continue;
        };
        if sub || format != 0 || payload.is_empty() {
            out.skipped += 1;
            continue;
        }
        out.rows.push(StepRow {
            idx,
            status: status.max(0) as u64,
            payload,
            metadata,
        });
    }
    Ok(out)
}

fn iso(ms: u64) -> String {
    chrono::DateTime::from_timestamp_millis(ms as i64)
        .map(|t| t.to_rfc3339())
        .unwrap_or_default()
}

fn cap(s: &str) -> String {
    if s.chars().count() <= TOOL_OUTPUT_CAP {
        return s.to_string();
    }
    let mut t: String = s.chars().take(TOOL_OUTPUT_CAP).collect();
    t.push_str("\n… [truncated]");
    t
}

fn error_details_text(err: &[u8]) -> Option<String> {
    text(err, f::ERR_USER)
        .or_else(|| text(err, f::ERR_SHORT))
        .or_else(|| text(err, f::ERR_FULL))
        .map(str::to_string)
}

/// A tool step: its display name, the arguments shown when agy's own tool call
/// record is missing, and the output text.
struct ToolStep {
    name: &'static str,
    input: Value,
    output: String,
}

fn join_nonempty(parts: &[Option<&str>]) -> String {
    parts
        .iter()
        .flatten()
        .map(|s| s.trim_end())
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join("\n")
}

fn tool_step(step: &[u8]) -> Option<ToolStep> {
    if let Some(m) = msg(step, f::RUN_COMMAND) {
        let combined = msg(m, 21).and_then(|o| text(o, 1).or_else(|| text(o, 2)));
        let mut output = match combined {
            Some(c) => c.to_string(),
            None => join_nonempty(&[text(m, 4), text(m, 5)]),
        };
        if let Some(code) = int32(m, 6) {
            output.push_str(&format!("\n[exit code {code}]"));
        }
        return Some(ToolStep {
            name: "run_command",
            input: json!({ "command": text(m, 23).unwrap_or(""), "cwd": text(m, 2) }),
            output,
        });
    }
    if let Some(m) = msg(step, f::COMMAND_STATUS) {
        let mut output = text(m, 9)
            .map(str::to_string)
            .unwrap_or_else(|| join_nonempty(&[text(m, 3), text(m, 4)]));
        if let Some(code) = int32(m, 5) {
            output.push_str(&format!("\n[exit code {code}]"));
        }
        return Some(ToolStep {
            name: "command_status",
            input: json!({ "command_id": text(m, 1) }),
            output,
        });
    }
    if let Some(m) = msg(step, f::VIEW_FILE) {
        return Some(ToolStep {
            name: "view_file",
            input: json!({ "path": text(m, 1), "start_line": varint(m, 2), "end_line": varint(m, 3) }),
            output: text(m, 4).or_else(|| text(m, 9)).unwrap_or("").to_string(),
        });
    }
    if let Some(m) = msg(step, f::WRITE_TO_FILE) {
        let code: Vec<&str> = msgs(m, 2)
            .into_iter()
            .filter_map(|b| std::str::from_utf8(b).ok())
            .collect();
        return Some(ToolStep {
            name: "write_to_file",
            input: json!({ "path": text(m, 1), "content": code.join("\n") }),
            output: if varint(m, 4) == Some(1) { "file created" } else { "file written" }.to_string(),
        });
    }
    if let Some(m) = msg(step, f::FILE_CHANGE) {
        return Some(ToolStep {
            name: "file_change",
            input: json!({ "path": text(m, 1), "instruction": text(m, 5) }),
            output: String::new(),
        });
    }
    if let Some(m) = msg(step, f::GREP_SEARCH) {
        return Some(ToolStep {
            name: "grep_search",
            input: json!({ "query": text(m, 1), "path": text(m, 11) }),
            output: text(m, 3).or_else(|| text(m, 5)).unwrap_or("").to_string(),
        });
    }
    if let Some(m) = msg(step, f::FIND) {
        return Some(ToolStep {
            name: "find",
            input: json!({ "pattern": text(m, 1), "directory": text(m, 10) }),
            output: text(m, 11)
                .or_else(|| text(m, 14))
                .or_else(|| text(m, 8))
                .unwrap_or("")
                .to_string(),
        });
    }
    if let Some(m) = msg(step, f::LIST_DIRECTORY) {
        let children: Vec<&str> = msgs(m, 2)
            .into_iter()
            .filter_map(|b| std::str::from_utf8(b).ok())
            .collect();
        return Some(ToolStep {
            name: "list_directory",
            input: json!({ "path": text(m, 1) }),
            output: children.join("\n"),
        });
    }
    if let Some(m) = msg(step, f::SEARCH_WEB) {
        return Some(ToolStep {
            name: "search_web",
            input: json!({ "query": text(m, 1), "domain": text(m, 3) }),
            output: text(m, 5).unwrap_or("").to_string(),
        });
    }
    if let Some(m) = msg(step, f::READ_URL_CONTENT) {
        return Some(ToolStep {
            name: "read_url_content",
            input: json!({ "url": text(m, 1) }),
            output: text(m, 3).unwrap_or("").to_string(),
        });
    }
    if let Some(m) = msg(step, f::MCP_TOOL) {
        let call = msg(m, 2);
        let args = call
            .and_then(|c| text(c, f::CALL_ARGS))
            .and_then(|a| serde_json::from_str::<Value>(a).ok())
            .unwrap_or(Value::Null);
        return Some(ToolStep {
            name: "mcp_tool",
            input: json!({
                "server": text(m, 1),
                "tool": call.and_then(|c| text(c, f::CALL_NAME)),
                "arguments": args,
            }),
            output: text(m, 3).or_else(|| text(m, 8)).unwrap_or("").to_string(),
        });
    }
    if let Some(m) = msg(step, f::INVOKE_SUBAGENT) {
        return Some(ToolStep {
            name: "invoke_subagent",
            input: json!({ "subagent": text(m, 1), "prompt": text(m, 2) }),
            output: text(m, 5)
                .map(|c| format!("subagent conversation {c}"))
                .unwrap_or_default(),
        });
    }
    if msg(step, f::GENERIC).is_some() {
        return Some(ToolStep {
            name: "generic",
            input: json!({}),
            output: String::new(),
        });
    }
    if msg(step, f::ASK_QUESTION).is_some() {
        return Some(ToolStep {
            name: "ask_question",
            input: json!({}),
            output: String::new(),
        });
    }
    None
}

fn user_text(m: &[u8]) -> Option<String> {
    if let Some(q) = text(m, 1) {
        return Some(q.to_string());
    }
    let items: Vec<&str> = msgs(m, 3).into_iter().filter_map(|i| text(i, 1)).collect();
    if !items.is_empty() {
        return Some(items.join(""));
    }
    text(m, 2).map(str::to_string)
}

fn assistant(uuid: String, ts: &str, content: Vec<Value>) -> Value {
    json!({
        "type": "assistant",
        "uuid": uuid,
        "timestamp": ts,
        "message": { "role": "assistant", "content": content },
    })
}

/// Turn decoded steps into Claude-shaped transcript records (`type` user /
/// assistant, `message.content` blocks), which every Fleet client renders.
///
/// uuids are `agy:<conv>:step:<idx>[:…]` — stable across re-reads, which the
/// clients' tail dedup relies on. Decoding stops at the first step that is
/// still being produced (other than a user input): rendering it now would
/// freeze a half-written message, and skipping just it would reorder the
/// transcript once it lands.
pub fn decode_steps(conv: &str, steps: &[StepRow]) -> Vec<Value> {
    let mut out = Vec::new();
    // Tool-call ids an assistant message already announced as `tool_use`.
    let mut announced: HashSet<String> = HashSet::new();
    for row in steps {
        let step = row.payload.as_slice();
        let status = varint(step, f::STEP_STATUS).unwrap_or(row.status);
        let meta = msg(step, f::STEP_METADATA).unwrap_or(row.metadata.as_slice());
        let ts = msg(meta, f::META_CREATED_AT)
            .and_then(timestamp_ms)
            .map(iso)
            .unwrap_or_default();
        let base = format!("agy:{conv}:step:{}", row.idx);

        if let Some(m) = msg(step, f::USER_INPUT) {
            if let Some(t) = user_text(m) {
                out.push(json!({
                    "type": "user",
                    "uuid": base,
                    "timestamp": ts,
                    "message": { "role": "user", "content": t },
                }));
            }
            continue;
        }
        if IN_FLIGHT_STATUSES.contains(&status) {
            break;
        }

        if let Some(m) = msg(step, f::PLANNER_RESPONSE) {
            let mut content = Vec::new();
            if let Some(t) = text(m, 3) {
                content.push(json!({ "type": "thinking", "thinking": t }));
            }
            if let Some(t) = text(m, 8).or_else(|| text(m, 1)) {
                content.push(json!({ "type": "text", "text": t }));
            }
            for (i, call) in msgs(m, 7).into_iter().enumerate() {
                let id = text(call, f::CALL_ID)
                    .map(str::to_string)
                    .unwrap_or_else(|| format!("{base}:call:{i}"));
                announced.insert(id.clone());
                content.push(tool_use_block(call, &id, "tool"));
            }
            if !content.is_empty() {
                out.push(assistant(base, &ts, content));
            }
            continue;
        }

        if let Some(tool) = tool_step(step) {
            let call = msg(meta, f::META_TOOL_CALL);
            let id = call
                .and_then(|c| text(c, f::CALL_ID))
                .map(str::to_string)
                .unwrap_or_else(|| format!("{base}:call"));
            if !announced.contains(&id) {
                // No planner step announced this call: synthesize the tool_use
                // so the result has something to attach to.
                let block = match call {
                    Some(c) => tool_use_block(c, &id, tool.name),
                    None => json!({ "type": "tool_use", "id": id, "name": tool.name, "input": tool.input }),
                };
                out.push(assistant(format!("{base}:call"), &ts, vec![block]));
                announced.insert(id.clone());
            }
            let failed = status == STATUS_ERROR || status == STATUS_CANCELED;
            let mut output = tool.output;
            if let Some(e) = msg(step, f::STEP_ERROR).and_then(error_details_text) {
                output = join_nonempty(&[Some(&output), Some(&e)]);
            } else if status == STATUS_CANCELED && output.is_empty() {
                output = "canceled".to_string();
            }
            out.push(json!({
                "type": "user",
                "uuid": base,
                "timestamp": ts,
                "message": {
                    "role": "user",
                    "content": [{
                        "type": "tool_result",
                        "tool_use_id": id,
                        "content": cap(&output),
                        "is_error": failed,
                    }],
                },
            }));
            continue;
        }

        // Plain assistant-visible text.
        let said = msg(step, f::NOTIFY_USER)
            .and_then(|m| text(m, 2))
            .map(str::to_string)
            .or_else(|| msg(step, f::FINISH).and_then(|m| text(m, 2)).map(str::to_string))
            .or_else(|| {
                msg(step, f::TASK_BOUNDARY).and_then(|m| {
                    let s = join_nonempty(&[text(m, 1), text(m, 3)]);
                    (!s.is_empty()).then_some(s)
                })
            })
            .or_else(|| {
                msg(step, f::ERROR_MESSAGE)
                    .and_then(|m| msg(m, 3))
                    .and_then(error_details_text)
                    .map(|e| format!("Error: {e}"))
            })
            .or_else(|| {
                msg(step, f::SYSTEM_MESSAGE)
                    .and_then(|m| text(m, 1))
                    .map(str::to_string)
            });
        // Ephemeral messages (Step #103) are reminders agy injects for the
        // model; they are deliberately not shown.
        let said = said.or_else(|| {
            (status == STATUS_ERROR)
                .then(|| msg(step, f::STEP_ERROR).and_then(error_details_text))
                .flatten()
                .map(|e| format!("Error: {e}"))
        });
        if let Some(t) = said {
            out.push(assistant(base, &ts, vec![json!({ "type": "text", "text": t })]));
        }
    }
    out
}

fn tool_use_block(call: &[u8], id: &str, fallback_name: &str) -> Value {
    let name = text(call, f::CALL_NAME).unwrap_or(fallback_name);
    let input = text(call, f::CALL_ARGS)
        .map(|a| serde_json::from_str::<Value>(a).unwrap_or_else(|_| json!({ "raw": a })))
        .unwrap_or_else(|| json!({}));
    json!({ "type": "tool_use", "id": id, "name": name, "input": input })
}

// ── gen_metadata (usage) ─────────────────────────────────────────────────────

/// One model generation's usage.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Generation {
    /// Dedup key: the response id, else `<conv>#<idx>`.
    pub key: String,
    pub timestamp_ms: u64,
    pub model: Option<String>,
    /// Uncached input (`ModelUsageStats.input_tokens`).
    pub input_tokens: u64,
    /// Total output, thinking included (`output_tokens`).
    pub output_tokens: u64,
    pub thinking_tokens: u64,
    pub cache_write_tokens: u64,
    pub cache_read_tokens: u64,
}

/// Generations read from one database.
#[derive(Clone, Debug, Default)]
pub struct Generations {
    pub rows: Vec<Generation>,
    /// Rows dropped by the `#3 == #9 + #10` self-check (layout drift).
    pub drifted: usize,
}

impl Generations {
    pub fn totals(&self) -> (u64, u64) {
        self.rows.iter().fold((0, 0), |(i, o), g| {
            (
                i.saturating_add(g.input_tokens)
                    .saturating_add(g.cache_read_tokens)
                    .saturating_add(g.cache_write_tokens),
                o.saturating_add(g.output_tokens),
            )
        })
    }

    /// The model of the most recent generation.
    pub fn last_model(&self) -> Option<&str> {
        self.rows
            .iter()
            .max_by_key(|g| g.timestamp_ms)
            .and_then(|g| g.model.as_deref())
    }
}

enum ParseGen {
    Row(Generation),
    Empty,
    Drift,
}

/// `fallback_ms` dates a row that carries no time of its own and matches no
/// step (agent-walker's chain: row → step → conversation start → file mtime;
/// the caller passes the file mtime).
pub fn read_generations(
    conn: &rusqlite::Connection,
    conv: &str,
    fallback_ms: u64,
) -> Result<Generations, String> {
    let mut out = Generations::default();
    if !has_table(conn, "gen_metadata") {
        return Ok(out);
    }
    let blobs: Vec<(i64, Vec<u8>)> = {
        let mut stmt = conn
            .prepare("SELECT idx, data FROM gen_metadata ORDER BY idx")
            .map_err(|e| format!("query gen_metadata: {e}"))?;
        let rows = stmt
            .query_map([], |r| {
                Ok((r.get::<_, i64>(0)?, r.get::<_, Option<Vec<u8>>>(1)?.unwrap_or_default()))
            })
            .map_err(|e| format!("read gen_metadata: {e}"))?;
        rows.filter_map(Result::ok).collect()
    };
    let fallback = conversation_start_ms(conn).unwrap_or(fallback_ms);
    let step_times = step_times_by_response(conn);
    let selected = selected_models(&blobs);
    let mut seen: HashSet<String> = HashSet::new();
    for (idx, blob) in &blobs {
        match parse_gen(blob, *idx, conv, fallback, &step_times, &selected) {
            ParseGen::Row(g) => {
                if seen.insert(g.key.clone()) {
                    out.rows.push(g);
                }
            }
            ParseGen::Empty => {}
            ParseGen::Drift => out.drifted += 1,
        }
    }
    Ok(out)
}

fn parse_gen(
    blob: &[u8],
    idx: i64,
    conv: &str,
    fallback_ms: u64,
    step_times: &HashMap<String, u64>,
    selected: &[String],
) -> ParseGen {
    let Some(chat) = msg(blob, f::GEN_CHAT_MODEL) else {
        return ParseGen::Empty;
    };
    let Some(usage) = msg(chat, f::CHAT_USAGE) else {
        return ParseGen::Empty;
    };
    let v = |field| varint(usage, field).unwrap_or(0).min(i64::MAX as u64);
    let input = v(f::USAGE_INPUT);
    let cache_write = v(f::USAGE_CACHE_WRITE);
    let cache_read = v(f::USAGE_CACHE_READ);
    let thinking = v(f::USAGE_THINKING);
    let response = v(f::USAGE_RESPONSE);
    let output = match varint(usage, f::USAGE_OUTPUT) {
        Some(total) if total.min(i64::MAX as u64) == thinking.saturating_add(response) => {
            thinking.saturating_add(response)
        }
        Some(_) => return ParseGen::Drift,
        None => return ParseGen::Empty,
    };
    if input == 0 && output == 0 && cache_read == 0 && cache_write == 0 {
        return ParseGen::Empty;
    }
    let response_id = text(usage, f::USAGE_RESPONSE_ID);
    let key = response_id
        .map(str::to_string)
        .unwrap_or_else(|| format!("{conv}#{idx}"));
    let timestamp_ms = msg(chat, f::CHAT_START)
        .and_then(|s| msg(s, f::CHAT_START_CREATED_AT))
        .and_then(timestamp_ms)
        .or_else(|| response_id.and_then(|id| step_times.get(id).copied()))
        .unwrap_or(fallback_ms);
    let model = text(chat, f::CHAT_DISPLAY_NAME)
        .map(str::to_string)
        .or_else(|| {
            text(chat, f::CHAT_RESPONSE_MODEL).map(|s| served_model_label(s, selected).to_string())
        });
    ParseGen::Row(Generation {
        key,
        timestamp_ms,
        model,
        input_tokens: input,
        output_tokens: output,
        thinking_tokens: thinking,
        cache_write_tokens: cache_write,
        cache_read_tokens: cache_read,
    })
}

/// The selected model minus its last segment (the mode: `-high`, …), taken
/// only when the served id is that base or the base plus one segment.
/// Ported from agent-walker.
fn served_model_label<'a>(served: &'a str, selected: &[String]) -> &'a str {
    let selection_base = |base: &str| selected.iter().any(|id| parent_id(id) == Some(base));
    if selection_base(served) {
        return served;
    }
    match parent_id(served) {
        Some(base) if selection_base(base) => base,
        _ => served,
    }
}

fn parent_id(id: &str) -> Option<&str> {
    id.rsplit_once('-').map(|(head, _)| head)
}

fn selected_models(blobs: &[(i64, Vec<u8>)]) -> Vec<String> {
    let mut models: Vec<String> = blobs
        .iter()
        .filter_map(|(_, b)| msg(b, f::GEN_PLANNER_CONFIG))
        .filter_map(|c| text(c, f::PLANNER_MODEL_NAME))
        .map(str::to_string)
        .collect();
    models.sort_unstable();
    models.dedup();
    models
}

/// Keyed by response id: some conversations hold more planner steps than gen
/// rows, so the n-th step is not the n-th row.
fn step_times_by_response(conn: &rusqlite::Connection) -> HashMap<String, u64> {
    let mut times = HashMap::new();
    if !has_table(conn, "steps") {
        return times;
    }
    let Ok(mut stmt) = conn.prepare("SELECT metadata FROM steps ORDER BY idx") else {
        return times;
    };
    let Ok(rows) = stmt.query_map([], |r| r.get::<_, Option<Vec<u8>>>(0)) else {
        return times;
    };
    for meta in rows.flatten().flatten() {
        let Some(id) = msg(&meta, f::META_MODEL_USAGE).and_then(|u| text(u, f::USAGE_RESPONSE_ID))
        else {
            continue;
        };
        let Some(ms) = msg(&meta, f::META_CREATED_AT).and_then(timestamp_ms) else {
            continue;
        };
        times.entry(id.to_string()).or_insert(ms);
    }
    times
}

fn conversation_start_ms(conn: &rusqlite::Connection) -> Option<u64> {
    if !has_table(conn, "trajectory_metadata_blob") {
        return None;
    }
    let blob: Vec<u8> = conn
        .query_row("SELECT data FROM trajectory_metadata_blob LIMIT 1", [], |r| r.get(0))
        .ok()?;
    msg(&blob, f::TRAJ_CREATED_AT).and_then(timestamp_ms)
}

// ── one-shot conversation read ───────────────────────────────────────────────

/// Everything Fleet reads from one conversation database.
#[derive(Clone, Debug, Default)]
pub struct Conversation {
    pub messages: Vec<Value>,
    pub steps_seen: usize,
    pub steps_skipped: usize,
    pub generations: Generations,
}

pub fn read_conversation(db: &Path, conv: &str) -> Result<Conversation, String> {
    let Some(conn) = open_db(db)? else {
        return Ok(Conversation::default());
    };
    let steps = read_steps(&conn)?;
    let fallback = newest_mtime_ms(db).unwrap_or(0);
    let generations = read_generations(&conn, conv, fallback)?;
    Ok(Conversation {
        messages: decode_steps(conv, &steps.rows),
        steps_seen: steps.rows.len(),
        steps_skipped: steps.skipped,
        generations,
    })
}

/// Newest mtime across the database and its WAL sidecars, ms.
pub fn newest_mtime_ms(db: &Path) -> Option<u64> {
    ["", "-wal", "-shm"]
        .iter()
        .filter_map(|suffix| {
            let mut name = db.as_os_str().to_owned();
            name.push(suffix);
            let t = std::fs::metadata(Path::new(&name)).ok()?.modified().ok()?;
            Some(t.duration_since(UNIX_EPOCH).ok()?.as_millis() as u64)
        })
        .max()
}

#[cfg(test)]
pub(crate) mod fixture {
    //! Protobuf encoders for tests. Fixtures are built from the field numbers
    //! in the embedded descriptor — they are not captured agy data.

    pub fn enc(mut v: u64) -> Vec<u8> {
        let mut out = Vec::new();
        loop {
            let mut b = (v & 0x7f) as u8;
            v >>= 7;
            if v != 0 {
                b |= 0x80;
            }
            out.push(b);
            if v == 0 {
                return out;
            }
        }
    }
    pub fn vf(field: u64, value: u64) -> Vec<u8> {
        let mut out = enc(field << 3);
        out.extend(enc(value));
        out
    }
    pub fn lf(field: u64, payload: &[u8]) -> Vec<u8> {
        let mut out = enc((field << 3) | 2);
        out.extend(enc(payload.len() as u64));
        out.extend_from_slice(payload);
        out
    }
    pub fn sf(field: u64, s: &str) -> Vec<u8> {
        lf(field, s.as_bytes())
    }
    pub fn ts(field: u64, ms: u64) -> Vec<u8> {
        let mut t = vf(1, ms / 1000);
        t.extend(vf(2, (ms % 1000) * 1_000_000));
        lf(field, &t)
    }
    pub fn cat(parts: &[Vec<u8>]) -> Vec<u8> {
        parts.concat()
    }

    /// A `steps` / `gen_metadata` database with agy 1.5.12's schema.
    pub fn conversation_db(
        path: &std::path::Path,
        steps: &[(i64, u64, Vec<u8>)],
        gens: &[Vec<u8>],
    ) {
        let conn = rusqlite::Connection::open(path).unwrap();
        conn.execute_batch(
            "PRAGMA journal_mode=WAL;
             CREATE TABLE `steps` (`idx` integer,`step_type` integer NOT NULL DEFAULT 0,`status` integer NOT NULL DEFAULT 0,`has_subtrajectory` numeric NOT NULL DEFAULT false,`metadata` blob,`error_details` blob,`permissions` blob,`task_details` blob,`render_info` blob,`step_payload` blob,`step_format` integer NOT NULL DEFAULT 0,PRIMARY KEY (`idx`));
             CREATE TABLE `gen_metadata` (`idx` integer,`data` blob,`size` integer NOT NULL DEFAULT 0,PRIMARY KEY (`idx`));
             CREATE TABLE `trajectory_metadata_blob` (`id` text DEFAULT \"main\",`data` blob,PRIMARY KEY (`id`));",
        )
        .unwrap();
        for (idx, status, payload) in steps {
            conn.execute(
                "INSERT INTO steps (idx, status, step_payload) VALUES (?1, ?2, ?3)",
                rusqlite::params![idx, *status as i64, payload],
            )
            .unwrap();
        }
        for (i, g) in gens.iter().enumerate() {
            conn.execute(
                "INSERT INTO gen_metadata (idx, data, size) VALUES (?1, ?2, ?3)",
                rusqlite::params![i as i64, g, g.len() as i64],
            )
            .unwrap();
        }
    }

    /// A `gemini_coder.Step`.
    pub fn step(status: u64, created_ms: u64, body_field: u64, body: &[u8], extra_meta: &[u8]) -> Vec<u8> {
        let mut meta = ts(1, created_ms);
        meta.extend_from_slice(extra_meta);
        cat(&[vf(4, status), lf(5, &meta), lf(body_field, body)])
    }

    /// A `ChatToolCall`.
    pub fn tool_call(id: &str, name: &str, args: &str) -> Vec<u8> {
        cat(&[sf(1, id), sf(2, name), sf(3, args)])
    }

    /// A gen_metadata row.
    pub fn generation(
        model_display: Option<&str>,
        served: Option<&str>,
        input: u64,
        thinking: u64,
        response: u64,
        total_out: u64,
        cache_read: u64,
        response_id: &str,
        created_ms: Option<u64>,
    ) -> Vec<u8> {
        let usage = cat(&[
            vf(1, 1017), // Model enum — must NOT count as tokens
            vf(2, input),
            vf(3, total_out),
            vf(5, cache_read),
            vf(9, thinking),
            vf(10, response),
            sf(11, response_id),
        ]);
        let mut chat = lf(4, &usage);
        if let Some(ms) = created_ms {
            chat.extend(lf(9, &ts(4, ms)));
        }
        if let Some(m) = served {
            chat.extend(sf(19, m));
        }
        if let Some(m) = model_display {
            chat.extend(sf(21, m));
        }
        lf(1, &chat)
    }
}

#[cfg(test)]
mod tests {
    use super::fixture::*;
    use super::*;

    const DONE: u64 = 3;

    #[test]
    fn decodes_a_turn_with_thinking_tool_call_and_result() {
        let tmp = tempfile::tempdir().unwrap();
        let db = tmp.path().join("c1.db");
        let user = step(DONE, 1_795_000_000_000, 19, &sf(1, "list files"), &[]);
        let planner = step(
            DONE,
            1_795_000_001_000,
            20,
            &cat(&[
                sf(3, "I should run ls"),
                sf(1, "Listing."),
                lf(7, &tool_call("call-1", "run_command", r#"{"CommandLine":"ls"}"#)),
            ]),
            &[],
        );
        let run = step(
            DONE,
            1_795_000_002_000,
            28,
            &cat(&[sf(23, "ls"), lf(21, &sf(1, "a.txt\nb.txt")), vf(6, 0)]),
            &lf(4, &tool_call("call-1", "run_command", "{}")),
        );
        let reply = step(DONE, 1_795_000_003_000, 20, &sf(1, "Two files."), &[]);
        conversation_db(
            &db,
            &[(0, DONE, user), (1, DONE, planner), (2, DONE, run), (3, DONE, reply)],
            &[],
        );
        let conv = read_conversation(&db, "c1").unwrap();
        let m = &conv.messages;
        assert_eq!(m.len(), 4, "{m:#?}");
        assert_eq!(m[0]["type"], "user");
        assert_eq!(m[0]["message"]["content"], "list files");
        assert_eq!(m[0]["uuid"], "agy:c1:step:0");
        assert!(m[0]["timestamp"].as_str().unwrap().starts_with("2026-"));
        let blocks = m[1]["message"]["content"].as_array().unwrap();
        assert_eq!(blocks[0]["type"], "thinking");
        assert_eq!(blocks[1]["text"], "Listing.");
        assert_eq!(blocks[2]["type"], "tool_use");
        assert_eq!(blocks[2]["id"], "call-1");
        assert_eq!(blocks[2]["input"]["CommandLine"], "ls");
        let result = &m[2]["message"]["content"][0];
        assert_eq!(result["type"], "tool_result");
        assert_eq!(result["tool_use_id"], "call-1");
        assert!(result["content"].as_str().unwrap().contains("b.txt"));
        assert!(result["content"].as_str().unwrap().contains("[exit code 0]"));
        assert_eq!(result["is_error"], false);
        assert_eq!(m[3]["message"]["content"][0]["text"], "Two files.");
    }

    #[test]
    fn a_tool_step_without_an_announcing_planner_gets_a_synthetic_tool_use() {
        let run = step(
            7, // ERROR
            1_795_000_000_000,
            28,
            &cat(&[sf(23, "false"), vf(6, 1)]),
            &[],
        );
        let mut with_err = run.clone();
        with_err.extend(lf(31, &sf(1, "command failed")));
        let msgs = decode_steps("c", &[StepRow { idx: 5, status: 7, payload: with_err, ..Default::default() }]);
        assert_eq!(msgs.len(), 2);
        assert_eq!(msgs[0]["message"]["content"][0]["type"], "tool_use");
        assert_eq!(msgs[0]["message"]["content"][0]["name"], "run_command");
        assert_eq!(msgs[0]["message"]["content"][0]["input"]["command"], "false");
        let r = &msgs[1]["message"]["content"][0];
        assert_eq!(r["tool_use_id"], "agy:c:step:5:call");
        assert_eq!(r["is_error"], true);
        assert!(r["content"].as_str().unwrap().contains("command failed"));
    }

    #[test]
    fn decoding_stops_at_the_first_step_still_being_generated() {
        let user = step(DONE, 1, 19, &sf(1, "hi"), &[]);
        let generating = step(8, 2, 20, &sf(1, "Hel"), &[]);
        let later_user = step(DONE, 3, 19, &sf(1, "again"), &[]);
        let rows: Vec<StepRow> = [user, generating, later_user]
            .into_iter()
            .enumerate()
            .map(|(i, p)| StepRow { idx: i as i64, payload: p, ..Default::default() })
            .collect();
        let msgs = decode_steps("c", &rows);
        assert_eq!(msgs.len(), 1, "the half-written reply and everything after wait");
        assert_eq!(msgs[0]["message"]["content"], "hi");
    }

    #[test]
    fn user_input_falls_back_to_items() {
        let body = cat(&[lf(3, &sf(1, "from ")), lf(3, &sf(1, "items")), sf(19, "client-msg")]);
        let msgs = decode_steps(
            "c",
            &[StepRow { idx: 0, payload: step(DONE, 1, 19, &body, &[]), ..Default::default() }],
        );
        assert_eq!(msgs[0]["message"]["content"], "from items");
    }

    #[test]
    fn unsupported_storage_shapes_are_skipped_and_counted() {
        let tmp = tempfile::tempdir().unwrap();
        let db = tmp.path().join("c.db");
        conversation_db(&db, &[(0, DONE, step(DONE, 1, 19, &sf(1, "ok"), &[]))], &[]);
        let conn = rusqlite::Connection::open(&db).unwrap();
        conn.execute(
            "INSERT INTO steps (idx, status, step_format, step_payload) VALUES (1, 3, 2, x'00')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO steps (idx, status, has_subtrajectory, step_payload) VALUES (2, 3, 1, x'00')",
            [],
        )
        .unwrap();
        drop(conn);
        let conv = read_conversation(&db, "c").unwrap();
        assert_eq!(conv.steps_seen, 1);
        assert_eq!(conv.steps_skipped, 2);
    }

    #[test]
    fn empty_and_missing_databases_are_never_opened() {
        let tmp = tempfile::tempdir().unwrap();
        let empty = tmp.path().join("e.db");
        std::fs::write(&empty, b"").unwrap();
        assert!(open_db(&empty).unwrap().is_none());
        assert_eq!(std::fs::metadata(&empty).unwrap().len(), 0);
        assert!(open_db(&tmp.path().join("missing.db")).unwrap().is_none());
        let conv = read_conversation(&empty, "e").unwrap();
        assert!(conv.messages.is_empty());
    }

    #[test]
    fn usage_follows_the_embedded_descriptor() {
        let tmp = tempfile::tempdir().unwrap();
        let db = tmp.path().join("u.db");
        let gens = vec![
            generation(Some("Gemini 4 Neon (High)"), None, 1000, 30, 70, 100, 500, "resp-1", Some(1_795_000_000_000)),
            // Retried generation: same response id → counted once.
            generation(Some("Gemini 4 Neon (High)"), None, 1000, 30, 70, 100, 500, "resp-1", Some(1_795_000_000_000)),
            // Drifted layout: #3 != #9 + #10.
            generation(None, Some("gemini-4-neon"), 10, 1, 1, 5, 0, "resp-2", None),
            // No time of its own → file mtime fallback.
            generation(None, Some("gemini-4-carbon-x"), 200, 0, 20, 20, 0, "resp-3", None),
        ];
        conversation_db(&db, &[], &gens);
        let conv = read_conversation(&db, "u").unwrap();
        let g = &conv.generations;
        assert_eq!(g.rows.len(), 2);
        assert_eq!(g.drifted, 1);
        let first = &g.rows[0];
        assert_eq!(first.input_tokens, 1000, "#1 is the Model enum, not tokens");
        assert_eq!(first.output_tokens, 100);
        assert_eq!(first.thinking_tokens, 30, "#9 is thinking per the descriptor");
        assert_eq!(first.cache_read_tokens, 500);
        assert_eq!(first.timestamp_ms, 1_795_000_000_000);
        assert_eq!(first.model.as_deref(), Some("Gemini 4 Neon (High)"));
        assert!(g.rows[1].timestamp_ms > 1_700_000_000_000, "falls back to the file mtime");
        assert_eq!(g.totals(), (1000 + 500 + 200, 120));
    }

    #[test]
    fn served_model_label_drops_routing_tags_against_the_selection() {
        let sel = vec!["gemini-3.8-flash-high".to_string()];
        assert_eq!(served_model_label("gemini-3.8-flash-n", &sel), "gemini-3.8-flash");
        assert_eq!(served_model_label("gemini-3.8-flash", &sel), "gemini-3.8-flash");
        assert_eq!(served_model_label("claude-x-y", &sel), "claude-x-y");
    }

    #[test]
    fn negative_int32_exit_codes_decode() {
        // int32 -1 is a 10-byte varint on the wire.
        let body = cat(&[sf(23, "x"), vf(6, u64::MAX)]);
        let msgs = decode_steps("c", &[StepRow { idx: 0, payload: step(DONE, 1, 28, &body, &[]), ..Default::default() }]);
        assert!(msgs[1]["message"]["content"][0]["content"]
            .as_str()
            .unwrap()
            .contains("[exit code -1]"));
    }

    #[test]
    fn garbage_payloads_do_not_panic() {
        let rows: Vec<StepRow> = [vec![0xff; 16], vec![0x0a, 0xff, 0xff], vec![]]
            .into_iter()
            .enumerate()
            .map(|(i, p)| StepRow { idx: i as i64, payload: p, ..Default::default() })
            .collect();
        let _ = decode_steps("c", &rows);
        assert!(matches!(
            parse_gen(&[0x0a, 0x05, 0x22], 0, "c", 0, &HashMap::new(), &[]),
            ParseGen::Empty
        ));
    }
}
