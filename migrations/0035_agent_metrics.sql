-- Host resource telemetry. Samples are authenticated by the agent gateway;
-- sample_id makes reconnect/replay safe without trusting a client timestamp.
create table if not exists agent_metric_samples (
    id                 uuid primary key default gen_random_uuid(),
    agent_id           uuid not null references agents(id) on delete cascade,
    batch_id           uuid not null,
    sample_id          uuid not null,
    schema_version     integer not null check (schema_version > 0),
    collected_at       timestamptz not null,
    received_at        timestamptz not null default now(),
    metrics            jsonb not null,
    constraint agent_metric_samples_metrics_object
        check (jsonb_typeof(metrics) = 'object'),
    constraint agent_metric_samples_metrics_bounded
        check (octet_length(metrics::text) <= 32768),
    constraint agent_metric_samples_identity_unique
        unique (agent_id, sample_id)
);

create index if not exists agent_metric_samples_agent_time_idx
    on agent_metric_samples (agent_id, collected_at desc, sample_id desc);

create index if not exists agent_metric_samples_received_idx
    on agent_metric_samples (received_at desc, id desc);
