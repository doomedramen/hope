-- Settings for source-only devices must not create a second hosts record.
CREATE TABLE source_device_settings (
    source_device_id TEXT PRIMARY KEY,
    body TEXT NOT NULL
);
ALTER TABLE device_associations ADD COLUMN manual INTEGER NOT NULL DEFAULT 0 CHECK(manual IN (0,1));
UPDATE device_associations SET manual=1 WHERE status='rejected' OR reason='Manually confirmed';
