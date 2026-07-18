CREATE TABLE organizations (
    id TEXT PRIMARY KEY,
    name TEXT NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE TABLE projects (
    id TEXT PRIMARY KEY,
    organization_id TEXT NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
    slug TEXT NOT NULL,
    name TEXT NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (organization_id, slug)
);

CREATE TABLE api_keys (
    id TEXT PRIMARY KEY,
    organization_id TEXT NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
    project_id TEXT NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
    name TEXT NOT NULL,
    key_prefix TEXT NOT NULL,
    key_hash BYTEA NOT NULL UNIQUE,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    last_used_at TIMESTAMPTZ,
    last_used_ip INET,
    revoked_at TIMESTAMPTZ
);

CREATE INDEX api_keys_prefix_idx ON api_keys(key_prefix) WHERE revoked_at IS NULL;

CREATE TABLE tasks (
    id TEXT PRIMARY KEY,
    organization_id TEXT NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
    project_id TEXT NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
    external_id TEXT,
    title TEXT,
    goal TEXT NOT NULL,
    workspace JSONB NOT NULL,
    status TEXT NOT NULL,
    active_run_id TEXT,
    metadata JSONB NOT NULL DEFAULT '{}'::jsonb,
    version BIGINT NOT NULL DEFAULT 1 CHECK (version >= 1),
    created_by_type TEXT NOT NULL,
    created_by_id TEXT NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    CHECK (status IN ('queued','running','waiting_input','paused','succeeded','failed','cancelled'))
);

CREATE UNIQUE INDEX tasks_external_id_unique
    ON tasks(organization_id, project_id, external_id)
    WHERE external_id IS NOT NULL;
CREATE INDEX tasks_project_updated_idx ON tasks(project_id, updated_at DESC, id DESC);

CREATE TABLE runs (
    id TEXT PRIMARY KEY,
    organization_id TEXT NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
    project_id TEXT NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
    task_id TEXT NOT NULL REFERENCES tasks(id) ON DELETE CASCADE,
    attempt INTEGER NOT NULL CHECK (attempt >= 1),
    predecessor_run_id TEXT REFERENCES runs(id),
    runner_id TEXT,
    status TEXT NOT NULL,
    provider TEXT NOT NULL,
    model TEXT,
    effort TEXT,
    permission_policy_id TEXT,
    started_at TIMESTAMPTZ,
    finished_at TIMESTAMPTZ,
    exit_reason TEXT,
    version BIGINT NOT NULL DEFAULT 1 CHECK (version >= 1),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (task_id, attempt),
    CHECK (status IN ('assigned','starting','running','waiting_input','stopping','succeeded','failed','cancelled','lost')),
    CHECK (provider IN ('claude_code','codex'))
);

ALTER TABLE tasks
    ADD CONSTRAINT tasks_active_run_fk
    FOREIGN KEY (active_run_id) REFERENCES runs(id) DEFERRABLE INITIALLY DEFERRED;

CREATE TABLE events (
    cursor BIGSERIAL PRIMARY KEY,
    id TEXT NOT NULL UNIQUE,
    organization_id TEXT NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
    project_id TEXT NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
    task_id TEXT REFERENCES tasks(id) ON DELETE CASCADE,
    run_id TEXT REFERENCES runs(id) ON DELETE CASCADE,
    event_type TEXT NOT NULL,
    task_sequence BIGINT,
    occurred_at TIMESTAMPTZ NOT NULL,
    recorded_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    data JSONB NOT NULL,
    schema_version INTEGER NOT NULL DEFAULT 1 CHECK (schema_version >= 1),
    UNIQUE (task_id, task_sequence),
    CHECK (task_sequence IS NULL OR task_sequence >= 1)
);

CREATE INDEX events_project_cursor_idx ON events(project_id, cursor);
CREATE INDEX events_task_sequence_idx ON events(task_id, task_sequence) WHERE task_id IS NOT NULL;

CREATE TABLE commands (
    id TEXT PRIMARY KEY,
    organization_id TEXT NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
    project_id TEXT NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
    task_id TEXT REFERENCES tasks(id) ON DELETE CASCADE,
    run_id TEXT REFERENCES runs(id) ON DELETE CASCADE,
    command_type TEXT NOT NULL,
    status TEXT NOT NULL DEFAULT 'pending',
    payload JSONB NOT NULL,
    expected_version BIGINT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    accepted_at TIMESTAMPTZ,
    completed_at TIMESTAMPTZ,
    error_code TEXT,
    CHECK (status IN ('pending','accepted','completed','rejected','failed'))
);

CREATE INDEX commands_pending_idx ON commands(project_id, created_at) WHERE status = 'pending';

CREATE TABLE audit_records (
    id BIGSERIAL PRIMARY KEY,
    organization_id TEXT NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
    project_id TEXT REFERENCES projects(id) ON DELETE CASCADE,
    principal_type TEXT NOT NULL,
    principal_id TEXT NOT NULL,
    request_id TEXT NOT NULL,
    action TEXT NOT NULL,
    resource_type TEXT NOT NULL,
    resource_id TEXT,
    details JSONB NOT NULL DEFAULT '{}'::jsonb,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
