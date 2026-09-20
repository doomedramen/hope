-- Milestone 8: dependency graph safeguards, provenance, and confirmation.
-- Existing dependency rows remain durable. Historical manual and inferred rows
-- are treated as confirmed because they predate the suggestion state.

alter table dependency_edges
    add column if not exists confirmation_state text,
    add column if not exists confirmed_by uuid references users(id),
    add column if not exists confirmed_at timestamptz;

update dependency_edges
set confirmation_state = case
        when origin in ('manual', 'inferred') then 'confirmed'
        else 'suggested'
    end,
    confirmed_at = case
        when origin in ('manual', 'inferred') then coalesce(confirmed_at, updated_at, created_at, now())
        else confirmed_at
    end
where confirmation_state is null;

alter table dependency_edges
    alter column confirmation_state set default 'confirmed',
    alter column confirmation_state set not null;

do $$
begin
    alter table dependency_edges
        add constraint dependency_edges_confirmation_state_check
        check (confirmation_state in ('suggested', 'confirmed', 'rejected'));
exception
    when duplicate_object then null;
end
$$;

create index if not exists dependency_edges_confirmation_idx
    on dependency_edges (confirmation_state, provider_kind, provider_id);
