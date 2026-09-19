-- Milestone 3: durable monitor proposals.
-- Proposals point only at canonical service and endpoint rows. They describe
-- policy output for a later monitor implementation; they never execute a
-- check or create a monitor.

create table if not exists monitor_proposals (
    id                    uuid primary key default gen_random_uuid(),
    service_id            uuid not null references services(id),
    endpoint_id           uuid not null references endpoints(id),
    rule_id               text not null,
    rule_version          integer not null default 1,
    target_identity       text not null,
    target                jsonb not null,
    protocol              text not null,
    product               text,
    product_version       text,
    check_type            text not null check (check_type in ('http', 'tcp')),
    check_config          jsonb not null default '{}'::jsonb,
    confidence            real not null check (confidence between 0 and 1),
    auto_create_allowed   boolean not null default false,
    resolved_from         jsonb not null default '{}'::jsonb,
    source_evidence_id    uuid references evidence(id),
    source_rule_id        text,
    status                text not null default 'pending'
                              check (status in ('pending', 'approved', 'rejected')),
    decision_source       text not null default 'automatic'
                              check (decision_source in ('automatic', 'manual')),
    decided_by            uuid references users(id),
    decided_at            timestamptz,
    user_overrides        jsonb not null default '{}'::jsonb,
    override_by           uuid references users(id),
    override_at           timestamptz,
    version               integer not null default 1,
    created_at            timestamptz not null default now(),
    updated_at            timestamptz not null default now(),

    constraint monitor_proposals_target_identity_check
        check (target_identity like 'endpoint:%'),
    constraint monitor_proposals_decision_consistent check (
        (status = 'pending' and decision_source = 'automatic'
            and decided_by is null and decided_at is null)
        or (status in ('approved', 'rejected') and decision_source = 'manual'
            and decided_by is not null and decided_at is not null)
    ),
    constraint monitor_proposals_override_consistent check (
        (user_overrides = '{}'::jsonb and override_by is null and override_at is null)
        or (jsonb_typeof(user_overrides) = 'object'
            and override_by is not null and override_at is not null)
    )
);

create unique index if not exists monitor_proposals_rule_endpoint_unique
    on monitor_proposals (rule_id, endpoint_id);

create unique index if not exists monitor_proposals_rule_target_unique
    on monitor_proposals (rule_id, target_identity);

create index if not exists monitor_proposals_status_idx
    on monitor_proposals (status, created_at, id);

create index if not exists monitor_proposals_service_idx
    on monitor_proposals (service_id, created_at desc);

create index if not exists monitor_proposals_endpoint_idx
    on monitor_proposals (endpoint_id, created_at desc);
