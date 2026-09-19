-- Milestone 5: bounded raw observations reported by enrolled agents.
-- Observation identity is scoped to the authenticated agent. The server keeps
-- recent rows for replay/debugging and bounds the retained set at ingest time.

create table if not exists agent_observations (
    id                uuid primary key default gen_random_uuid(),
    agent_id          uuid not null references agents(id) on delete cascade,
    batch_id          uuid not null,
    idempotency_key   text not null,
    observation_key   text not null,
    source            text not null,
    protocol_version  integer not null check (protocol_version > 0),
    collected_at      timestamptz not null,
    observed_at       timestamptz not null,
    state             text not null check (state in ('ok', 'degraded', 'unavailable')),
    value             jsonb not null,
    received_at       timestamptz not null default now(),
    constraint agent_observations_idempotency_unique
        unique (agent_id, idempotency_key)
);

create index if not exists agent_observations_agent_received_idx
    on agent_observations (agent_id, received_at desc, id desc);

create index if not exists agent_observations_agent_observed_idx
    on agent_observations (agent_id, observed_at desc, id desc);
