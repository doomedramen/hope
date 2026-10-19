-- Summaries make late source samples mergeable without retaining every 30-second row forever.
CREATE TABLE IF NOT EXISTS source_metric_rollups (
    source_device_id TEXT NOT NULL,
    bucket INTEGER NOT NULL,
    statistics TEXT NOT NULL,
    PRIMARY KEY(source_device_id,bucket)
);
CREATE INDEX IF NOT EXISTS source_metric_rollups_bucket
    ON source_metric_rollups(bucket);
