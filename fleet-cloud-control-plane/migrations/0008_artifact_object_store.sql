ALTER TABLE artifact_objects
    ALTER COLUMN ciphertext DROP NOT NULL,
    ADD COLUMN storage_backend TEXT NOT NULL DEFAULT 'postgres';

ALTER TABLE artifact_objects
    ADD CONSTRAINT artifact_objects_storage_backend_check
    CHECK (
        (storage_backend = 'postgres' AND ciphertext IS NOT NULL)
        OR (storage_backend = 's3' AND ciphertext IS NULL)
    );
