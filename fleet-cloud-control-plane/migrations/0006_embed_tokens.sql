CREATE TABLE embed_tokens (
    id TEXT PRIMARY KEY,
    organization_id TEXT NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
    project_id TEXT NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
    task_id TEXT REFERENCES tasks(id) ON DELETE CASCADE,
    token_hash BYTEA NOT NULL UNIQUE,
    allowed_origins JSONB NOT NULL,
    views JSONB NOT NULL,
    created_by_api_key_id TEXT NOT NULL REFERENCES api_keys(id),
    expires_at TIMESTAMPTZ NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    revoked_at TIMESTAMPTZ
);

CREATE INDEX embed_tokens_active_hash_idx
    ON embed_tokens(token_hash) WHERE revoked_at IS NULL;
