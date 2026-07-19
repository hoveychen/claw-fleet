CREATE TABLE webhook_endpoints (
    id TEXT PRIMARY KEY,
    organization_id TEXT NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
    project_id TEXT NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
    url TEXT NOT NULL,
    event_types TEXT[] NOT NULL,
    description TEXT,
    enabled BOOLEAN NOT NULL DEFAULT TRUE,
    secret_ciphertext BYTEA NOT NULL,
    secret_nonce BYTEA NOT NULL,
    previous_secret_ciphertext BYTEA,
    previous_secret_nonce BYTEA,
    previous_valid_until TIMESTAMPTZ,
    last_delivery_at TIMESTAMPTZ,
    version BIGINT NOT NULL DEFAULT 1 CHECK (version >= 1),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    CHECK (cardinality(event_types) > 0)
);

CREATE INDEX webhook_endpoints_project_idx ON webhook_endpoints(project_id, created_at, id);

CREATE TABLE webhook_deliveries (
    id TEXT PRIMARY KEY,
    organization_id TEXT NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
    project_id TEXT NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
    endpoint_id TEXT NOT NULL REFERENCES webhook_endpoints(id) ON DELETE CASCADE,
    event_id TEXT NOT NULL REFERENCES events(id) ON DELETE CASCADE,
    status TEXT NOT NULL DEFAULT 'pending',
    attempt_count INTEGER NOT NULL DEFAULT 0 CHECK (attempt_count >= 0),
    next_attempt_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    deadline_at TIMESTAMPTZ NOT NULL DEFAULT (now() + interval '24 hours'),
    last_status_code INTEGER,
    last_error TEXT,
    delivered_at TIMESTAMPTZ,
    replay_of_id TEXT REFERENCES webhook_deliveries(id),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    CHECK (status IN ('pending','delivered','failed'))
);

CREATE UNIQUE INDEX webhook_deliveries_initial_unique
    ON webhook_deliveries(endpoint_id, event_id) WHERE replay_of_id IS NULL;
CREATE INDEX webhook_deliveries_due_idx
    ON webhook_deliveries(next_attempt_at, id) WHERE status = 'pending';

CREATE OR REPLACE FUNCTION enqueue_webhook_deliveries() RETURNS trigger AS $$
BEGIN
    INSERT INTO webhook_deliveries(
        id, organization_id, project_id, endpoint_id, event_id
    )
    SELECT
        'del_' || replace(gen_random_uuid()::text, '-', ''),
        NEW.organization_id,
        NEW.project_id,
        endpoint.id,
        NEW.id
    FROM webhook_endpoints endpoint
    WHERE endpoint.organization_id = NEW.organization_id
      AND endpoint.project_id = NEW.project_id
      AND endpoint.enabled
      AND (NEW.event_type = ANY(endpoint.event_types) OR '*' = ANY(endpoint.event_types));
    RETURN NEW;
END;
$$ LANGUAGE plpgsql;

CREATE TRIGGER events_enqueue_webhooks
AFTER INSERT ON events
FOR EACH ROW EXECUTE FUNCTION enqueue_webhook_deliveries();

CREATE TABLE usage_hourly (
    organization_id TEXT NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
    project_id TEXT NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
    hour TIMESTAMPTZ NOT NULL,
    task_count BIGINT NOT NULL DEFAULT 0,
    run_seconds BIGINT NOT NULL DEFAULT 0,
    peak_runner_concurrency BIGINT NOT NULL DEFAULT 0,
    event_bytes BIGINT NOT NULL DEFAULT 0,
    artifact_bytes BIGINT NOT NULL DEFAULT 0,
    decision_count BIGINT NOT NULL DEFAULT 0,
    input_tokens BIGINT NOT NULL DEFAULT 0,
    output_tokens BIGINT NOT NULL DEFAULT 0,
    provider_cost_usd DOUBLE PRECISION NOT NULL DEFAULT 0,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (project_id, hour)
);

CREATE TABLE project_governance (
    project_id TEXT PRIMARY KEY REFERENCES projects(id) ON DELETE CASCADE,
    organization_id TEXT NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
    requests_per_minute INTEGER NOT NULL DEFAULT 600 CHECK (requests_per_minute > 0),
    max_concurrent_runs INTEGER NOT NULL DEFAULT 32 CHECK (max_concurrent_runs >= 0),
    max_artifact_bytes BIGINT NOT NULL DEFAULT 536870912 CHECK (max_artifact_bytes >= 0),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE TABLE api_rate_windows (
    project_id TEXT NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
    window_start TIMESTAMPTZ NOT NULL,
    request_count INTEGER NOT NULL DEFAULT 0,
    PRIMARY KEY (project_id, window_start)
);

CREATE TABLE api_metrics_minute (
    project_id TEXT NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
    route TEXT NOT NULL,
    minute TIMESTAMPTZ NOT NULL,
    request_count BIGINT NOT NULL DEFAULT 0,
    error_count BIGINT NOT NULL DEFAULT 0,
    total_latency_ms BIGINT NOT NULL DEFAULT 0,
    max_latency_ms BIGINT NOT NULL DEFAULT 0,
    PRIMARY KEY (project_id, route, minute)
);

CREATE OR REPLACE FUNCTION reject_audit_mutation() RETURNS trigger AS $$
BEGIN
    RAISE EXCEPTION 'audit_records are immutable';
END;
$$ LANGUAGE plpgsql;

CREATE TRIGGER audit_records_immutable
BEFORE UPDATE OR DELETE ON audit_records
FOR EACH ROW EXECUTE FUNCTION reject_audit_mutation();
