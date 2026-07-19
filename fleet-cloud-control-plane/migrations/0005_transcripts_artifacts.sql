CREATE TABLE transcript_records (
    id TEXT PRIMARY KEY,
    organization_id TEXT NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
    project_id TEXT NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
    task_id TEXT NOT NULL REFERENCES tasks(id) ON DELETE CASCADE,
    run_id TEXT NOT NULL REFERENCES runs(id) ON DELETE CASCADE,
    source_sequence BIGINT NOT NULL CHECK (source_sequence >= 1),
    record_type TEXT NOT NULL,
    role TEXT NOT NULL,
    content JSONB NOT NULL,
    redactions JSONB NOT NULL DEFAULT '{}'::jsonb,
    occurred_at TIMESTAMPTZ NOT NULL,
    retention_until TIMESTAMPTZ NOT NULL DEFAULT (now() + interval '30 days'),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (run_id, source_sequence)
);

CREATE INDEX transcript_records_run_sequence_idx
    ON transcript_records(run_id, source_sequence);
CREATE INDEX transcript_records_retention_idx
    ON transcript_records(retention_until, id);

CREATE TABLE artifacts (
    id TEXT PRIMARY KEY,
    organization_id TEXT NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
    project_id TEXT NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
    task_id TEXT NOT NULL REFERENCES tasks(id) ON DELETE CASCADE,
    run_id TEXT REFERENCES runs(id) ON DELETE SET NULL,
    kind TEXT NOT NULL,
    filename TEXT NOT NULL,
    mime_type TEXT NOT NULL,
    size_bytes BIGINT NOT NULL CHECK (size_bytes >= 0),
    sha256 TEXT NOT NULL CHECK (sha256 ~ '^[0-9a-f]{64}$'),
    status TEXT NOT NULL DEFAULT 'active',
    retention_until TIMESTAMPTZ NOT NULL DEFAULT (now() + interval '180 days'),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    deleted_at TIMESTAMPTZ,
    CHECK (kind IN ('attachment','image','patch','report','log','wiki','other')),
    CHECK (status IN ('active','deleting','deleted'))
);

CREATE INDEX artifacts_task_created_idx ON artifacts(task_id, created_at, id);
CREATE INDEX artifacts_retention_idx ON artifacts(retention_until, id) WHERE status = 'active';

CREATE TABLE artifact_objects (
    artifact_id TEXT PRIMARY KEY REFERENCES artifacts(id) ON DELETE CASCADE,
    object_key TEXT NOT NULL UNIQUE,
    ciphertext BYTEA NOT NULL,
    object_nonce BYTEA NOT NULL,
    encrypted_data_key BYTEA NOT NULL,
    data_key_nonce BYTEA NOT NULL,
    kms_key_id TEXT NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE TABLE retention_jobs (
    job_name TEXT PRIMARY KEY,
    lease_owner TEXT,
    lease_until TIMESTAMPTZ,
    last_started_at TIMESTAMPTZ,
    last_completed_at TIMESTAMPTZ,
    last_error TEXT,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE TABLE task_usage_summaries (
    task_id TEXT PRIMARY KEY,
    organization_id TEXT NOT NULL,
    project_id TEXT NOT NULL,
    input_tokens BIGINT NOT NULL DEFAULT 0,
    output_tokens BIGINT NOT NULL DEFAULT 0,
    provider_cost_usd DOUBLE PRECISION NOT NULL DEFAULT 0,
    event_bytes BIGINT NOT NULL DEFAULT 0,
    artifact_bytes BIGINT NOT NULL DEFAULT 0,
    content_deleted_at TIMESTAMPTZ,
    retained_until TIMESTAMPTZ NOT NULL DEFAULT (now() + interval '365 days'),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
