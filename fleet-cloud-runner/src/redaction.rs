use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use serde::Serialize;
use serde_json::Value;

pub const MAX_RECORD_BYTES: usize = 8 * 1024 * 1024;
pub const MAX_PROCESSING_TIME: Duration = Duration::from_millis(50);

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RedactionError {
    RecordTooLarge,
    ProcessingTimedOut,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct RedactedRecord {
    pub record: Value,
    pub counters: BTreeMap<String, u64>,
}

#[derive(Debug, Clone, Default)]
pub struct Redactor {
    literal_secrets: Vec<String>,
}

impl Redactor {
    pub fn new<I, S>(literal_secrets: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        let mut literal_secrets: Vec<String> = literal_secrets
            .into_iter()
            .map(|secret| secret.as_ref().to_owned())
            .filter(|secret| !secret.is_empty())
            .collect();
        literal_secrets.sort_by_key(|secret| std::cmp::Reverse(secret.len()));
        literal_secrets.dedup();
        Self { literal_secrets }
    }

    pub fn from_environment() -> Self {
        Self::new(std::env::vars().filter_map(|(key, value)| {
            env_kind(&key)
                .is_some()
                .then_some(value)
                .filter(|value| !value.is_empty())
        }))
    }

    pub fn redact(&self, mut record: Value) -> Result<RedactedRecord, RedactionError> {
        if minimum_json_size(&record) > MAX_RECORD_BYTES {
            return Err(RedactionError::RecordTooLarge);
        }
        let started = Instant::now();
        if bounded_json_size(&record, MAX_RECORD_BYTES, started)?.is_none() {
            return Err(RedactionError::RecordTooLarge);
        }

        let mut counters = BTreeMap::new();
        self.redact_value(
            &mut record,
            &mut counters,
            started,
            StructuredContext::Generic,
        )?;
        if started.elapsed() > MAX_PROCESSING_TIME {
            return Err(RedactionError::ProcessingTimedOut);
        }
        Ok(RedactedRecord { record, counters })
    }

    fn redact_value(
        &self,
        value: &mut Value,
        counters: &mut BTreeMap<String, u64>,
        started: Instant,
        context: StructuredContext,
    ) -> Result<(), RedactionError> {
        if started.elapsed() > MAX_PROCESSING_TIME {
            return Err(RedactionError::ProcessingTimedOut);
        }
        match value {
            Value::Object(map) => {
                let local_path_keys: Vec<String> = map
                    .keys()
                    .filter(|key| is_local_path_key(key))
                    .cloned()
                    .collect();
                for key in local_path_keys {
                    map.remove(&key);
                    increment(counters, "local_path", 1);
                }
                for (key, child) in map.iter_mut() {
                    let kind = match context {
                        StructuredContext::Headers => header_kind(key),
                        StructuredContext::Environment => env_kind(key),
                        StructuredContext::Generic => None,
                    };
                    if let Some(kind) = kind {
                        *child = Value::String(marker(kind));
                        increment(counters, kind, 1);
                    } else {
                        let child_context = match key.to_ascii_lowercase().as_str() {
                            "headers" | "header" => StructuredContext::Headers,
                            "env" | "environment" => StructuredContext::Environment,
                            _ => StructuredContext::Generic,
                        };
                        self.redact_value(child, counters, started, child_context)?;
                    }
                }
            }
            Value::Array(values) => {
                for child in values {
                    self.redact_value(child, counters, started, context)?;
                }
            }
            Value::String(text) => self.redact_text(text, counters, started)?,
            _ => {}
        }
        Ok(())
    }

    fn redact_text(
        &self,
        text: &mut String,
        counters: &mut BTreeMap<String, u64>,
        started: Instant,
    ) -> Result<(), RedactionError> {
        redact_sensitive_lines(text, counters);
        if started.elapsed() > MAX_PROCESSING_TIME {
            return Err(RedactionError::ProcessingTimedOut);
        }
        replace_pem_blocks(text, counters);
        for (prefix, minimum_length, kind) in [
            ("ghp_", 24, "github_token"),
            ("gho_", 24, "github_token"),
            ("ghu_", 24, "github_token"),
            ("ghs_", 24, "github_token"),
            ("ghr_", 24, "github_token"),
            ("sk-ant-", 20, "anthropic_key"),
            ("sk-proj-", 20, "openai_key"),
            ("sk-", 20, "openai_key"),
            ("flk_", 20, "fleet_token"),
        ] {
            if started.elapsed() > MAX_PROCESSING_TIME {
                return Err(RedactionError::ProcessingTimedOut);
            }
            replace_token_format(text, prefix, minimum_length, kind, counters, started)?;
        }
        for secret in &self.literal_secrets {
            if started.elapsed() > MAX_PROCESSING_TIME {
                return Err(RedactionError::ProcessingTimedOut);
            }
            let count = text.matches(secret).count() as u64;
            if count > 0 {
                *text = text.replace(secret, &marker("literal_secret"));
                increment(counters, "literal_secret", count);
            }
        }
        Ok(())
    }
}

#[derive(Clone, Copy)]
enum StructuredContext {
    Generic,
    Headers,
    Environment,
}

fn header_kind(key: &str) -> Option<&'static str> {
    match key.to_ascii_lowercase().replace('-', "_").as_str() {
        "authorization" => Some("authorization"),
        "proxy_authorization" => Some("proxy_authorization"),
        "cookie" => Some("cookie"),
        "set_cookie" => Some("set_cookie"),
        "x_api_key" => Some("api_key"),
        _ => None,
    }
}

