-- Milestone 9: durable maintenance planning, normalized reservations, and
-- bounded occurrence expansion. PostgreSQL remains authoritative for all
-- schedule and reservation data.

create table if not exists maintenance_events (
    id                    uuid primary key default gen_random_uuid(),
    name                  text not null check (char_length(name) between 1 and 200),
    description           text check (description is null or char_length(description) <= 4000),
    timezone              text not null check (char_length(timezone) between 1 and 128),
    start_at              timestamptz not null,
    end_at                timestamptz not null,
    recurrence_rule       text check (recurrence_rule is null or char_length(recurrence_rule) <= 2000),
    lead_in_seconds       bigint not null default 0 check (lead_in_seconds between 0 and 2678400),
    cooldown_seconds      bigint not null default 0 check (cooldown_seconds between 0 and 2678400),
    disruptive            boolean not null default false,
    notification_policy   jsonb not null default '{}'::jsonb,
    owner                 text check (owner is null or char_length(owner) <= 256),
    source                text check (source is null or char_length(source) <= 256),
    notes                 text check (notes is null or char_length(notes) <= 8000),
    links                 jsonb not null default '[]'::jsonb,
    state                 text not null default 'scheduled'
                          check (state in ('draft', 'scheduled', 'upcoming', 'active',
                                           'overrunning', 'completed', 'cancelled')),
    version               integer not null default 1 check (version >= 1),
    created_by            uuid references users(id),
    created_at            timestamptz not null default now(),
    updated_at            timestamptz not null default now(),
    constraint maintenance_events_range_check check (end_at > start_at),
    constraint maintenance_events_notification_policy_object check
        (jsonb_typeof(notification_policy) = 'object'),
    constraint maintenance_events_links_array check (jsonb_typeof(links) = 'array')
);

create index if not exists maintenance_events_state_start_idx
    on maintenance_events (state, start_at, id);
create index if not exists maintenance_events_created_idx
    on maintenance_events (created_at desc, id desc);

create table if not exists maintenance_resources (
    id                uuid primary key default gen_random_uuid(),
    event_id          uuid not null references maintenance_events(id) on delete restrict,
    role              text not null check (role in ('target', 'required', 'affected', 'exclusive')),
    resource_kind     text,
    resource_id       uuid,
    resource_key      text,
    expected_failure  boolean not null default false,
    created_at        timestamptz not null default now(),
    constraint maintenance_resources_identity_check check (
        (resource_key is not null and resource_kind is null and resource_id is null)
        or
        (resource_key is null and resource_kind is not null and resource_id is not null)
    ),
    constraint maintenance_resources_kind_check check
        (resource_kind is null or char_length(resource_kind) between 1 and 64),
    constraint maintenance_resources_key_check check
        (resource_key is null or char_length(resource_key) between 1 and 128)
);

create index if not exists maintenance_resources_event_idx
    on maintenance_resources (event_id, role, id);
create index if not exists maintenance_resources_entity_idx
    on maintenance_resources (resource_kind, resource_id, role);
create index if not exists maintenance_resources_key_idx
    on maintenance_resources (resource_key, role);

create table if not exists maintenance_occurrences (
    id                 uuid primary key,
    event_id           uuid not null references maintenance_events(id) on delete restrict,
    occurrence_key     text not null,
    occurrence_index   integer not null check (occurrence_index >= 0),
    start_at           timestamptz not null,
    end_at             timestamptz not null,
    reservation_start  timestamptz not null,
    reservation_end    timestamptz not null,
    timezone           text not null,
    created_at         timestamptz not null default now(),
    constraint maintenance_occurrences_range_check check (end_at > start_at),
    constraint maintenance_occurrences_reservation_check check
        (reservation_start <= start_at and reservation_end >= end_at),
    unique (event_id, occurrence_key)
);

create index if not exists maintenance_occurrences_event_start_idx
    on maintenance_occurrences (event_id, start_at, id);
create index if not exists maintenance_occurrences_reservation_idx
    on maintenance_occurrences (reservation_start, reservation_end, id);
