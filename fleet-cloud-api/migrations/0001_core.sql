CREATE TABLE organizations (
    id uuid PRIMARY KEY,
    name text NOT NULL,
    created_at timestamptz NOT NULL DEFAULT now()
);

CREATE TABLE projects (
    id uuid PRIMARY KEY,
    organization_id uuid NOT NULL REFERENCES organizations(id),
    name text NOT NULL,
    created_at timestamptz NOT NULL DEFAULT now(),
    UNIQUE (organization_id, name)
);

CREATE TABLE runners (
    id uuid PRIMARY KEY,
    organization_id uuid NOT NULL REFERENCES organizations(id),
    project_id uuid NOT NULL REFERENCES projects(id),
    display_name text NOT NULL,
    runner_version text NOT NULL,
    protocol_versions jsonb NOT NULL DEFAULT '[]'::jsonb,
    capabilities jsonb NOT NULL DEFAULT '[]'::jsonb,
    labels jsonb NOT NULL DEFAULT '{}'::jsonb,
    connection_state text NOT NULL DEFAULT 'offline',
    last_heartbeat_at timestamptz,
    revoked_at timestamptz,
    created_at timestamptz NOT NULL DEFAULT now()
);

CREATE TABLE workspaces (
    id uuid PRIMARY KEY,
    organization_id uuid NOT NULL REFERENCES organizations(id),
    project_id uuid NOT NULL REFERENCES projects(id),
    runner_id uuid NOT NULL REFERENCES runners(id),
    display_name text NOT NULL,
    locator text NOT NULL,
    labels jsonb NOT NULL DEFAULT '{}'::jsonb,
    created_at timestamptz NOT NULL DEFAULT now(),
    UNIQUE (runner_id, locator)
);

CREATE TABLE tasks (
    id uuid PRIMARY KEY,
    organization_id uuid NOT NULL REFERENCES organizations(id),
    project_id uuid NOT NULL REFERENCES projects(id),
    external_id text,
    title text,
    prompt text NOT NULL,
    status text NOT NULL CHECK (status IN (
        'queued', 'assigned', 'running', 'waiting_for_input', 'paused',
        'rate_limited', 'succeeded', 'failed', 'cancelled'
    )),
    workspace_selector jsonb NOT NULL,
    agent_profile jsonb NOT NULL,
    metadata jsonb NOT NULL DEFAULT '{}'::jsonb,
    current_attempt_id uuid,
    waiting_decision_count integer NOT NULL DEFAULT 0 CHECK (waiting_decision_count >= 0),
    event_cursor bigint NOT NULL DEFAULT 0 CHECK (event_cursor >= 0),
    version bigint NOT NULL DEFAULT 1 CHECK (version >= 1),
    created_at timestamptz NOT NULL DEFAULT now(),
    updated_at timestamptz NOT NULL DEFAULT now(),
    UNIQUE (project_id, id)
);

CREATE UNIQUE INDEX tasks_project_external_id_unique
    ON tasks(project_id, external_id)
    WHERE external_id IS NOT NULL;

CREATE INDEX tasks_project_status_updated_idx
    ON tasks(project_id, status, updated_at DESC, id DESC);

CREATE TABLE attempts (
    id uuid PRIMARY KEY,
    organization_id uuid NOT NULL REFERENCES organizations(id),
    project_id uuid NOT NULL REFERENCES projects(id),
    task_id uuid NOT NULL REFERENCES tasks(id),
    runner_id uuid REFERENCES runners(id),
    workspace_id uuid REFERENCES workspaces(id),
    agent_source text NOT NULL CHECK (agent_source IN ('claude', 'codex')),
    agent_session_id text,
    ordinal integer NOT NULL CHECK (ordinal >= 1),
    reason text NOT NULL CHECK (reason IN ('initial', 'handoff', 'retry', 'resume', 'recovery')),
    status text NOT NULL CHECK (status IN ('starting', 'running', 'waiting', 'ended', 'lost')),
    pid_ref text,
    exit jsonb,
    started_at timestamptz NOT NULL DEFAULT now(),
    ended_at timestamptz,
    UNIQUE (task_id, ordinal)
);

ALTER TABLE tasks
    ADD CONSTRAINT tasks_current_attempt_fk
    FOREIGN KEY (current_attempt_id) REFERENCES attempts(id);

