CREATE INDEX IF NOT EXISTS source_metrics_rollup_groups
    ON source_metrics(resolution,CAST(observed_at/300 AS INTEGER),source_device_id);
