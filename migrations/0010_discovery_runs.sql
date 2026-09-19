-- Milestone 2: durable scan execution and per-port observations.
-- `complete` records whether every planned probe ran. Only complete runs may
-- later create absence evidence for a previously open port.

create table if not exists scan_runs (
    id                  uuid primary key default gen_random_uuid(),
    network_id          uuid not null references networks(id),
    job_id              uuid unique references jobs(id),
    kind                text not null check (kind in ('initial_discovery', 'change_scan', 'full_tcp')),
    status              text not null default 'pending'
                            check (status in ('pending', 'running', 'succeeded', 'failed', 'cancelled')),
    scope_version       integer not null,
    targets_planned     bigint not null check (targets_planned >= 0),
    targets_completed   bigint not null default 0 check (targets_completed >= 0),
    ports_planned       bigint not null check (ports_planned >= 0),
    ports_completed     bigint not null default 0 check (ports_completed >= 0),
    complete            boolean not null default false,
    cancellation_requested boolean not null default false,
    requested_by        uuid references users(id),
    source              text not null default 'operator'
                            check (source in ('operator', 'scheduler', 'system')),
    error               text,
    started_at          timestamptz,
    finished_at         timestamptz,
    created_at          timestamptz not null default now(),
    updated_at          timestamptz not null default now(),
    check (targets_completed <= targets_planned),
    check (ports_completed <= ports_planned),
    check (not complete or (status = 'succeeded'
        and targets_completed = targets_planned
        and ports_completed = ports_planned))
);

create index if not exists scan_runs_network_created_idx
    on scan_runs (network_id, created_at desc);
create index if not exists scan_runs_pending_idx
    on scan_runs (created_at) where status in ('pending', 'running');

create table if not exists port_observations (
    id              uuid primary key default gen_random_uuid(),
    scan_run_id     uuid not null references scan_runs(id),
    device_id       uuid references devices(id),
    address         inet not null,
    port            integer not null check (port between 1 and 65535),
    transport       text not null default 'tcp' check (transport in ('tcp', 'udp')),
    state           text not null check (state in ('open', 'closed', 'filtered', 'open_or_filtered')),
    latency_ms      integer check (latency_ms >= 0),
    error           text,
    observed_at     timestamptz not null default now(),
    created_at      timestamptz not null default now(),
    unique (scan_run_id, address, port, transport)
);

create index if not exists port_observations_address_port_idx
    on port_observations (address, port, transport, observed_at desc);
create index if not exists port_observations_device_idx
    on port_observations (device_id, observed_at desc) where device_id is not null;
