-- Retention deletes are time-bounded across all sources. Existing indexes start
-- with source/device ownership columns and cannot seek by observed_at alone.
CREATE INDEX IF NOT EXISTS source_observations_expiry
    ON source_observations(observed_at);
CREATE INDEX IF NOT EXISTS child_batches_expiry
    ON child_batches(observed_at);
