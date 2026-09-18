-- Milestone 0 slice 2: agent enrollment + mTLS identity (ADR-0007, ADR-0008).

create table if not exists enrollment_tokens (
    id          uuid primary key default gen_random_uuid(),
    token_hash  text not null unique,
    expires_at  timestamptz not null,
    used_at     timestamptz,
    created_at  timestamptz not null default now()
);

create index if not exists enrollment_tokens_unused_idx
    on enrollment_tokens (expires_at)
    where used_at is null;

create table if not exists agents (
    id               uuid primary key default gen_random_uuid(),
    cert_fingerprint text not null unique,
    cert_serial      text not null,
    hostname         text,
    revoked_at       timestamptz,
    last_seen        timestamptz,
    created_at       timestamptz not null default now()
);
