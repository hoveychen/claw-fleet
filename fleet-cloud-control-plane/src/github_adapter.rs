use std::sync::Arc;
use std::time::Duration;

use axum::body::Bytes;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::IntoResponse;
use axum::routing::{get, post};
use axum::{Json, Router};
use chrono::{DateTime, Utc};
use hmac::{Hmac, Mac};
use jsonwebtoken::{Algorithm, EncodingKey, Header};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::Sha256;
use tokio::sync::Mutex;

use crate::services::tasks::{AgentSpec, CreateTaskRequest, WorkspaceSpec};

const STATUS_LABELS: &[&str] = &[
    "fleet:queued",
    "fleet:running",
    "fleet:waiting",
    "fleet:succeeded",
    "fleet:failed",
    "fleet:cancelled",
];

#[derive(Debug, thiserror::Error)]
pub enum AdapterError {
    #[error("invalid GitHub webhook signature")]
    InvalidSignature,
    #[error("invalid GitHub webhook payload: {0}")]
    InvalidPayload(String),
    #[error("invalid adapter configuration: {0}")]
    InvalidConfig(String),
    #[error("Fleet Cloud API request failed: {0}")]
    FleetApi(String),
    #[error("GitHub API request failed: {0}")]
    GithubApi(String),
}

impl IntoResponse for AdapterError {
    fn into_response(self) -> axum::response::Response {
        let status = match self {
            Self::InvalidSignature => StatusCode::UNAUTHORIZED,
            Self::InvalidPayload(_) => StatusCode::BAD_REQUEST,
            Self::InvalidConfig(_) | Self::FleetApi(_) | Self::GithubApi(_) => {
                StatusCode::BAD_GATEWAY
            }
        };
        (status, Json(json!({"error": self.to_string()}))).into_response()
    }
}

pub fn verify_github_signature(
    secret: &[u8],
    raw_body: &[u8],
    signature: &str,
) -> Result<(), AdapterError> {
    let digest = signature
        .strip_prefix("sha256=")
        .ok_or(AdapterError::InvalidSignature)?;
    let digest = hex::decode(digest).map_err(|_| AdapterError::InvalidSignature)?;
    let mut mac = Hmac::<Sha256>::new_from_slice(secret)
        .map_err(|_| AdapterError::InvalidSignature)?;
    mac.update(raw_body);
    mac.verify_slice(&digest)
        .map_err(|_| AdapterError::InvalidSignature)
}

#[derive(Debug, Clone)]
pub struct LabeledIssueAction {
    pub issue_number: u64,
    pub external_id: String,
    pub task: CreateTaskRequest,
}

pub fn parse_labeled_issue(
    payload: &Value,
    repository: &str,
    project_id: &str,
) -> Result<Option<LabeledIssueAction>, AdapterError> {
    if payload.get("action").and_then(Value::as_str) != Some("labeled")
        || payload.pointer("/label/name").and_then(Value::as_str) != Some("fleet-task")
        || payload
            .pointer("/repository/full_name")
            .and_then(Value::as_str)
            != Some(repository)
    {
        return Ok(None);
    }
    let issue_number = payload
        .pointer("/issue/number")
        .and_then(Value::as_u64)
        .ok_or_else(|| AdapterError::InvalidPayload("issue.number is required".into()))?;
    let title = payload
        .pointer("/issue/title")
        .and_then(Value::as_str)
        .ok_or_else(|| AdapterError::InvalidPayload("issue.title is required".into()))?;
    let clone_url = payload
        .pointer("/repository/clone_url")
        .and_then(Value::as_str)
        .ok_or_else(|| AdapterError::InvalidPayload("repository.clone_url is required".into()))?;
    let default_branch = payload
        .pointer("/repository/default_branch")
        .and_then(Value::as_str)
        .unwrap_or("main");
    let issue_url = payload
        .pointer("/issue/html_url")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let body = payload
        .pointer("/issue/body")
        .and_then(Value::as_str)
        .unwrap_or("No issue body was provided.");
    let external_id = format!("github:{repository}#{issue_number}");
    let goal = format!(
        "Handle GitHub Issue #{issue_number}: {title}\n\n{body}\n\nSource: {issue_url}"
    );
    Ok(Some(LabeledIssueAction {
        issue_number,
        external_id: external_id.clone(),
        task: CreateTaskRequest {
            project_id: project_id.to_owned(),
            external_id: Some(external_id),
            title: Some(title.to_owned()),
            goal,
            workspace: WorkspaceSpec {
                repository: clone_url.to_owned(),
                r#ref: default_branch.to_owned(),
                subdirectory: None,
            },
            agent: AgentSpec {
                provider: "codex".into(),
                model: Some("gpt-5.6-sol".into()),
                effort: Some("medium".into()),
                permission_policy_id: Some("policy_pilot".into()),
            },
            metadata: json!({
                "source": "github_app",
                "repository": repository,
                "issue_number": issue_number,
                "issue_url": issue_url,
            }),
        },
    }))
}

