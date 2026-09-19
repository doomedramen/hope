-- Milestone 4: monitor records, check results, and incidents.
-- This migration stores approved monitor intent and health history. A later
-- worker slice owns leases, protocol checks, state transitions, and delivery.

create table if not exists monitors (
    id                    uuid primary key default gen_random_uuid(),
    proposal_id           uuid unique references monitor_proposals(id),
    service_id            uuid not null references services(id),
    endpoint_id           uuid not null references endpoints(id),
    monitor_type          text not null check (monitor_type in
                              ('icmp', 'tcp', 'http', 'https', 'dns', 'tls')),
    config                jsonb not null default '{}'::jsonb,
    interval_seconds      integer not null default 60 check (interval_seconds between 5 and 86400),
    timeout_ms            integer not null default 5000 check (timeout_ms between 50 and 60000),
    failure_threshold     integer not null default 2 check (failure_threshold between 1 and 20),
    recovery_threshold    integer not null default 2 check (recovery_threshold between 1 and 20),
    enabled               boolean not null default true,
    state                 text not null default 'unknown'
                              check (state in ('unknown', 'up', 'degraded', 'down', 'stale')),
    consecutive_failures  integer not null default 0 check (consecutive_failures >= 0),
    consecutive_successes integer not null default 0 check (consecutive_successes >= 0),
    last_result_at        timestamptz,
    last_success_at       timestamptz,
    last_failure_at       timestamptz,
    next_run_at           timestamptz,
    lease_owner           text,
    lease_expires_at      timestamptz,
    version               integer not null default 1,
    created_by            uuid references users(id),
    created_at            timestamptz not null default now(),
    updated_at            timestamptz not null default now()
);

create index if not exists monitors_enabled_due_idx
    on monitors (next_run_at, id) where enabled;
create index if not exists monitors_service_idx
    on monitors (service_id, created_at desc);

create table if not exists monitor_results (
    id             uuid primary key default gen_random_uuid(),
    monitor_id     uuid not null references monitors(id),
    status         text not null check (status in ('success', 'failure', 'timeout', 'error')),
    observed_at    timestamptz not null default now(),
    latency_ms     integer check (latency_ms >= 0),
    error          text,
    details        jsonb not null default '{}'::jsonb,
    created_at     timestamptz not null default now()
);

create index if not exists monitor_results_monitor_time_idx
    on monitor_results (monitor_id, observed_at desc, id desc);

create table if not exists incidents (
    id             uuid primary key default gen_random_uuid(),
    monitor_id     uuid not null references monitors(id),
    state          text not null default 'open' check (state in ('open', 'recovered')),
    severity       text not null default 'critical'
                     check (severity in ('warning', 'critical')),
    opened_at      timestamptz not null default now(),
    recovered_at   timestamptz,
    last_event_at  timestamptz not null default now(),
    failure_count  integer not null default 1 check (failure_count >= 1),
    last_result_id uuid references monitor_results(id),
    summary        text,
    created_at     timestamptz not null default now(),
    updated_at     timestamptz not null default now(),
    constraint incidents_recovery_consistent check (
        (state = 'open' and recovered_at is null)
        or (state = 'recovered' and recovered_at is not null)
    )
);

create unique index if not exists incidents_one_open_per_monitor_idx
    on incidents (monitor_id) where state = 'open';
create index if not exists incidents_monitor_time_idx
    on incidents (monitor_id, opened_at desc, id desc);
