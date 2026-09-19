-- Milestone 1: change-event and audit-log framework.
-- Spec §17 M1, §11, §12.5; design docs/design/m1-inventory.md §1 slice 4.

create table if not exists change_events (
    id             uuid primary key default gen_random_uuid(),
    entity_kind    text not null,
    entity_id      uuid not null,
    category       text not null,
    severity       text not null check (severity in ('info', 'notice', 'warning', 'critical')),
    before         jsonb,
    after          jsonb,
    evidence_source text,
    occurred_at    timestamptz not null default now(),
    acknowledged   boolean not null default false
);

create index if not exists change_events_entity_idx
    on change_events (entity_kind, entity_id, occurred_at desc);
create index if not exists change_events_occurred_at_idx on change_events (occurred_at desc);

create table if not exists audit_events (
    id            uuid primary key default gen_random_uuid(),
    actor_user_id uuid references users(id),
    actor_kind    text not null check (actor_kind in ('operator', 'agent', 'worker', 'system')),
    action        text not null,
    target_kind   text,
    target_id     uuid,
    result        text not null check (result in ('success', 'failure')),
    detail        jsonb,
    ip            inet,
    occurred_at   timestamptz not null default now()
);

create index if not exists audit_events_occurred_at_idx on audit_events (occurred_at desc);
create index if not exists audit_events_actor_idx on audit_events (actor_user_id, occurred_at desc);
