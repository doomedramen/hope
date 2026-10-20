CREATE TABLE IF NOT EXISTS notification_outbox (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    alert_key TEXT NOT NULL,
    host_id TEXT,
    kind TEXT NOT NULL,
    recovery INTEGER NOT NULL DEFAULT 0,
    destination TEXT NOT NULL,
    body TEXT NOT NULL,
    attempts INTEGER NOT NULL DEFAULT 0,
    next_attempt REAL NOT NULL,
    created_at REAL NOT NULL
);
CREATE INDEX IF NOT EXISTS notification_outbox_due ON notification_outbox(next_attempt,id);
CREATE INDEX IF NOT EXISTS notification_outbox_order ON notification_outbox(alert_key,id);
