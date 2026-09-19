-- Milestone 1: containment and dependency graph primitives.
-- Spec §17 M1, §9; design docs/design/m1-inventory.md §1 slice 3.

create table if not exists containment_edges (
    id         uuid primary key default gen_random_uuid(),
    parent_kind text not null,
    parent_id   uuid not null,
    child_kind  text not null,
    child_id    uuid not null,
    relation    text not null check (relation in
                    ('hosts_vm', 'hosts_container', 'has_interface', 'runs_service')),
    version     integer not null default 1,
    created_at  timestamptz not null default now(),
    updated_at  timestamptz not null default now(),

    constraint containment_edges_unique
        unique (parent_kind, parent_id, child_kind, child_id, relation)
);

create index if not exists containment_edges_parent_idx on containment_edges (parent_kind, parent_id);
create index if not exists containment_edges_child_idx on containment_edges (child_kind, child_id);

create table if not exists dependency_edges (
    id                 uuid primary key default gen_random_uuid(),
    provider_kind      text not null,
    provider_id        uuid not null,
    consumer_kind      text not null,
    consumer_id        uuid not null,
    dependency_kind    text not null,
    criticality        text not null default 'soft' check (criticality in ('hard', 'soft')),
    origin             text not null default 'manual' check (origin in ('manual', 'inferred')),
    confidence         real,
    inference_rule     text,
    health_propagation text not null default 'none' check (health_propagation in
                           ('none', 'propagate', 'suppress_only')),
    version            integer not null default 1,
    created_at         timestamptz not null default now(),
    updated_at         timestamptz not null default now(),

    constraint dependency_edges_unique
        unique (provider_kind, provider_id, consumer_kind, consumer_id, dependency_kind)
);

create index if not exists dependency_edges_provider_idx on dependency_edges (provider_kind, provider_id);
create index if not exists dependency_edges_consumer_idx on dependency_edges (consumer_kind, consumer_id);
