-- Milestone 1: identity-match review queue.
-- Spec §17 M1, §4.2; design docs/design/m1-inventory.md §3 "Review queue".
-- Ambiguous matches (0.4 <= score < 0.85) land here instead of mutating
-- the device graph; an operator confirms or rejects.

create table if not exists identity_suggestions (
    id                 uuid primary key default gen_random_uuid(),
    candidate_device_id uuid not null references devices(id),
    -- The observed identifiers that produced this suggestion, and the
    -- full identity::score() explanation, so the UI's "Why?" panel and
    -- confirm/reject flow need no re-scoring.
    observed           jsonb not null,
    explanation        jsonb not null,
    score              real not null,
    status             text not null default 'pending'
                           check (status in ('pending', 'confirmed', 'rejected')),
    resolved_by        uuid references users(id),
    resolved_at        timestamptz,
    created_at         timestamptz not null default now(),
    updated_at         timestamptz not null default now()
);

create index if not exists identity_suggestions_pending_idx
    on identity_suggestions (created_at)
    where status = 'pending';
create index if not exists identity_suggestions_candidate_idx
    on identity_suggestions (candidate_device_id);