pub fn status_label(status: &str) -> Option<&'static str> {
    match status {
        "queued" => Some("fleet:queued"),
        "running" => Some("fleet:running"),
        "waiting_input" | "paused" => Some("fleet:waiting"),
        "succeeded" => Some("fleet:succeeded"),
        "failed" => Some("fleet:failed"),
        "cancelled" => Some("fleet:cancelled"),
        _ => None,
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct FleetTask {
    pub id: String,
    pub external_id: Option<String>,
    pub status: String,
    pub title: Option<String>,
}

#[derive(Debug, Deserialize)]
struct FleetCreateResponse {
    task: FleetTask,
}

#[derive(Debug, Deserialize)]
struct FleetTaskPage {
    data: Vec<FleetTask>,
    has_more: bool,
    next_cursor: Option<String>,
}

#[derive(Clone)]
pub struct FleetApiClient {
    client: reqwest::Client,
    base_url: String,
    project_id: String,
    api_key: String,
}

impl FleetApiClient {
    pub fn new(base_url: String, project_id: String, api_key: String) -> Result<Self, AdapterError> {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(15))
            .build()
            .map_err(|error| AdapterError::InvalidConfig(error.to_string()))?;
        Ok(Self {
            client,
            base_url: base_url.trim_end_matches('/').to_owned(),
            project_id,
            api_key,
        })
    }

    pub async fn create_task(
        &self,
        delivery_id: &str,
        request: &CreateTaskRequest,
    ) -> Result<FleetTask, AdapterError> {
        let response = self
            .client
            .post(format!("{}/tasks", self.base_url))
            .bearer_auth(&self.api_key)
            .header("idempotency-key", format!("github-delivery-{delivery_id}"))
            .json(request)
            .send()
            .await
            .map_err(|error| AdapterError::FleetApi(error.to_string()))?;
        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(AdapterError::FleetApi(format!("{status}: {body}")));
        }
        response
            .json::<FleetCreateResponse>()
            .await
            .map(|response| response.task)
            .map_err(|error| AdapterError::FleetApi(error.to_string()))
    }

    pub async fn list_tasks(&self) -> Result<Vec<FleetTask>, AdapterError> {
        let mut cursor = None::<String>;
        let mut tasks = Vec::new();
        loop {
            let mut url = reqwest::Url::parse(&format!("{}/tasks", self.base_url))
                .map_err(|error| AdapterError::InvalidConfig(error.to_string()))?;
            {
                let mut query = url.query_pairs_mut();
                query.append_pair("project_id", &self.project_id);
                query.append_pair("limit", "200");
                if let Some(cursor) = cursor.as_deref() {
                    query.append_pair("cursor", cursor);
                }
            }
            let response = self
                .client
                .get(url)
                .bearer_auth(&self.api_key)
                .send()
                .await
                .map_err(|error| AdapterError::FleetApi(error.to_string()))?;
            if !response.status().is_success() {
                return Err(AdapterError::FleetApi(response.status().to_string()));
            }
            let page = response
                .json::<FleetTaskPage>()
                .await
                .map_err(|error| AdapterError::FleetApi(error.to_string()))?;
            tasks.extend(page.data);
            if !page.has_more {
                return Ok(tasks);
            }
            cursor = page.next_cursor;
            if cursor.is_none() {
                return Err(AdapterError::FleetApi(
                    "task page has_more without next_cursor".into(),
                ));
            }
        }
    }
}

#[derive(Debug, Serialize)]
struct GithubAppClaims<'a> {
    iat: i64,
    exp: i64,
    iss: &'a str,
}

#[derive(Debug, Clone)]
struct InstallationToken {
    value: String,
    expires_at: DateTime<Utc>,
}

