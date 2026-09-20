-- Milestone 9: maintenance overruns use the existing notification channels
-- and routes, but have their own idempotent delivery identity.

create table if not exists maintenance_notification_deliveries (
    id                       uuid primary key default gen_random_uuid(),
    maintenance_event_id    uuid not null references maintenance_events(id) on delete restrict,
    maintenance_occurrence_id uuid not null references maintenance_occurrences(id) on delete restrict,
    channel_id              uuid not null references notification_channels(id) on delete restrict,
    event_type              text not null check (event_type = 'maintenance.overrun'),
    severity                text not null check (severity in ('info', 'notice', 'warning', 'critical')),
    payload                 jsonb not null,
    status                  text not null default 'pending'
                            check (status in ('pending', 'sending', 'sent')),
    attempts                integer not null default 0 check (attempts >= 0),
    last_error              text,
    delivered_at            timestamptz,
    created_at              timestamptz not null default now(),
    updated_at              timestamptz not null default now(),
    unique (maintenance_event_id, maintenance_occurrence_id, channel_id, event_type)
);

create index if not exists maintenance_notification_deliveries_status_idx
    on maintenance_notification_deliveries (status, created_at);
