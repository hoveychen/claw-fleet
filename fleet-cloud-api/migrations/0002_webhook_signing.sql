ALTER TABLE webhook_endpoints
    ADD COLUMN signing_secret text NOT NULL DEFAULT '';

ALTER TABLE webhook_endpoints
    ALTER COLUMN signing_secret DROP DEFAULT;