#[derive(Clone)]
pub struct GithubApiClient {
    client: reqwest::Client,
    api_url: String,
    app_id: String,
    installation_id: String,
    encoding_key: Arc<EncodingKey>,
    token: Arc<Mutex<Option<InstallationToken>>>,
}

impl GithubApiClient {
    pub fn new(
        api_url: String,
        app_id: String,
        installation_id: String,
        private_key_pem: &str,
    ) -> Result<Self, AdapterError> {
        let normalized_key = private_key_pem.replace("\\n", "\n");
        let encoding_key = EncodingKey::from_rsa_pem(normalized_key.as_bytes())
            .map_err(|error| AdapterError::InvalidConfig(error.to_string()))?;
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(15))
            .user_agent("fleet-cloud-github-adapter/1")
            .build()
            .map_err(|error| AdapterError::InvalidConfig(error.to_string()))?;
        Ok(Self {
            client,
            api_url: api_url.trim_end_matches('/').to_owned(),
            app_id,
            installation_id,
            encoding_key: Arc::new(encoding_key),
            token: Arc::new(Mutex::new(None)),
        })
    }

    async fn installation_token(&self) -> Result<String, AdapterError> {
        let mut cached = self.token.lock().await;
        if let Some(token) = cached.as_ref() {
            if token.expires_at > Utc::now() + chrono::Duration::minutes(2) {
                return Ok(token.value.clone());
            }
        }
        let now = Utc::now().timestamp();
        let jwt = jsonwebtoken::encode(
            &Header::new(Algorithm::RS256),
            &GithubAppClaims {
                iat: now - 60,
                exp: now + 9 * 60,
                iss: &self.app_id,
            },
            &self.encoding_key,
        )
        .map_err(|error| AdapterError::GithubApi(error.to_string()))?;
        let response = self
            .client
            .post(format!(
                "{}/app/installations/{}/access_tokens",
                self.api_url, self.installation_id
            ))
            .bearer_auth(jwt)
            .header("accept", "application/vnd.github+json")
            .header("x-github-api-version", "2022-11-28")
            .send()
            .await
            .map_err(|error| AdapterError::GithubApi(error.to_string()))?;
        if !response.status().is_success() {
            return Err(AdapterError::GithubApi(response.status().to_string()));
        }
        #[derive(Deserialize)]
        struct TokenResponse {
            token: String,
            expires_at: DateTime<Utc>,
        }
        let response = response
            .json::<TokenResponse>()
            .await
            .map_err(|error| AdapterError::GithubApi(error.to_string()))?;
        *cached = Some(InstallationToken {
            value: response.token.clone(),
            expires_at: response.expires_at,
        });
        Ok(response.token)
    }
}

#[derive(Clone)]
pub struct GithubWebhookState {
    pub webhook_secret: Arc<Vec<u8>>,
    pub repository: Arc<String>,
    pub project_id: Arc<String>,
    pub fleet: FleetApiClient,
}

pub fn webhook_router(state: GithubWebhookState) -> Router {
    Router::new()
        .route("/health/live", get(|| async { Json(json!({"status":"ok"})) }))
        .route("/github/webhook", post(github_webhook))
        .with_state(state)
}

async fn github_webhook(
    State(state): State<GithubWebhookState>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<impl IntoResponse, AdapterError> {
    let signature = required_header(&headers, "x-hub-signature-256")?;
    verify_github_signature(&state.webhook_secret, &body, signature)?;
    let event = required_header(&headers, "x-github-event")?;
    let delivery = required_header(&headers, "x-github-delivery")?;
    if event != "issues" {
        return Ok((StatusCode::ACCEPTED, Json(json!({"status":"ignored"}))));
    }
    let payload: Value = serde_json::from_slice(&body)
        .map_err(|error| AdapterError::InvalidPayload(error.to_string()))?;
    let Some(action) = parse_labeled_issue(&payload, &state.repository, &state.project_id)? else {
        return Ok((StatusCode::ACCEPTED, Json(json!({"status":"ignored"}))));
    };
    let task = state.fleet.create_task(delivery, &action.task).await?;
    Ok((
        StatusCode::ACCEPTED,
        Json(json!({"status":"accepted","task_id":task.id})),
    ))
}

fn required_header<'a>(headers: &'a HeaderMap, name: &str) -> Result<&'a str, AdapterError> {
    headers
        .get(name)
        .and_then(|value| value.to_str().ok())
        .ok_or_else(|| AdapterError::InvalidPayload(format!("{name} header is required")))
}
