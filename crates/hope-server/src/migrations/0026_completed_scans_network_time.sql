CREATE INDEX IF NOT EXISTS scans_completed_network_time
    ON scans(network_id,finished_at DESC)
    WHERE status='completed';
