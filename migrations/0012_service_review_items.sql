-- Milestone 3: durable service fingerprint review queue.
-- Low-confidence product candidates and conflicts are retained with the
-- evidence and rule details needed for an explainable operator decision.

create table if not exists service_review_items (
    id                    uuid primary key default gen_random_uuid(),
    service_id            uuid not null references services(id),
    fingerprint_evidence_id uuid not null references evidence(id),
    source_instance       text,
    -- Stable candidate identity. It excludes scan/evidence IDs so an
    -- unchanged candidate from a later scan does not resurface.
    candidate_key         text not null,
    candidate             jsonb not null,
    candidates            jsonb not null default '[]'::jsonb,
    evidence              jsonb not null,
    rule_id               text not null,
    fixture_version       integer not null,
    confidence            real not null check (confidence between 0 and 1),
    reason                text not null,
    policy_threshold      real not null check (policy_threshold between 0 and 1),
    status                text not null default 'pending'
                              check (status in ('pending', 'confirmed', 'rejected')),
    resolved_by           uuid references users(id),
    resolved_at           timestamptz,
    created_at            timestamptz not null default now(),
    updated_at            timestamptz not null default now(),

    constraint service_review_items_resolution_consistent check (
        (status = 'pending' and resolved_by is null and resolved_at is null)
        or (status in ('confirmed', 'rejected')
            and resolved_by is not null and resolved_at is not null)
    )
);

create unique index if not exists service_review_items_candidate_unique
    on service_review_items (service_id, candidate_key);

create index if not exists service_review_items_pending_idx
    on service_review_items (created_at, id)
    where status = 'pending';

create index if not exists service_review_items_service_idx
    on service_review_items (service_id, created_at desc);
