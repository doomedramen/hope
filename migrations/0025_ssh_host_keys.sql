-- Milestone 6: SSH host-key trust records.
-- A changed key is retained as a blocked observation until an operator
-- explicitly trusts the replacement; it is never accepted by TOFU again.

create table if not exists ssh_host_keys (
    id                         uuid primary key default gen_random_uuid(),
    device_id                  uuid references devices(id),
    host                       text not null check (char_length(host) between 1 and 253),
    port                       integer not null check (port between 1 and 65535),
    key_type                   text not null check (char_length(key_type) between 1 and 128),
    fingerprint_sha256         text not null check (fingerprint_sha256 like 'SHA256:%'),
    previous_fingerprint_sha256 text,
    state                      text not null default 'pending'
                                   check (state in ('pending', 'trusted', 'changed', 'revoked')),
    first_seen_at              timestamptz not null default now(),
    last_seen_at               timestamptz not null default now(),
    trusted_at                 timestamptz,
    changed_at                 timestamptz,
    revoked_at                 timestamptz,
    updated_at                 timestamptz not null default now(),
    version                    integer not null default 1 check (version > 0),
    unique (host, port)
);

create index if not exists ssh_host_keys_device_idx
    on ssh_host_keys (device_id, last_seen_at desc);

create index if not exists ssh_host_keys_state_idx
    on ssh_host_keys (state, last_seen_at desc);