CREATE TABLE decisions (
    id uuid PRIMARY KEY,
    organization_id uuid NOT NULL REFERENCES organizations(id),
    project_id uuid NOT NULL REFERENCES projects(id),
    task_id uuid NOT NULL REFERENCES tasks(id),
    attempt_id uuid NOT NULL REFERENCES attempts(id),
    kind text NOT NULL CHECK (kind IN (
        'guard', 'elicitation', 'fleet_ask', 'plan_approval',
        'permission_prompt', 'a2ui'
    )),
    blocking boolean NOT NULL,
    schema_version text NOT NULL,
    presentation jsonb NOT NULL,
    status text NOT NULL CHECK (status IN ('open', 'answered', 'declined', 'expired', 'cancelled')),
    response jsonb,
    responded_by jsonb,
    expires_at timestamptz,
    created_at timestamptz NOT NULL DEFAULT now(),
    resolved_at timestamptz
);

CREATE TABLE task_events (
    id uuid PRIMARY KEY,
    organization_id uuid NOT NULL REFERENCES organizations(id),
    project_id uuid NOT NULL REFERENCES projects(id),
    task_id uuid NOT NULL REFERENCES tasks(id),
    attempt_id uuid REFERENCES attempts(id),
    sequence bigint NOT NULL CHECK (sequence >= 1),
    event_type text NOT NULL,
    occurred_at timestamptz NOT NULL,
    recorded_at timestamptz NOT NULL DEFAULT now(),
    producer jsonb NOT NULL,
    runner_id uuid REFERENCES runners(id),
    dedupe_key text,
    schema_version text NOT NULL,
    data jsonb NOT NULL,
    UNIQUE (task_id, sequence)
);

CREATE UNIQUE INDEX task_events_runner_dedupe_unique
    ON task_events(runner_id, dedupe_key)
    WHERE runner_id IS NOT NULL AND dedupe_key IS NOT NULL;

CREATE INDEX task_events_replay_idx ON task_events(task_id, sequence);

CREATE TABLE idempotency_keys (
    project_id uuid NOT NULL REFERENCES projects(id),
    operation text NOT NULL,
    idempotency_key text NOT NULL,
    request_fingerprint bytea NOT NULL,
    resource_id uuid,
    response jsonb,
    created_at timestamptz NOT NULL DEFAULT now(),
    expires_at timestamptz NOT NULL,
    PRIMARY KEY (project_id, operation, idempotency_key)
);

CREATE INDEX idempotency_keys_expiry_idx ON idempotency_keys(expires_at);

CREATE TABLE webhook_endpoints (
    id uuid PRIMARY KEY,
    organization_id uuid NOT NULL REFERENCES organizations(id),
    project_id uuid NOT NULL REFERENCES projects(id),
    url text NOT NULL,
    event_types jsonb NOT NULL DEFAULT '[]'::jsonb,
    enabled boolean NOT NULL DEFAULT true,
    created_at timestamptz NOT NULL DEFAULT now()
);

CREATE TABLE webhook_deliveries (
    id uuid PRIMARY KEY,
    organization_id uuid NOT NULL REFERENCES organizations(id),
    project_id uuid NOT NULL REFERENCES projects(id),
    endpoint_id uuid NOT NULL REFERENCES webhook_endpoints(id),
    event_id uuid NOT NULL REFERENCES task_events(id),
    status text NOT NULL DEFAULT 'pending',
    attempt_count integer NOT NULL DEFAULT 0,
    next_attempt_at timestamptz NOT NULL DEFAULT now(),
    last_status integer,
    last_error text,
    created_at timestamptz NOT NULL DEFAULT now(),
    delivered_at timestamptz,
    UNIQUE (endpoint_id, event_id)
);

CREATE TABLE outbox (
    id uuid PRIMARY KEY,
    organization_id uuid NOT NULL REFERENCES organizations(id),
    project_id uuid NOT NULL REFERENCES projects(id),
    topic text NOT NULL,
    aggregate_id uuid NOT NULL,
    payload jsonb NOT NULL,
    available_at timestamptz NOT NULL DEFAULT now(),
    claimed_at timestamptz,
    completed_at timestamptz,
    created_at timestamptz NOT NULL DEFAULT now()
);

CREATE INDEX outbox_pending_idx
    ON outbox(available_at, created_at)
    WHERE completed_at IS NULL;
