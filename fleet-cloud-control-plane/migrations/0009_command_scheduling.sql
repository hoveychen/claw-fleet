ALTER TABLE commands
    ADD COLUMN runner_pool_id TEXT REFERENCES runner_pools(id);

CREATE INDEX commands_unassigned_scheduling_idx
    ON commands(organization_id, project_id, runner_pool_id, created_at, id)
    WHERE status = 'pending' AND runner_id IS NULL;
