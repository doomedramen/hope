CREATE TABLE pending_legacy_identities (
    source TEXT NOT NULL,
    parent_agent_id TEXT NOT NULL,
    legacy_source_device_id TEXT NOT NULL,
    source_scope TEXT NOT NULL,
    source_key TEXT NOT NULL,
    canonical_incarnation TEXT NOT NULL,
    PRIMARY KEY(source, parent_agent_id, legacy_source_device_id, source_scope, source_key, canonical_incarnation)
);
CREATE INDEX pending_legacy_identity_target
    ON pending_legacy_identities(source, source_scope, source_key, canonical_incarnation);
