alter table agent_metric_samples add column rolled_up_at timestamptz;
create index agent_metrics_unrolled on agent_metric_samples(received_at,id) where rolled_up_at is null;
create table agent_metric_rollups (
    agent_id uuid not null references agents(id) on delete cascade,
    resolution integer not null check (resolution in (300,3600)),
    bucket_start timestamptz not null,
    sample_count bigint not null,
    stats jsonb not null,
    primary key(agent_id,resolution,bucket_start)
);
