-- Milestone 5: agent-backed monitors.
--
-- Agent monitors target an enrolled agent directly. They do not require a
-- synthetic service or endpoint because heartbeat and host metrics are
-- observations of the agent identity, not network reachability checks.

alter table monitors
    alter column service_id drop not null,
    alter column endpoint_id drop not null;

alter table monitors
    add column if not exists agent_id uuid references agents(id) on delete cascade;

alter table monitors
    drop constraint if exists monitors_monitor_type_check;

alter table monitors
    add constraint monitors_monitor_type_check
        check (monitor_type in (
            'icmp', 'tcp', 'http', 'https', 'dns', 'tls',
            'agent_heartbeat', 'agent_metric'
        ));

alter table monitors
    add constraint monitors_target_check
        check (
            (
                monitor_type in ('icmp', 'tcp', 'http', 'https', 'dns', 'tls')
                and service_id is not null
                and endpoint_id is not null
                and agent_id is null
            )
            or (
                monitor_type in ('agent_heartbeat', 'agent_metric')
                and service_id is null
                and endpoint_id is null
                and agent_id is not null
            )
        );

create index if not exists monitors_agent_due_idx
    on monitors (agent_id, next_run_at, id)
    where enabled and agent_id is not null;
