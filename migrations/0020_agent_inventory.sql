-- Milestone 5: agent metadata, bounded inventory snapshots, and heartbeat health.
-- Agent certificates remain the authentication identity. These columns and
-- tables persist the application identity and its latest inside-the-host view
-- without replacing or deleting the canonical inventory graph.

alter table agents
    add column if not exists agent_version text not null default '',
    add column if not exists os text not null default '',
    add column if not exists arch text not null default '',
    add column if not exists protocol_version integer not null default 1,
    add column if not exists capabilities jsonb not null default '[]'::jsonb,
    add column if not exists last_heartbeat_at timestamptz,
    add column if not exists heartbeat_timeout_seconds integer not null default 90;

alter table agents
    add constraint agents_protocol_version_positive
        check (protocol_version > 0),
    add constraint agents_heartbeat_timeout_bounded
        check (heartbeat_timeout_seconds between 30 and 86_400);

create table if not exists agent_inventory_snapshots (
    id                    uuid primary key default gen_random_uuid(),
    agent_id              uuid not null references agents(id) on delete cascade,
    message_id            uuid not null,
    protocol_version      integer not null check (protocol_version > 0),
    sequence              bigint not null check (sequence >= 0),
    collected_at          timestamptz not null,
    received_at           timestamptz not null default now(),
    inventory             jsonb not null,
    complete              boolean not null default true,
    constraint agent_inventory_snapshots_message_unique
        unique (agent_id, message_id),
    constraint agent_inventory_snapshots_sequence_unique
        unique (agent_id, sequence)
);

create index if not exists agent_inventory_snapshots_agent_time_idx
    on agent_inventory_snapshots (agent_id, collected_at desc, id desc);

-- A separate current projection keeps reads cheap while the history table
-- retains recent snapshots for replay/debugging. Application code never
-- removes the row referenced here during bounded history cleanup.
create table if not exists agent_inventory_current (
    agent_id              uuid primary key references agents(id) on delete cascade,
    snapshot_id           uuid not null unique references agent_inventory_snapshots(id),
    message_id            uuid not null,
    protocol_version      integer not null check (protocol_version > 0),
    sequence              bigint not null check (sequence >= 0),
    collected_at          timestamptz not null,
    received_at           timestamptz not null default now(),
    inventory             jsonb not null,
    complete              boolean not null default true
);

create index if not exists agent_inventory_current_collected_idx
    on agent_inventory_current (collected_at desc, agent_id);

-- Durable liveness state is separate from M4 monitor incidents because an
-- enrolled agent has no required service/endpoint monitor yet. One open
-- incident per agent is enforced, and recovery never touches inventory rows.
create table if not exists agent_health_incidents (
    id             uuid primary key default gen_random_uuid(),
    agent_id       uuid not null references agents(id) on delete cascade,
    state          text not null default 'open'
                     check (state in ('open', 'recovered')),
    severity       text not null default 'warning'
                     check (severity in ('warning', 'critical')),
    opened_at      timestamptz not null default now(),
    recovered_at   timestamptz,
    last_event_at  timestamptz not null default now(),
    failure_count  integer not null default 1 check (failure_count >= 1),
    summary        text,
    details        jsonb not null default '{}'::jsonb,
    created_at     timestamptz not null default now(),
    updated_at     timestamptz not null default now(),
    constraint agent_health_incidents_recovery_consistent check (
        (state = 'open' and recovered_at is null)
        or (state = 'recovered' and recovered_at is not null)
    )
);

create unique index if not exists agent_health_incidents_one_open_idx
    on agent_health_incidents (agent_id) where state = 'open';

create index if not exists agent_health_incidents_agent_time_idx
    on agent_health_incidents (agent_id, opened_at desc, id desc);

-- Source keys make repeated agent reports update canonical rows instead of
-- creating duplicate workloads/services/endpoints. Existing M1 rows remain
-- valid and continue to use null source keys.
alter table interfaces
    add column if not exists source_type text not null default 'unknown',
    add column if not exists agent_source_key text;
alter table addresses
    add column if not exists source_type text not null default 'unknown';
alter table workloads
    add column if not exists source_type text not null default 'unknown',
    add column if not exists agent_source_key text;
alter table services
    add column if not exists source_type text not null default 'unknown',
    add column if not exists agent_source_key text;
alter table endpoints
    add column if not exists source_type text not null default 'unknown',
    add column if not exists agent_source_key text,
    add column if not exists reachability_state text not null default 'unknown',
    add column if not exists reachability_checked_at timestamptz,
    add column if not exists reachability_worker_id text;

create unique index if not exists workloads_agent_source_unique
    on workloads (host_device_id, agent_source_key)
    where agent_source_key is not null;

create unique index if not exists interfaces_agent_source_unique
    on interfaces (device_id, agent_source_key)
    where agent_source_key is not null;

create unique index if not exists services_agent_source_unique
    on services (owner_kind, owner_id, agent_source_key)
    where agent_source_key is not null;

create unique index if not exists endpoints_agent_source_unique
    on endpoints (service_id, agent_source_key)
    where agent_source_key is not null;
