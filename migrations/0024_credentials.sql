-- Milestone 6: encrypted operator credentials.
-- Secret material is authenticated ciphertext only. The master key is
-- deliberately external to PostgreSQL (see ADR-0015).

create table if not exists credentials (
    id                  uuid primary key default gen_random_uuid(),
    name                text not null check (char_length(name) between 1 and 200),
    kind                text not null check (kind in ('ssh_private_key', 'ssh_password')),
    scope               jsonb not null,
    -- 1 MiB plaintext ceiling plus envelope version, nonce, and Poly1305 tag.
    secret_ciphertext   bytea not null check (octet_length(secret_ciphertext) between 1 and 1048605),
    version             integer not null default 1 check (version > 0),
    created_by          uuid references users(id),
    created_at          timestamptz not null default now(),
    updated_at          timestamptz not null default now(),
    last_used_at        timestamptz,
    revoked_at          timestamptz,
    deleted_at          timestamptz
);

create index if not exists credentials_active_idx
    on credentials (updated_at desc)
    where deleted_at is null and revoked_at is null;

create index if not exists credentials_scope_idx
    on credentials using gin (scope);

create index if not exists credentials_kind_idx
    on credentials (kind, updated_at desc)
    where deleted_at is null;

create table if not exists credential_associations (
    id               uuid primary key default gen_random_uuid(),
    credential_id    uuid not null references credentials(id),
    device_id        uuid not null references devices(id),
    purpose          text not null check (purpose in ('agent_install', 'management')),
    associated_at    timestamptz not null default now(),
    disassociated_at timestamptz,
    unique (credential_id, device_id, purpose)
);

create index if not exists credential_associations_device_idx
    on credential_associations (device_id, associated_at desc)
    where disassociated_at is null;
