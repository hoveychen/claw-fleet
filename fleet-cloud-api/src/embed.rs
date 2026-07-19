use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use hmac::{Hmac, Mac};
use serde::{Deserialize, Serialize};
use sha2::Sha256;
use std::fmt;
use uuid::Uuid;

type HmacSha256 = Hmac<Sha256>;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EmbedClaims {
    pub organization_id: Uuid,
    pub project_id: Uuid,
    pub task_id: Option<Uuid>,
    pub origins: Vec<String>,
    pub expires_at: i64,
}

impl EmbedClaims {
    pub fn allows_task(&self, task_id: Uuid) -> bool {
        self.task_id.is_none_or(|allowed| allowed == task_id)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EmbedError(pub String);

impl fmt::Display for EmbedError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl std::error::Error for EmbedError {}

#[derive(Clone)]
pub struct EmbedTokenVerifier {
    secret: Vec<u8>,
}

impl EmbedTokenVerifier {
    pub fn new(secret: &[u8]) -> Self {
        Self {
            secret: secret.to_vec(),
        }
    }

    pub fn issue(&self, claims: &EmbedClaims) -> Result<String, EmbedError> {
        if claims.origins.is_empty() {
            return Err(EmbedError(
                "embed token requires at least one origin".into(),
            ));
        }
        let payload = serde_json::to_vec(claims).map_err(|error| EmbedError(error.to_string()))?;
        let encoded = URL_SAFE_NO_PAD.encode(payload);
        let signature = self.sign(encoded.as_bytes())?;
        Ok(format!("{encoded}.{}", URL_SAFE_NO_PAD.encode(signature)))
    }

    pub fn verify(
        &self,
        token: &str,
        origin: &str,
        now_epoch_seconds: i64,
    ) -> Result<EmbedClaims, EmbedError> {
        let (payload, signature) = token
            .split_once('.')
            .ok_or_else(|| EmbedError("malformed embed token".into()))?;
        let signature = URL_SAFE_NO_PAD
            .decode(signature)
            .map_err(|_| EmbedError("malformed embed signature".into()))?;
        let mut mac = HmacSha256::new_from_slice(&self.secret)
            .map_err(|_| EmbedError("invalid embed secret".into()))?;
        mac.update(payload.as_bytes());
        mac.verify_slice(&signature)
            .map_err(|_| EmbedError("invalid embed signature".into()))?;
        let payload = URL_SAFE_NO_PAD
            .decode(payload)
            .map_err(|_| EmbedError("malformed embed payload".into()))?;
        let claims: EmbedClaims = serde_json::from_slice(&payload)
            .map_err(|_| EmbedError("invalid embed claims".into()))?;
        if claims.expires_at <= now_epoch_seconds {
            return Err(EmbedError("embed token expired".into()));
        }
        if !claims.origins.iter().any(|allowed| allowed == origin) {
            return Err(EmbedError("embed origin is not allowed".into()));
        }
        Ok(claims)
    }

    fn sign(&self, message: &[u8]) -> Result<Vec<u8>, EmbedError> {
        let mut mac = HmacSha256::new_from_slice(&self.secret)
            .map_err(|_| EmbedError("invalid embed secret".into()))?;
        mac.update(message);
        Ok(mac.finalize().into_bytes().to_vec())
    }
}
