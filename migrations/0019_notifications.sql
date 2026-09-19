-- M4: durable notification routes and idempotent incident deliveries.

create table if not exists notification_channels (
    id          uuid primary key default gen_random_uuid(),
    name        text not null check (length(name) between 1 and 128),
    provider    text not null check (provider in ('webhook', 'ntfy')),
    config      jsonb not null default '{}'::jsonb,
    enabled     boolean not null default true,
    created_at  timestamptz not null default now(),
    updated_at  timestamptz not null default now()
);

create table if not exists notification_routes (
    id              uuid primary key default gen_random_uuid(),
    channel_id      uuid not null references notification_channels(id),
    min_severity    text not null default 'warning'
                    check (min_severity in ('info', 'notice', 'warning', 'critical')),
    event_types     jsonb not null default '["incident.opened", "incident.recovered"]'::jsonb,
    delay_seconds   integer not null default 0 check (delay_seconds between 0 and 86400),
    enabled         boolean not null default true,
    created_at      timestamptz not null default now(),
    updated_at      timestamptz not null default now(),
    check (jsonb_typeof(event_types) = 'array')
);

create index if not exists notification_routes_channel_idx
    on notification_routes (channel_id, enabled);

create table if not exists notification_deliveries (
    id              uuid primary key default gen_random_uuid(),
    incident_id     uuid not null references incidents(id),
    channel_id      uuid not null references notification_channels(id),
    event_type      text not null check (event_type in ('incident.opened', 'incident.recovered')),
    severity        text not null check (severity in ('info', 'notice', 'warning', 'critical')),
    payload         jsonb not null,
    status          text not null default 'pending'
                    check (status in ('pending', 'sending', 'sent')),
    attempts        integer not null default 0 check (attempts >= 0),
    last_error      text,
    delivered_at    timestamptz,
    created_at      timestamptz not null default now(),
    updated_at      timestamptz not null default now(),
    unique (incident_id, channel_id, event_type)
);

create index if not exists notification_deliveries_status_idx
    on notification_deliveries (status, created_at);
