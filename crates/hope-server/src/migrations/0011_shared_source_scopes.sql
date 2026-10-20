CREATE TABLE IF NOT EXISTS shared_source_scopes (
    source TEXT NOT NULL,
    source_scope TEXT NOT NULL,
    token_hash TEXT NOT NULL,
    created_at REAL NOT NULL,
    PRIMARY KEY(source,source_scope)
);
ALTER TABLE source_devices ADD COLUMN ownership_generation INTEGER NOT NULL DEFAULT 0;
CREATE TABLE IF NOT EXISTS device_id_aliases (
    alias_id TEXT PRIMARY KEY,
    survivor_id TEXT NOT NULL
);
