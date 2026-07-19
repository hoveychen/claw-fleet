CREATE TABLE decisions (
    id TEXT PRIMARY KEY,
    organization_id TEXT NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
    project_id TEXT NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
    task_id TEXT NOT NULL REFERENCES tasks(id) ON DELETE CASCADE,
    run_id TEXT NOT NULL REFERENCES runs(id) ON DELETE CASCADE,
    runner_id TEXT NOT NULL REFERENCES runners(id),
    source_decision_id TEXT NOT NULL,
    kind TEXT NOT NULL,
    status TEXT NOT NULL DEFAULT 'pending',
    payload JSONB NOT NULL,
    response_schema JSONB NOT NULL DEFAULT '{}'::jsonb,
    response JSONB,
    response_principal_id TEXT,
    deadline TIMESTAMPTZ,
    schema_version INTEGER NOT NULL DEFAULT 1 CHECK (schema_version >= 1),
    version BIGINT NOT NULL DEFAULT 1 CHECK (version >= 1),
    created_at TIMESTAMPTZ NOT NULL,
    updated_at TIMESTAMPTZ NOT NULL,
    resolved_at TIMESTAMPTZ,
    UNIQUE (runner_id, source_decision_id),
    CHECK (kind IN ('guard','elicitation','fleet_ask','plan_approval','permission_prompt','a2ui')),
    CHECK (status IN ('pending','answer_queued','answered','declined','cancelled','expired'))
);

CREATE INDEX decisions_task_pending_idx
    ON decisions(task_id, created_at, id) WHERE status IN ('pending','answer_queued');
