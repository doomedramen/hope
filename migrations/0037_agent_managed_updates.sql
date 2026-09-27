-- Outbound update operations share the existing operation history with SSH updates.
alter table agent_update_operations alter column device_id drop not null;
alter table agent_update_operations add column transport text not null default 'ssh' check (transport in ('ssh','agent'));
alter table agent_update_operations add column request_key text;
create unique index agent_updates_request_key on agent_update_operations(agent_id,request_key) where request_key is not null;
alter table agent_update_operations drop constraint agent_update_operations_state_check;
alter table agent_update_operations add constraint agent_update_operations_state_check check (state in ('pending','verifying','downloading','installing','restarting','awaiting_health','succeeded','failed','rolled_back','pinned','incompatible','cancelled'));
drop index agent_update_operations_one_active_idx;
create unique index agent_update_operations_one_active_idx on agent_update_operations(agent_id) where state in ('pending','verifying','downloading','installing','restarting','awaiting_health');
create table agent_managed_update_policies (
    agent_id uuid primary key references agents(id) on delete cascade,
    mode text not null default 'notify' check (mode in ('manual','notify','automatic')),
    channel text not null default 'stable' check (channel in ('stable','canary')),
    pinned_version text,
    rollout_percent integer not null default 5 check (rollout_percent between 0 and 100),
    window_start_utc integer not null default 0 check (window_start_utc between 0 and 23),
    window_end_utc integer not null default 0 check (window_end_utc between 0 and 23),
    updated_at timestamptz not null default now(),
    updated_by uuid references users(id)
);
