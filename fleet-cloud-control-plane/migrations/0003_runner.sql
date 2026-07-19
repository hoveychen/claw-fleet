CREATE TABLE runner_pools (
    id TEXT PRIMARY KEY,
    organization_id TEXT NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
    project_id TEXT NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
    name TEXT NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE TABLE runner_registration_tokens (
    id TEXT PRIMARY KEY,
    organization_id TEXT NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
    project_id TEXT NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
    pool_id TEXT NOT NULL REFERENCES runner_pools(id) ON DELETE CASCADE,
    name TEXT,
    token_hash BYTEA NOT NULL UNIQUE,
    expires_at TIMESTAMPTZ NOT NULL,
    used_at TIMESTAMPTZ,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE TABLE runners (
    id TEXT PRIMARY KEY,
    organization_id TEXT NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
    project_id TEXT NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
    pool_id TEXT NOT NULL REFERENCES runner_pools(id),
    name TEXT NOT NULL,
    status TEXT NOT NULL DEFAULT 'offline',
    certificate_fingerprint BYTEA NOT NULL UNIQUE,
    build_version TEXT,
    platform TEXT,
    architecture TEXT,
    capabilities JSONB NOT NULL DEFAULT '[]'::jsonb,
    labels JSONB NOT NULL DEFAULT '{}'::jsonb,
    max_concurrency INTEGER NOT NULL DEFAULT 1 CHECK (max_concurrency BETWEEN 1 AND 256),
    active_runs INTEGER NOT NULL DEFAULT 0 CHECK (active_runs >= 0),
    scheduling_enabled BOOLEAN NOT NULL DEFAULT TRUE,
    last_heartbeat_at TIMESTAMPTZ,
    revoked_at TIMESTAMPTZ,
    version BIGINT NOT NULL DEFAULT 1,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    CHECK (status IN ('online','offline','draining','disabled','revoked'))
);

ALTER TABLE commands ADD COLUMN runner_id TEXT REFERENCES runners(id);
ALTER TABLE commands ADD COLUMN assignment_sequence BIGINT;
ALTER TABLE commands ADD COLUMN deadline TIMESTAMPTZ NOT NULL DEFAULT (now() + interval '1 hour');
ALTER TABLE commands ADD COLUMN required_capability JSONB;
CREATE UNIQUE INDEX commands_runner_sequence_unique ON commands(runner_id, assignment_sequence) WHERE runner_id IS NOT NULL;

CREATE TABLE runner_source_events (
    runner_id TEXT NOT NULL REFERENCES runners(id) ON DELETE CASCADE,
    source_event_id TEXT NOT NULL,
    outbox_sequence BIGINT NOT NULL,
    event_cursor BIGINT REFERENCES events(cursor),
    received_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (runner_id, source_event_id),
    UNIQUE (runner_id, outbox_sequence)
);
