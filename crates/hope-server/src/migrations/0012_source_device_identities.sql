CREATE TABLE IF NOT EXISTS source_device_identities (
    source TEXT NOT NULL,
    identity_owner TEXT NOT NULL,
    source_scope TEXT NOT NULL,
    source_key TEXT NOT NULL,
    incarnation TEXT NOT NULL,
    source_device_id TEXT NOT NULL,
    PRIMARY KEY(source,identity_owner,source_scope,source_key,incarnation)
);
CREATE INDEX IF NOT EXISTS source_device_identities_device ON source_device_identities(source_device_id);
INSERT OR IGNORE INTO source_device_identities(source,identity_owner,source_scope,source_key,incarnation,source_device_id)
SELECT source,parent_agent_id,source_scope,source_key,incarnation,id FROM source_devices;
