CREATE TABLE IF NOT EXISTS source_device_aliases (
    alias_id TEXT PRIMARY KEY,
    survivor_id TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS source_device_aliases_survivor ON source_device_aliases(survivor_id);
