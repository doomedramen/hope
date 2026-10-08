ALTER TABLE metrics ADD COLUMN statistics TEXT;
ALTER TABLE enrollment_tokens ADD COLUMN reset_agent_id TEXT;
CREATE INDEX metrics_cleanup ON metrics(resolution,collected_at);
CREATE INDEX metrics_expiry ON metrics(collected_at);
CREATE INDEX scans_expiry ON scans(finished_at);
