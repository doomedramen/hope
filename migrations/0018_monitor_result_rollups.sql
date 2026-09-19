-- M4: compact older monitor observations before raw-result retention.

alter table monitor_results
    add column if not exists rolled_up_at timestamptz;

create index if not exists monitor_results_rollup_idx
    on monitor_results (rolled_up_at, observed_at, id)
    where rolled_up_at is null;

create table if not exists monitor_result_rollups (
    monitor_id      uuid not null references monitors(id) on delete cascade,
    bucket_start    timestamptz not null,
    sample_count    bigint not null check (sample_count > 0),
    success_count   bigint not null check (success_count >= 0),
    failure_count   bigint not null check (failure_count >= 0),
    timeout_count   bigint not null check (timeout_count >= 0),
    error_count     bigint not null check (error_count >= 0),
    min_latency_ms  integer,
    avg_latency_ms  double precision,
    max_latency_ms  integer,
    last_status     text not null check (last_status in ('success', 'failure', 'timeout', 'error')),
    created_at      timestamptz not null default now(),
    updated_at      timestamptz not null default now(),
    primary key (monitor_id, bucket_start)
);

create index if not exists monitor_result_rollups_time_idx
    on monitor_result_rollups (bucket_start desc, monitor_id);
