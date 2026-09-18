-- Milestone 0: job queue + admin bootstrap schema.
-- Spec §5.1 / ADR-0006: custom Postgres-backed job queue with
-- SKIP LOCKED dequeue, lease+heartbeat, idempotency keys, and backoff.

create extension if not exists pgcrypto;

create table if not exists jobs (
    id              uuid primary key default gen_random_uuid(),
    job_type        text not null,
    idempotency_key text not null,
    payload         jsonb not null default '{}'::jsonb,
    status          text not null default 'pending'
                        check (status in ('pending', 'running', 'succeeded', 'failed', 'cancelled')),
    progress        jsonb not null default '{}'::jsonb,
    attempts        integer not null default 0,
    max_attempts    integer not null default 5,
    run_at          timestamptz not null default now(),
    locked_by       text,
    locked_at       timestamptz,
    lease_expires_at timestamptz,
    cancel_requested boolean not null default false,
    last_error      text,
    created_at      timestamptz not null default now(),
    updated_at      timestamptz not null default now(),

    constraint jobs_idempotency_key_unique unique (job_type, idempotency_key)
);

create index if not exists jobs_claimable_idx
    on jobs (run_at)
    where status = 'pending';

create index if not exists jobs_status_idx on jobs (status);

-- Spec §17 M0 / §13.2 first-run flow: single admin bootstrap.
create table if not exists users (
    id            uuid primary key default gen_random_uuid(),
    email         text not null unique,
    password_hash text not null,
    created_at    timestamptz not null default now(),
    updated_at    timestamptz not null default now()
);
