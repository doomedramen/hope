-- Per-range schedule for background scans (minutes), and open notification alerts.
ALTER TABLE networks ADD COLUMN interval_minutes INTEGER NOT NULL DEFAULT 5;
CREATE TABLE IF NOT EXISTS alerts (
    key TEXT PRIMARY KEY,
    host_id TEXT NOT NULL,
    kind TEXT NOT NULL,
    message TEXT NOT NULL,
    opened_at REAL NOT NULL
);
