CREATE INDEX IF NOT EXISTS device_relationships_source_latest
    ON device_relationships(source_device_id,kind,observed_at DESC);
