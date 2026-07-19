use std::time::{Duration, Instant};

use fleet_cloud_runner::redaction::{RedactionError, Redactor, MAX_RECORD_BYTES};
use serde_json::json;

fn assert_secret_absent(result: &fleet_cloud_runner::redaction::RedactedRecord, secret: &str) {
    let serialized = serde_json::to_string(result).unwrap();
    assert!(!serialized.contains(secret), "secret leaked: {secret}");
}

#[test]
fn structured_headers_env_and_local_paths_are_redacted_first() {
    let record = json!({
        "headers": {
            "Authorization": "Bearer auth-secret",
            "proxy-authorization": "Basic proxy-secret",
            "Cookie": "session=cookie-secret",
            "Set-Cookie": "session=set-cookie-secret",
            "X-Api-Key": "api-key-secret",
            "Content-Type": "application/json"
        },
        "env": {
            "ACCESS_TOKEN": "env-token-secret",
            "DB_PASSWORD": "env-password-secret",
            "SIGNING_PRIVATE_KEY": "env-key-secret",
            "PUBLIC_VALUE": "kept"
        },
        "cwd": "/private/customer/repository",
        "nested": {"workspace_path": "/private/worktree", "safe": true}
    });

    let result = Redactor::default().redact(record).unwrap();

    assert_eq!(
        result.record["headers"]["Authorization"],
        "[REDACTED:authorization]"
    );
    assert_eq!(
        result.record["headers"]["proxy-authorization"],
        "[REDACTED:proxy_authorization]"
    );
    assert_eq!(result.record["headers"]["Cookie"], "[REDACTED:cookie]");
    assert_eq!(
        result.record["headers"]["Set-Cookie"],
        "[REDACTED:set_cookie]"
    );
    assert_eq!(result.record["headers"]["X-Api-Key"], "[REDACTED:api_key]");
    assert_eq!(result.record["headers"]["Content-Type"], "application/json");
    assert_eq!(result.record["env"]["ACCESS_TOKEN"], "[REDACTED:token]");
    assert_eq!(result.record["env"]["DB_PASSWORD"], "[REDACTED:password]");
    assert_eq!(
        result.record["env"]["SIGNING_PRIVATE_KEY"],
        "[REDACTED:private_key]"
    );
    assert_eq!(result.record["env"]["PUBLIC_VALUE"], "kept");
    assert!(result.record.get("cwd").is_none());
    assert!(result.record["nested"].get("workspace_path").is_none());
    assert_eq!(result.counters.get("authorization"), Some(&1));
    assert_eq!(result.counters.get("local_path"), Some(&2));
    for secret in [
        "auth-secret",
        "proxy-secret",
        "cookie-secret",
        "set-cookie-secret",
        "api-key-secret",
        "env-token-secret",
        "env-password-secret",
        "env-key-secret",
        "/private/customer/repository",
        "/private/worktree",
    ] {
        assert_secret_absent(&result, secret);
    }
}

#[test]
fn pem_provider_tokens_and_configured_literals_are_redacted_with_counts() {
    let github = "ghp_abcdefghijklmnopqrstuvwxyz1234567890";
    let openai = "sk-proj-abcdefghijklmnopqrstuvwxyz1234567890";
    let anthropic = "sk-ant-api03-abcdefghijklmnopqrstuvwxyz1234567890";
    let fleet = "flk_live_abcdefghijklmnopqrstuvwxyz1234567890";
    let literal = "customer-supplied-secret-value";
    let pem = "-----BEGIN PRIVATE KEY-----\nabc123\ndef456\n-----END PRIVATE KEY-----";
    let record = json!({
        "text": format!("github={github} openai={openai} anthropic={anthropic} fleet={fleet} literal={literal}"),
        "pem": pem
    });

    let result = Redactor::new([literal]).redact(record).unwrap();

    for secret in [
        github, openai, anthropic, fleet, literal, "abc123", "def456",
    ] {
        assert_secret_absent(&result, secret);
    }
    for kind in [
        "github_token",
        "openai_key",
        "anthropic_key",
        "fleet_token",
        "literal_secret",
        "private_key",
    ] {
        assert_eq!(
            result.counters.get(kind),
            Some(&1),
            "missing count for {kind}"
        );
        assert!(serde_json::to_string(&result.record)
            .unwrap()
            .contains(&format!("[REDACTED:{kind}]")));
    }
}

#[test]
fn raw_dotenv_and_header_lines_are_redacted() {
    let record = json!({
        "output": "export OPENAI_API_KEY=plain-env-secret\nDB_PASSWORD=database-secret\nPUBLIC_VALUE=visible\nAuthorization: Bearer raw-auth-secret\nCookie: raw-cookie-secret\n"
    });
    let result = Redactor::default().redact(record).unwrap();
    let output = result.record["output"].as_str().unwrap();
    for secret in [
        "plain-env-secret",
        "database-secret",
        "raw-auth-secret",
        "raw-cookie-secret",
    ] {
        assert!(!output.contains(secret));
    }
    assert!(output.contains("OPENAI_API_KEY=[REDACTED:api_key]"));
    assert!(output.contains("DB_PASSWORD=[REDACTED:password]"));
    assert!(output.contains("PUBLIC_VALUE=visible"));
    assert!(output.contains("Authorization: [REDACTED:authorization]"));
    assert_eq!(result.counters.get("api_key"), Some(&1));
    assert_eq!(result.counters.get("password"), Some(&1));
    assert_eq!(result.counters.get("authorization"), Some(&1));
    assert_eq!(result.counters.get("cookie"), Some(&1));
}

#[test]
fn oversized_records_are_rejected_before_processing() {
    let record = json!({"text": "x".repeat(MAX_RECORD_BYTES + 1)});
    assert_eq!(
        Redactor::default().redact(record),
        Err(RedactionError::RecordTooLarge)
    );
}

#[test]
fn malicious_long_lines_finish_within_the_budget() {
    let adversarial = format!(
        "{}-----BEGIN PRIVATE KEY-----{}ghp_{}",
        "-".repeat(2_000_000),
        "A".repeat(2_000_000),
        "z".repeat(2_000_000)
    );
    let started = Instant::now();
    let result = Redactor::default().redact(json!({"output": adversarial}));
    assert!(
        started.elapsed() < Duration::from_millis(100),
        "bounded redaction failed to return promptly: {:?}",
        started.elapsed()
    );
    assert!(matches!(
        result,
        Ok(_) | Err(RedactionError::ProcessingTimedOut)
    ));
}
