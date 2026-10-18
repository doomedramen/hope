-- Parent-reported devices are stored independently from scan hosts and agent metrics. This
-- preserves source identity, completeness, and source-specific measurements across restarts.
CREATE TABLE IF NOT EXISTS child_batch_heads (
    parent_agent_id TEXT NOT NULL,
    source TEXT NOT NULL CHECK(source IN ('docker','proxmox')),
    source_scope TEXT NOT NULL,
    sequence INTEGER NOT NULL DEFAULT 0,
    inventory_generation INTEGER NOT NULL DEFAULT 0,
    observed_at REAL,
    complete_generation INTEGER NOT NULL DEFAULT 0,
    complete_observed_at REAL,
    PRIMARY KEY(parent_agent_id,source,source_scope)
);

CREATE TABLE IF NOT EXISTS child_batches (
    batch_id TEXT PRIMARY KEY,
    parent_agent_id TEXT NOT NULL,
    source TEXT NOT NULL CHECK(source IN ('docker','proxmox')),
    source_scope TEXT NOT NULL,
    sequence INTEGER NOT NULL,
    inventory_generation INTEGER NOT NULL,
    observed_at REAL NOT NULL,
    complete INTEGER NOT NULL CHECK(complete IN (0,1)),
    received_at REAL NOT NULL,
    UNIQUE(parent_agent_id,source,source_scope,sequence)
);
CREATE INDEX IF NOT EXISTS child_batches_source_time
    ON child_batches(parent_agent_id,source,source_scope,observed_at);

CREATE TABLE IF NOT EXISTS source_devices (
    id TEXT PRIMARY KEY,
    parent_agent_id TEXT NOT NULL,
    source TEXT NOT NULL CHECK(source IN ('docker','proxmox')),
    source_scope TEXT NOT NULL,
    source_key TEXT NOT NULL,
    incarnation TEXT NOT NULL,
    kind TEXT NOT NULL CHECK(kind IN ('docker_container','proxmox_vm','proxmox_lxc')),
    host_id TEXT,
    lifecycle TEXT NOT NULL CHECK(lifecycle IN ('running','stopped','stale','archived')),
    name TEXT NOT NULL,
    observed_at REAL NOT NULL,
    inventory_generation INTEGER NOT NULL,
    missing_complete_count INTEGER NOT NULL DEFAULT 0,
    missing_since REAL,
    missing_inventory_generation INTEGER NOT NULL DEFAULT 0,
    body TEXT NOT NULL,
    UNIQUE(parent_agent_id,source,source_scope,source_key,incarnation)
);
CREATE INDEX IF NOT EXISTS source_devices_parent
    ON source_devices(parent_agent_id,source,source_scope,lifecycle);
CREATE INDEX IF NOT EXISTS source_devices_host ON source_devices(host_id);

CREATE TABLE IF NOT EXISTS source_observations (
    source_device_id TEXT NOT NULL,
    batch_id TEXT NOT NULL,
    observed_at REAL NOT NULL,
    inventory_generation INTEGER NOT NULL,
    body TEXT NOT NULL,
    PRIMARY KEY(source_device_id,batch_id)
);
CREATE INDEX IF NOT EXISTS source_observations_time
    ON source_observations(source_device_id,observed_at);

CREATE TABLE IF NOT EXISTS source_metrics (
    source_device_id TEXT NOT NULL,
    report_id TEXT NOT NULL,
    observed_at REAL NOT NULL,
    resolution INTEGER NOT NULL DEFAULT 30,
    body TEXT NOT NULL,
    PRIMARY KEY(source_device_id,report_id)
);
CREATE INDEX IF NOT EXISTS source_metrics_time
    ON source_metrics(source_device_id,observed_at);

CREATE TABLE IF NOT EXISTS device_associations (
    source_device_id TEXT NOT NULL,
    host_id TEXT NOT NULL,
    status TEXT NOT NULL CHECK(status IN ('suggested','confirmed','rejected')),
    reason TEXT NOT NULL,
    evidence TEXT NOT NULL,
    evidence_hash TEXT NOT NULL,
    updated_at REAL NOT NULL,
    PRIMARY KEY(source_device_id,host_id)
);
CREATE INDEX IF NOT EXISTS device_associations_host ON device_associations(host_id,status);

CREATE TABLE IF NOT EXISTS device_relationships (
    from_device_id TEXT NOT NULL,
    to_device_id TEXT NOT NULL,
    kind TEXT NOT NULL CHECK(kind IN ('runs_on','connected_to','exposes_service')),
    detail TEXT,
    source_device_id TEXT NOT NULL,
    observed_at REAL NOT NULL,
    PRIMARY KEY(from_device_id,to_device_id,kind,source_device_id)
);
CREATE INDEX IF NOT EXISTS device_relationships_to ON device_relationships(to_device_id,kind);
