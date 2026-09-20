-- Milestone 7: centrally managed, signed agent releases and update state.
-- Release binaries remain in the server-side release repository; PostgreSQL
-- stores verified metadata, policy, and the durable progress needed by the
-- API/UI to explain an update without exposing binary contents.

create table if not exists agent_release_versions (
    version                 text primary key check (char_length(version) between 1 and 128),
    manifest                jsonb not null,
    manifest_sha256         text not null check (manifest_sha256 ~ '^[0-9a-f]{64}$'),
    signing_key_fingerprint  text not null check (signing_key_fingerprint ~ '^[0-9a-f]{64}$'),
    imported_at              timestamptz not null default now(),
    last_seen_at             timestamptz not null default now(),
    enabled                  boolean not null default true,
    created_at               timestamptz not null default now(),
    updated_at               timestamptz not null default now()
);

create index if not exists agent_release_versions_enabled_idx
    on agent_release_versions (enabled, version desc);

create table if not exists agent_update_policies (
    agent_id                 uuid primary key references agents(id) on delete cascade,
    mode                     text not null default 'manual'
                             check (mode in ('manual', 'notify', 'automatic')),
    channel                  text not null default 'stable'
                             check (channel in ('stable', 'canary')),
    pinned_version           text,
    rollout_percent          integer not null default 100
                             check (rollout_percent between 0 and 100),
    device_id                uuid not null references devices(id),
    host                     text not null check (char_length(host) between 1 and 255),
    port                     integer not null check (port between 1 and 65535),
    credential_id            uuid not null references credentials(id),
    repair_on_failure        boolean not null default true,
    updated_by               uuid references users(id),
    created_at               timestamptz not null default now(),
    updated_at               timestamptz not null default now()
);

create index if not exists agent_update_policies_mode_idx
    on agent_update_policies (mode, channel, updated_at desc);

create table if not exists agent_update_operations (
    id                       uuid primary key default gen_random_uuid(),
    agent_id                 uuid not null references agents(id) on delete cascade,
    job_id                   uuid unique references jobs(id) on delete set null,
    device_id                uuid not null references devices(id),
    target_version           text not null check (char_length(target_version) between 1 and 128),
    previous_version         text,
    state                    text not null default 'pending'
                             check (state in (
                                 'pending', 'verifying', 'installing', 'restarting',
                                 'succeeded', 'failed', 'rolled_back', 'pinned', 'incompatible'
                             )),
    progress                 jsonb not null default '{}'::jsonb,
    last_error               text,
    rollback_reason          text,
    deadline_at              timestamptz,
    started_at               timestamptz,
    finished_at              timestamptz,
    created_by               uuid references users(id),
    created_at               timestamptz not null default now(),
    updated_at               timestamptz not null default now()
);

create index if not exists agent_update_operations_agent_time_idx
    on agent_update_operations (agent_id, created_at desc, id desc);

create unique index if not exists agent_update_operations_one_active_idx
    on agent_update_operations (agent_id)
    where state in ('pending', 'verifying', 'installing', 'restarting');
