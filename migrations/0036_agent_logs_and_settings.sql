-- Bounded agent source configuration and idempotent searchable log history.
create table agent_collection_settings (
    agent_id uuid primary key references agents(id) on delete cascade,
    revision bigint not null default 0,
    config jsonb not null default '{"journal_units":[],"docker_containers":[],"redact":[]}'::jsonb,
    applied_revision bigint,
    applied_at timestamptz,
    log_retention_days integer not null default 7 check (log_retention_days between 1 and 90),
    updated_at timestamptz not null default now(),
    updated_by uuid references users(id)
);
create table agent_log_entries (
    id uuid primary key default gen_random_uuid(),
    agent_id uuid not null references agents(id) on delete cascade,
    event_id text not null check (length(event_id) between 1 and 256),
    observed_at timestamptz not null,
    received_at timestamptz not null default now(),
    source text not null check (length(source) between 1 and 256),
    severity text not null,
    message text not null check (octet_length(message) <= 8192),
    attributes jsonb not null check (jsonb_typeof(attributes) = 'object' and octet_length(attributes::text) <= 32768),
    unique(agent_id, event_id)
);
create index agent_logs_agent_received on agent_log_entries(agent_id, received_at desc, id desc);
create index agent_logs_source_time on agent_log_entries(agent_id, source, observed_at desc);
create index agent_logs_observed on agent_log_entries(agent_id, observed_at desc);
create index agent_logs_search on agent_log_entries using gin(to_tsvector('simple', message));
