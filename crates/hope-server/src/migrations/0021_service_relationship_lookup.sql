CREATE INDEX IF NOT EXISTS device_relationships_kind_source
    ON device_relationships(kind,source_device_id);
