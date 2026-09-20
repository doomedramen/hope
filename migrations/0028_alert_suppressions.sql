-- Milestone 8: durable reasons for downstream notification suppression.
-- Incident rows remain authoritative; this table only records why a delivery
-- was intentionally not created for an otherwise-notifiable incident.

create table if not exists incident_notification_suppressions (
    id                 uuid primary key default gen_random_uuid(),
    incident_id        uuid not null references incidents(id) on delete cascade,
    root_incident_id   uuid not null references incidents(id) on delete cascade,
    dependency_edge_id uuid not null references dependency_edges(id) on delete restrict,
    dependency_path    jsonb not null default '[]'::jsonb,
    provider_kind      text not null check (char_length(provider_kind) between 1 and 64),
    provider_id        uuid not null,
    event_type         text not null check (event_type in ('incident.opened', 'incident.recovered')),
    reason             text not null check (char_length(reason) between 1 and 512),
    created_at         timestamptz not null default now(),
    unique (incident_id, root_incident_id, dependency_edge_id, event_type)
);

create index if not exists incident_notification_suppressions_incident_idx
    on incident_notification_suppressions (incident_id, created_at desc);

create index if not exists incident_notification_suppressions_root_idx
    on incident_notification_suppressions (root_incident_id, created_at desc);
