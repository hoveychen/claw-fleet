use axum::{http::StatusCode, response::IntoResponse, Json};
use serde_json::json;
use thiserror::Error;
use uuid::Uuid;

#[derive(Debug, Error)]
pub enum ApiError {
    #[error("authentication required")]
    AuthenticationRequired,
    #[error("permission denied")]
    PermissionDenied,
    #[error("resource not found")]
    NotFound,
    #[error("{0}")]
    Validation(String),
    #[error("idempotency key was reused with a different request")]
    IdempotencyMismatch,
    #[error("resource state does not allow this operation")]
    StateConflict,
    #[error("If-Match does not match the current resource version")]
    VersionConflict,
    #[error("If-Match is required")]
    PreconditionRequired,
    #[error("decision has already been resolved")]
    DecisionAlreadyResolved,
    #[error("decision deadline has expired")]
    DecisionExpired,
    #[error("runner does not advertise the required capability")]
    RunnerCapabilityMissing,
    #[error("runner is unavailable for assignment")]
    RunnerUnavailable,
    #[error("database error")]
    Database(#[from] sqlx::Error),
    #[error("internal error")]
    Internal,
}

impl ApiError {
    fn status_and_code(&self) -> (StatusCode, &'static str, &'static str) {
        match self {
            Self::AuthenticationRequired => (
                StatusCode::UNAUTHORIZED,
                "authentication",
                "authentication_required",
            ),
            Self::PermissionDenied => (StatusCode::FORBIDDEN, "authorization", "permission_denied"),
            Self::NotFound => (StatusCode::NOT_FOUND, "not_found", "resource_not_found"),
            Self::Validation(_) => (
                StatusCode::UNPROCESSABLE_ENTITY,
                "validation",
                "validation_failed",
            ),
            Self::IdempotencyMismatch => (StatusCode::CONFLICT, "conflict", "idempotency_mismatch"),
            Self::StateConflict => (StatusCode::CONFLICT, "conflict", "state_conflict"),
            Self::VersionConflict => (
                StatusCode::PRECONDITION_FAILED,
                "conflict",
                "version_conflict",
            ),
            Self::PreconditionRequired => (
                StatusCode::PRECONDITION_REQUIRED,
                "precondition",
                "if_match_required",
            ),
            Self::DecisionAlreadyResolved => (
                StatusCode::CONFLICT,
                "conflict",
                "decision_already_resolved",
            ),
            Self::DecisionExpired => (
                StatusCode::CONFLICT,
                "conflict",
                "decision_expired",
            ),
            Self::RunnerCapabilityMissing => {
                (StatusCode::CONFLICT, "runner", "runner_capability_missing")
            }
            Self::RunnerUnavailable => (StatusCode::CONFLICT, "runner", "runner_unavailable"),
            Self::Database(_) | Self::Internal => (
                StatusCode::INTERNAL_SERVER_ERROR,
                "internal",
                "internal_error",
            ),
        }
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> axum::response::Response {
        let (status, error_type, code) = self.status_and_code();
        let message = if status.is_server_error() {
            "An internal error occurred.".to_owned()
        } else {
            self.to_string()
        };
        let request_id = format!("req_{}", Uuid::now_v7().simple());
        (
            status,
            Json(json!({
                "error": {
                    "type": error_type,
                    "code": code,
                    "message": message,
                    "request_id": request_id
                }
            })),
        )
            .into_response()
    }
}
