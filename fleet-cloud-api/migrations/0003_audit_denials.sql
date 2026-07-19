CREATE TABLE audit_denials (
    id uuid PRIMARY KEY,
    requested_organization_id uuid NOT NULL,
    requested_project_id uuid NOT NULL,
    resource_type text NOT NULL,
    resource_id uuid NOT NULL,
    reason text NOT NULL,
    occurred_at timestamptz NOT NULL DEFAULT now()
);

CREATE INDEX audit_denials_scope_time_idx
    ON audit_denials(requested_organization_id, requested_project_id, occurred_at DESC);
