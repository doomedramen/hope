-- Milestone 1: evidence, identity rules/pins, merge/split/undo history.
-- Spec §17 M1, §4; design docs/design/m1-inventory.md §1 slice 2.

create table if not exists identity_rules (
    id         uuid primary key default gen_random_uuid(),
    device_id  uuid not null references devices(id),
    rule_type  text not null check (rule_type in
                   ('agent_id', 'machine_id', 'hardware_uuid', 'mac', 'serial',
                    'proxmox_vmid', 'docker_container_id', 'hostname',
                    'ssh_host_key', 'snmp_engine_id', 'tls_cert_identity')),
    value      text not null,
    pinned     boolean not null default false,
    pinned_by  uuid references users(id),
    pinned_at  timestamptz,
    version    integer not null default 1,
    created_at timestamptz not null default now(),
    updated_at timestamptz not null default now()
);

create index if not exists identity_rules_device_id_idx on identity_rules (device_id);
create index if not exists identity_rules_type_value_idx on identity_rules (rule_type, value);
-- A pinned identifier can't be claimed by more than one device.
create unique index if not exists identity_rules_pinned_unique
    on identity_rules (rule_type, value) where pinned;

create table if not exists merge_events (
    id                  uuid primary key default gen_random_uuid(),
    kind                text not null check (kind in ('merge', 'split', 'undo_merge')),
    survivor_device_id  uuid not null references devices(id),
    absorbed_device_id  uuid not null references devices(id),
    performed_by        uuid references users(id),
    reason              text,
    score               real,
    explanation         jsonb,
    undone_at           timestamptz,
    undone_by           uuid references users(id),
    created_at          timestamptz not null default now()
);

create index if not exists merge_events_survivor_idx on merge_events (survivor_device_id);
create index if not exists merge_events_absorbed_idx on merge_events (absorbed_device_id);

create table if not exists evidence (
    id              uuid primary key default gen_random_uuid(),
    subject_table   text not null,
    subject_id      uuid not null,
    source_type     text not null check (source_type in
                        ('network_scan', 'agent', 'docker', 'mdns', 'snmp', 'api', 'manual')),
    source_instance text,
    attribute       text not null,
    value           jsonb not null,
    confidence      real not null,
    first_seen      timestamptz not null default now(),
    last_seen       timestamptz not null default now(),
    expires_at      timestamptz,
    absent          boolean not null default false,
    confirmed_by    uuid references users(id),
    overridden      boolean not null default false,
    version         integer not null default 1,
    created_at      timestamptz not null default now(),
    updated_at      timestamptz not null default now()
);

create index if not exists evidence_subject_idx on evidence (subject_table, subject_id);
create index if not exists evidence_source_idx on evidence (source_type, source_instance);
create index if not exists evidence_subject_attribute_idx
    on evidence (subject_table, subject_id, attribute, last_seen desc);