fn env_kind(key: &str) -> Option<&'static str> {
    match key.to_ascii_lowercase().replace('-', "_").as_str() {
        normalized if normalized.contains("private_key") => Some("private_key"),
        normalized if normalized.contains("password") => Some("password"),
        normalized if normalized.contains("api_key") => Some("api_key"),
        normalized if normalized.contains("secret") => Some("secret"),
        normalized if normalized.contains("token") => Some("token"),
        normalized if normalized.contains("cookie") => Some("cookie"),
        _ => None,
    }
}

fn is_local_path_key(key: &str) -> bool {
    matches!(
        key.to_ascii_lowercase().as_str(),
        "pid" | "cwd" | "jsonl_path" | "workspace_path" | "absolute_path"
    )
}

fn replace_pem_blocks(text: &mut String, counters: &mut BTreeMap<String, u64>) {
    const BEGIN: &str = "-----BEGIN ";
    const END_PREFIX: &str = "-----END ";
    let mut cursor = 0;
    while let Some(relative_begin) = text[cursor..].find(BEGIN) {
        let begin = cursor + relative_begin;
        let header_start = begin + BEGIN.len();
        let Some(header_end_relative) = text[header_start..].find("-----") else {
            break;
        };
        let header_end = header_start + header_end_relative + 5;
        let header = &text[header_start..header_end - 5];
        if !header.contains("PRIVATE KEY") {
            cursor = header_end;
            continue;
        }
        let end_marker = format!("{END_PREFIX}{header}-----");
        let Some(end_relative) = text[header_end..].find(&end_marker) else {
            cursor = header_end;
            continue;
        };
        let end = header_end + end_relative + end_marker.len();
        text.replace_range(begin..end, &marker("private_key"));
        increment(counters, "private_key", 1);
        cursor = begin + marker("private_key").len();
    }
}

fn redact_sensitive_lines(text: &mut String, counters: &mut BTreeMap<String, u64>) {
    if !text.contains('=') && !text.contains(':') {
        return;
    }
    let mut output = String::with_capacity(text.len());
    let mut changed = false;
    for segment in text.split_inclusive('\n') {
        let (line, newline) = segment
            .strip_suffix('\n')
            .map_or((segment, ""), |line| (line, "\n"));
        let trimmed = line.trim_start();
        let env_line = trimmed.strip_prefix("export ").unwrap_or(trimmed);
        if let Some((key, _)) = env_line.split_once('=') {
            if let Some(kind) = env_kind(key.trim()) {
                let separator = line.find('=').unwrap_or(line.len());
                output.push_str(&line[..=separator]);
                output.push_str(&marker(kind));
                output.push_str(newline);
                increment(counters, kind, 1);
                changed = true;
                continue;
            }
        }
        if let Some((key, _)) = trimmed.split_once(':') {
            if let Some(kind) = header_kind(key.trim()) {
                let separator = line.find(':').unwrap_or(line.len());
                output.push_str(&line[..=separator]);
                output.push(' ');
                output.push_str(&marker(kind));
                output.push_str(newline);
                increment(counters, kind, 1);
                changed = true;
                continue;
            }
        }
        output.push_str(segment);
    }
    if changed {
        *text = output;
    }
}

