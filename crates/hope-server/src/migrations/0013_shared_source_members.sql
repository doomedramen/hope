CREATE TABLE IF NOT EXISTS shared_source_members (
    source TEXT NOT NULL,
    source_scope TEXT NOT NULL,
    agent_id TEXT NOT NULL,
    joined_at REAL NOT NULL,
    PRIMARY KEY(source,source_scope,agent_id)
);
