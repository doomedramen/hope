-- Milestone 9: retain the exact maintenance occurrence that intentionally
-- suppressed an expected incident notification.

create table if not exists maintenance_notification_suppressions (
    id                     uuid primary key default gen_random_uuid(),
    incident_id            uuid not null references incidents(id) on delete cascade,
    maintenance_event_id  uuid not null references maintenance_events(id) on delete restrict,
    maintenance_occurrence_id uuid not null references maintenance_occurrences(id) on delete restrict,
    resource_role          text not null check (resource_role in ('target', 'required', 'affected', 'exclusive')),
    resource_kind          text,
    resource_id            uuid,
    resource_key           text,
    expected_failure       boolean not null default true,
    event_type             text not null check (event_type in ('incident.opened', 'incident.recovered')),
    reason                 text not null check (char_length(reason) between 1 and 512),
    created_at             timestamptz not null default now(),
    constraint maintenance_notification_suppressions_identity_check check (
        (resource_key is not null and resource_kind is null and resource_id is null)
        or
        (resource_key is null and resource_kind is not null and resource_id is not null)
    ),
    unique (incident_id, maintenance_occurrence_id, event_type, resource_role)
);

create index if not exists maintenance_notification_suppressions_incident_idx
    on maintenance_notification_suppressions (incident_id, created_at desc);

create index if not exists maintenance_notification_suppressions_event_idx
    on maintenance_notification_suppressions (maintenance_event_id, created_at desc);