fn replace_token_format(
    text: &mut String,
    prefix: &str,
    minimum_length: usize,
    kind: &'static str,
    counters: &mut BTreeMap<String, u64>,
    started: Instant,
) -> Result<(), RedactionError> {
    let mut cursor = 0;
    while let Some(relative_start) = text[cursor..].find(prefix) {
        let start = cursor + relative_start;
        let mut end = start;
        for (index, character) in text[start..].char_indices() {
            if !(character.is_ascii_alphanumeric() || matches!(character, '_' | '-')) {
                break;
            }
            end = start + index + character.len_utf8();
            if index & 0xffff == 0 && started.elapsed() > MAX_PROCESSING_TIME {
                return Err(RedactionError::ProcessingTimedOut);
            }
        }
        if end - start >= minimum_length {
            let replacement = marker(kind);
            text.replace_range(start..end, &replacement);
            increment(counters, kind, 1);
            cursor = start + replacement.len();
        } else {
            cursor = start + prefix.len();
        }
    }
    Ok(())
}

fn marker(kind: &str) -> String {
    format!("[REDACTED:{kind}]")
}

fn increment(counters: &mut BTreeMap<String, u64>, kind: &str, count: u64) {
    *counters.entry(kind.to_owned()).or_default() += count;
}

fn minimum_json_size(value: &Value) -> usize {
    match value {
        Value::Null | Value::Bool(_) => 4,
        Value::Number(number) => number.to_string().len(),
        Value::String(value) => value.len().saturating_add(2),
        Value::Array(values) => values.iter().fold(
            2usize.saturating_add(values.len().saturating_sub(1)),
            |size, value| size.saturating_add(minimum_json_size(value)),
        ),
        Value::Object(map) => map.iter().fold(
            2usize.saturating_add(map.len().saturating_sub(1)),
            |size, (key, value)| {
                size.saturating_add(key.len())
                    .saturating_add(3)
                    .saturating_add(minimum_json_size(value))
            },
        ),
    }
}

fn bounded_json_size(
    value: &Value,
    limit: usize,
    started: Instant,
) -> Result<Option<usize>, RedactionError> {
    fn add(total: &mut usize, amount: usize, limit: usize) -> Option<()> {
        *total = total.checked_add(amount)?;
        (*total <= limit).then_some(())
    }

    fn string_size(
        value: &str,
        total: &mut usize,
        limit: usize,
        started: Instant,
    ) -> Result<Option<()>, RedactionError> {
        if add(total, 2, limit).is_none() {
            return Ok(None);
        }
        for (index, byte) in value.bytes().enumerate() {
            if index & 0xffff == 0 && started.elapsed() > MAX_PROCESSING_TIME {
                return Err(RedactionError::ProcessingTimedOut);
            }
            let encoded = match byte {
                b'"' | b'\\' | 0x08 | 0x0c | b'\n' | b'\r' | b'\t' => 2,
                0x00..=0x1f => 6,
                _ => 1,
            };
            if add(total, encoded, limit).is_none() {
                return Ok(None);
            }
        }
        Ok(Some(()))
    }

    fn visit(
        value: &Value,
        total: &mut usize,
        limit: usize,
        started: Instant,
    ) -> Result<Option<()>, RedactionError> {
        let result = match value {
            Value::Null => add(total, 4, limit),
            Value::Bool(true) => add(total, 4, limit),
            Value::Bool(false) => add(total, 5, limit),
            Value::Number(number) => add(total, number.to_string().len(), limit),
            Value::String(value) => return string_size(value, total, limit, started),
            Value::Array(values) => {
                if add(total, 2, limit).is_none() {
                    return Ok(None);
                }
                for (index, value) in values.iter().enumerate() {
                    if index > 0 && add(total, 1, limit).is_none() {
                        return Ok(None);
                    }
                    if visit(value, total, limit, started)?.is_none() {
                        return Ok(None);
                    }
                }
                Some(())
            }
            Value::Object(map) => {
                if add(total, 2, limit).is_none() {
                    return Ok(None);
                }
                for (index, (key, value)) in map.iter().enumerate() {
                    if index > 0 && add(total, 1, limit).is_none() {
                        return Ok(None);
                    }
                    if string_size(key, total, limit, started)?.is_none()
                        || add(total, 1, limit).is_none()
                        || visit(value, total, limit, started)?.is_none()
                    {
                        return Ok(None);
                    }
                }
                Some(())
            }
        };
        Ok(result)
    }

    let mut total = 0;
    if visit(value, &mut total, limit, started)?.is_none() {
        return Ok(None);
    }
    Ok(Some(total))
}
