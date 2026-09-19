-- Milestone 1: canonical inventory core entity tables.
-- Spec §17 M1; design docs/design/m1-inventory.md §1 slice 1.

create table if not exists sites (
    id         uuid primary key default gen_random_uuid(),
    name       text not null,
    timezone   text not null default 'UTC',
    version    integer not null default 1,
    created_at timestamptz not null default now(),
    updated_at timestamptz not null default now()
);

create table if not exists networks (
    id          uuid primary key default gen_random_uuid(),
    site_id     uuid references sites(id),
    cidr        cidr not null,
    vlan        integer,
    gateway     inet,
    scan_policy jsonb not null default '{}'::jsonb,
    name        text,
    version     integer not null default 1,
    created_at  timestamptz not null default now(),
    updated_at  timestamptz not null default now()
);

create table if not exists devices (
    id                  uuid primary key default gen_random_uuid(),
    device_type         text not null check (device_type in
                            ('physical_host', 'vm', 'lxc', 'container_host', 'appliance', 'unknown')),
    name                text,
    status              text not null default 'active' check (status in
                            ('active', 'stale', 'archived', 'merged')),
    identity_confidence real not null default 0,
    canonical_of        uuid references devices(id),
    version             integer not null default 1,
    created_at          timestamptz not null default now(),
    updated_at          timestamptz not null default now()
);

create index if not exists devices_canonical_of_idx on devices (canonical_of);

create table if not exists interfaces (
    id          uuid primary key default gen_random_uuid(),
    device_id   uuid not null references devices(id),
    mac         macaddr,
    description text,
    first_seen  timestamptz not null default now(),
    last_seen   timestamptz not null default now(),
    version     integer not null default 1,
    created_at  timestamptz not null default now(),
    updated_at  timestamptz not null default now()
);

create index if not exists interfaces_device_id_idx on interfaces (device_id);
create index if not exists interfaces_mac_idx on interfaces (mac);

create table if not exists addresses (
    id           uuid primary key default gen_random_uuid(),
    interface_id uuid not null references interfaces(id),
    ip           inet not null,
    address_type text not null default 'unknown' check (address_type in
                     ('static', 'dhcp', 'unknown')),
    first_seen   timestamptz not null default now(),
    last_seen    timestamptz not null default now(),
    is_current   boolean not null default true,
    version      integer not null default 1,
    created_at   timestamptz not null default now(),
    updated_at   timestamptz not null default now()
);

create index if not exists addresses_interface_id_idx on addresses (interface_id);
-- IP history: never mutate/delete on move, close the old row (is_current
-- = false) and insert a new one. At most one live owner per IP.
create unique index if not exists addresses_current_ip_unique
    on addresses (ip) where is_current;

create table if not exists workloads (
    id                 uuid primary key default gen_random_uuid(),
    workload_type      text not null check (workload_type in
                           ('vm', 'lxc', 'docker_container', 'other')),
    host_device_id     uuid references devices(id),
    runtime_id         text,
    image_or_template  text,
    name               text,
    status             text,
    version            integer not null default 1,
    created_at         timestamptz not null default now(),
    updated_at         timestamptz not null default now()
);

create index if not exists workloads_host_device_id_idx on workloads (host_device_id);

create table if not exists services (
    id              uuid primary key default gen_random_uuid(),
    name            text,
    protocol        text,
    product         text,
    product_version text,
    owner_kind      text not null check (owner_kind in ('device', 'workload')),
    owner_id        uuid not null,
    version         integer not null default 1,
    created_at      timestamptz not null default now(),
    updated_at      timestamptz not null default now()
);

create index if not exists services_owner_idx on services (owner_kind, owner_id);

create table if not exists endpoints (
    id            uuid primary key default gen_random_uuid(),
    service_id    uuid not null references services(id),
    endpoint_type text not null check (endpoint_type in
                      ('socket', 'published_port', 'url', 'dns_name')),
    address       inet,
    port          integer,
    url           text,
    dns_name      text,
    first_seen    timestamptz not null default now(),
    last_seen     timestamptz not null default now(),
    is_current    boolean not null default true,
    version       integer not null default 1,
    created_at    timestamptz not null default now(),
    updated_at    timestamptz not null default now()
);

create index if not exists endpoints_service_id_idx on endpoints (service_id);
