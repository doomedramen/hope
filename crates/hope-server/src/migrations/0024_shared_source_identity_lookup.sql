CREATE INDEX IF NOT EXISTS source_devices_shared_identity
    ON source_devices(
        source,
        source_scope,
        source_key,
        incarnation,
        ownership_generation DESC,
        observed_at DESC,
        id,
        parent_agent_id
    );
