-- Milestone 2: an operator-approved discovery scope gates all scan work.
-- A draft always loses confirmation when its CIDR exclusions/profile changes;
-- workers must require confirmed_at before targeting this network.

create table if not exists discovery_scopes (
    network_id                  uuid primary key references networks(id),
    excluded_cidrs              jsonb not null default '[]'::jsonb,
    scan_profile                text not null default 'normal'
                                    check (scan_profile in ('normal', 'low_impact')),
    target_count                bigint not null check (target_count >= 0),
    confirmed_target_count      bigint,
    confirmed_at                timestamptz,
    enabled                     boolean not null default true,
    tcp_concurrency             integer not null default 64
                                    check (tcp_concurrency between 1 and 512),
    per_host_concurrency        integer not null default 2
                                    check (per_host_concurrency between 1 and 32),
    connect_timeout_ms          integer not null default 1000
                                    check (connect_timeout_ms between 50 and 10000),
    discovery_interval_seconds  integer not null default 900
                                    check (discovery_interval_seconds between 60 and 86400),
    full_tcp_interval_seconds   integer not null default 86400
                                    check (full_tcp_interval_seconds between 3600 and 604800),
    version                     integer not null default 1,
    created_at                  timestamptz not null default now(),
    updated_at                  timestamptz not null default now(),
    check (
        (confirmed_at is null and confirmed_target_count is null)
        or (confirmed_at is not null and confirmed_target_count = target_count)
    )
);
